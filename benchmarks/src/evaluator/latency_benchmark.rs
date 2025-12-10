use crate::datasets::Query;
use crate::evaluator::metrics::{
    LatencyBenchmarkConfig, LatencyBenchmarkResult, LatencyCalculator, LatencyMeasurement,
    LatencyStats, SystemInfo,
};
use crate::search_client::{create_search_request, OmniSearchClient};
use anyhow::Result;
use chrono::Utc;
use futures::stream::{self, StreamExt};
use indicatif::{ProgressBar, ProgressStyle};
use omni_searcher::models::SearchMode;
use sqlx::{Pool, Postgres, Row};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::Semaphore;
use tracing::{debug, info, warn};

pub struct LatencyBenchmarkEvaluator {
    search_client: OmniSearchClient,
    db_pool: Pool<Postgres>,
}

impl LatencyBenchmarkEvaluator {
    pub fn new(search_client: OmniSearchClient, db_pool: Pool<Postgres>) -> Self {
        Self {
            search_client,
            db_pool,
        }
    }

    /// Run the latency benchmark
    pub async fn run_benchmark(
        &self,
        queries: Vec<Query>,
        config: &LatencyBenchmarkConfig,
        dataset_name: &str,
        search_mode: SearchMode,
    ) -> Result<LatencyBenchmarkResult> {
        info!(
            "Starting latency benchmark: {} queries, concurrency={}, warmup={}",
            config.num_queries, config.concurrency, config.warmup_queries
        );

        // Check if search service is healthy
        if !self.search_client.health_check().await? {
            return Err(anyhow::anyhow!("Search service is not healthy"));
        }

        // Limit queries to config.num_queries
        let benchmark_queries: Vec<Query> = queries
            .into_iter()
            .take(config.num_queries + config.warmup_queries)
            .collect();

        if benchmark_queries.len() < config.warmup_queries {
            return Err(anyhow::anyhow!(
                "Not enough queries for warmup. Have {}, need at least {}",
                benchmark_queries.len(),
                config.warmup_queries
            ));
        }

        // Split into warmup and benchmark queries
        let (warmup_queries, benchmark_queries): (Vec<_>, Vec<_>) = benchmark_queries
            .into_iter()
            .enumerate()
            .partition(|(i, _)| *i < config.warmup_queries);

        let warmup_queries: Vec<Query> = warmup_queries.into_iter().map(|(_, q)| q).collect();
        let benchmark_queries: Vec<Query> = benchmark_queries.into_iter().map(|(_, q)| q).collect();

        // Run warmup phase
        info!("Running warmup phase ({} queries)...", warmup_queries.len());
        self.run_warmup(&warmup_queries, config.concurrency, search_mode.clone())
            .await?;

        // Run benchmark phase
        info!(
            "Running benchmark phase ({} queries)...",
            benchmark_queries.len()
        );
        let (measurements, total_duration) = self
            .run_benchmark_phase(&benchmark_queries, config, search_mode.clone())
            .await?;

        // Calculate statistics
        let latency_stats =
            LatencyCalculator::calculate_stats(&measurements, total_duration.as_secs_f64());

        // Get system info
        let system_info = self.get_system_info().await?;

        let result = LatencyBenchmarkResult {
            dataset_name: dataset_name.to_string(),
            search_mode: format!("{:?}", search_mode),
            config: config.clone(),
            latency_stats,
            measurements,
            system_info,
            run_timestamp: Utc::now(),
        };

        Ok(result)
    }

    /// Run warmup queries to warm up caches
    async fn run_warmup(
        &self,
        queries: &[Query],
        concurrency: usize,
        search_mode: SearchMode,
    ) -> Result<()> {
        let semaphore = Arc::new(Semaphore::new(concurrency));
        let completed = Arc::new(AtomicUsize::new(0));
        let total = queries.len();

        let progress_bar = ProgressBar::new(total as u64);
        progress_bar.set_style(
            ProgressStyle::default_bar()
                .template("{spinner:.yellow} Warmup [{bar:40.yellow}] {pos}/{len}")
                .unwrap()
                .progress_chars("=>-"),
        );

        let futures: Vec<_> = queries
            .iter()
            .map(|query| {
                let semaphore = semaphore.clone();
                let search_client = &self.search_client;
                let search_mode = search_mode.clone();
                let completed = completed.clone();
                let progress_bar = progress_bar.clone();

                async move {
                    let _permit = semaphore.acquire().await.unwrap();
                    let request = create_search_request(query.text.clone(), search_mode);
                    let _ = search_client.search(&request).await;
                    completed.fetch_add(1, Ordering::SeqCst);
                    progress_bar.inc(1);
                }
            })
            .collect();

        futures::future::join_all(futures).await;
        progress_bar.finish_with_message("Warmup completed");

        info!("Warmup completed: {} queries executed", total);
        Ok(())
    }

    /// Run the actual benchmark phase
    async fn run_benchmark_phase(
        &self,
        queries: &[Query],
        config: &LatencyBenchmarkConfig,
        search_mode: SearchMode,
    ) -> Result<(Vec<LatencyMeasurement>, Duration)> {
        let total = queries.len();
        let inter_query_delay = config.inter_query_delay_ms();

        let progress_bar = ProgressBar::new(total as u64);
        let mode_str = if inter_query_delay.is_some() {
            format!("rate-limited @ {:.1} QPS", config.target_qps)
        } else {
            format!("burst mode, concurrency={}", config.concurrency)
        };
        progress_bar.set_style(
            ProgressStyle::default_bar()
                .template(&format!(
                    "{{spinner:.green}} Benchmark [{{bar:40.cyan/blue}}] {{pos}}/{{len}} ({}) {{msg}}",
                    mode_str
                ))
                .unwrap()
                .progress_chars("#>-"),
        );

        let start_time = Instant::now();

        // Choose execution strategy based on rate limiting
        let results = if let Some(delay_ms) = inter_query_delay {
            // Rate-limited mode: execute queries at target QPS
            self.run_rate_limited(queries, config, search_mode, delay_ms, &progress_bar)
                .await
        } else {
            // Burst mode: execute all queries as fast as possible with concurrency limit
            self.run_burst_mode(queries, config, search_mode, &progress_bar)
                .await
        };

        let total_duration = start_time.elapsed();
        progress_bar.finish_with_message(format!("Done in {:.2}s", total_duration.as_secs_f64()));

        // Collect successful measurements and log failures
        let mut measurements = Vec::new();
        let mut failures = 0;

        for result in results {
            match result {
                Ok(measurement) => measurements.push(measurement),
                Err(e) => {
                    failures += 1;
                    warn!("Query failed: {}", e);
                }
            }
        }

        if failures > 0 {
            warn!("{} queries failed during benchmark", failures);
        }

        info!(
            "Benchmark phase completed: {} successful, {} failed, {:.2}s total",
            measurements.len(),
            failures,
            total_duration.as_secs_f64()
        );

        Ok((measurements, total_duration))
    }

    /// Run queries in burst mode with concurrency limit
    async fn run_burst_mode(
        &self,
        queries: &[Query],
        config: &LatencyBenchmarkConfig,
        search_mode: SearchMode,
        progress_bar: &ProgressBar,
    ) -> Vec<Result<LatencyMeasurement>> {
        let semaphore = Arc::new(Semaphore::new(config.concurrency));

        let futures: Vec<_> = queries
            .iter()
            .map(|query| {
                let semaphore = semaphore.clone();
                let search_client = &self.search_client;
                let search_mode = search_mode.clone();
                let timeout_ms = config.timeout_ms;
                let progress_bar = progress_bar.clone();
                let query = query.clone();

                async move {
                    let _permit = semaphore.acquire().await.unwrap();

                    let measurement = self
                        .execute_query_with_timing(search_client, &query, search_mode, timeout_ms)
                        .await;

                    progress_bar.inc(1);
                    if let Ok(ref m) = measurement {
                        progress_bar.set_message(format!("{:.1}ms", m.latency_ms));
                    }

                    measurement
                }
            })
            .collect();

        futures::future::join_all(futures).await
    }

    /// Run queries at a target QPS rate
    async fn run_rate_limited(
        &self,
        queries: &[Query],
        config: &LatencyBenchmarkConfig,
        search_mode: SearchMode,
        delay_ms: u64,
        progress_bar: &ProgressBar,
    ) -> Vec<Result<LatencyMeasurement>> {
        let mut results = Vec::with_capacity(queries.len());
        let mut interval = tokio::time::interval(Duration::from_millis(delay_ms));

        for query in queries {
            // Wait for next interval
            interval.tick().await;

            let measurement = self
                .execute_query_with_timing(
                    &self.search_client,
                    query,
                    search_mode.clone(),
                    config.timeout_ms,
                )
                .await;

            progress_bar.inc(1);
            if let Ok(ref m) = measurement {
                progress_bar.set_message(format!("{:.1}ms", m.latency_ms));
            }

            results.push(measurement);
        }

        results
    }

    /// Execute a single query and measure latency
    async fn execute_query_with_timing(
        &self,
        search_client: &OmniSearchClient,
        query: &Query,
        search_mode: SearchMode,
        timeout_ms: u64,
    ) -> Result<LatencyMeasurement> {
        let request = create_search_request(query.text.clone(), search_mode);

        let start = Instant::now();

        let result = tokio::time::timeout(
            Duration::from_millis(timeout_ms),
            search_client.search(&request),
        )
        .await;

        let latency_ms = start.elapsed().as_secs_f64() * 1000.0;
        let timestamp = Utc::now();

        match result {
            Ok(Ok(response)) => {
                debug!(
                    "Query '{}' completed in {:.2}ms with {} results",
                    query.id,
                    latency_ms,
                    response.results.len()
                );
                Ok(LatencyMeasurement {
                    query_id: query.id.clone(),
                    query_text: query.text.clone(),
                    latency_ms,
                    result_count: response.results.len(),
                    timestamp,
                    error: None,
                })
            }
            Ok(Err(e)) => {
                warn!("Query '{}' failed: {}", query.id, e);
                Ok(LatencyMeasurement {
                    query_id: query.id.clone(),
                    query_text: query.text.clone(),
                    latency_ms,
                    result_count: 0,
                    timestamp,
                    error: Some(e.to_string()),
                })
            }
            Err(_) => {
                warn!("Query '{}' timed out after {}ms", query.id, timeout_ms);
                Ok(LatencyMeasurement {
                    query_id: query.id.clone(),
                    query_text: query.text.clone(),
                    latency_ms: timeout_ms as f64,
                    result_count: 0,
                    timestamp,
                    error: Some(format!("Timeout after {}ms", timeout_ms)),
                })
            }
        }
    }

    /// Get system information for context
    async fn get_system_info(&self) -> Result<SystemInfo> {
        // Get document count
        let total_documents: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM documents")
            .fetch_one(&self.db_pool)
            .await
            .unwrap_or(0);

        // Get embedding count
        let total_embeddings: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM embeddings")
            .fetch_one(&self.db_pool)
            .await
            .unwrap_or(0);

        // Get index size (approximate)
        let index_size_bytes: Option<i64> = sqlx::query_scalar(
            "SELECT pg_total_relation_size('documents') + COALESCE(pg_total_relation_size('embeddings'), 0)",
        )
        .fetch_one(&self.db_pool)
        .await
        .ok();

        // Get postgres version
        let postgres_version: Option<String> = sqlx::query_scalar("SELECT version()")
            .fetch_one(&self.db_pool)
            .await
            .ok();

        Ok(SystemInfo {
            total_documents,
            total_embeddings,
            index_size_bytes,
            postgres_version,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_latency_benchmark_config_default() {
        let config = LatencyBenchmarkConfig::default();
        assert_eq!(config.num_queries, 1000);
        assert_eq!(config.concurrency, 10);
        assert_eq!(config.warmup_queries, 100);
        assert_eq!(config.timeout_ms, 30000);
        assert_eq!(config.target_qps, 0.0);
    }

    #[test]
    fn test_inter_query_delay() {
        let mut config = LatencyBenchmarkConfig::default();

        // Zero QPS = unlimited (no delay)
        config.target_qps = 0.0;
        assert_eq!(config.inter_query_delay_ms(), None);

        // 1 QPS = 1000ms delay
        config.target_qps = 1.0;
        assert_eq!(config.inter_query_delay_ms(), Some(1000));

        // 10 QPS = 100ms delay
        config.target_qps = 10.0;
        assert_eq!(config.inter_query_delay_ms(), Some(100));

        // 1.5 QPS = 666ms delay
        config.target_qps = 1.5;
        assert_eq!(config.inter_query_delay_ms(), Some(666));
    }
}

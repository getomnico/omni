use crate::config::BenchmarkConfig;
use crate::datasets::DatasetLoader;
use crate::datasets::Query;
use crate::evaluator::metrics::{
    BenchmarkConfigSummary, BenchmarkResult, EvaluationMetrics, LatencyCalculator,
    LatencyMeasurement, MetricsCalculator, QueryResult, RelevantDocument, RetrievedDocument,
};
use crate::search_client::{create_search_request, with_limit, with_offset, OmniSearchClient};
use anyhow::Result;
use chrono::Utc;
use futures::stream::{self, StreamExt};
use indicatif::{ProgressBar, ProgressStyle};
use omni_searcher::models::SearchMode;
use std::time::{Duration, Instant};
use tokio::time::sleep;
use tracing::{info, warn};

pub struct BenchmarkEvaluator {
    search_client: OmniSearchClient,
}

impl BenchmarkEvaluator {
    pub fn new(search_client: OmniSearchClient) -> Self {
        Self { search_client }
    }

    pub async fn run_benchmark(
        &self,
        dataset_loader: &dyn DatasetLoader,
        search_mode: &str,
        config: &BenchmarkConfig,
        warmup_queries: usize,
    ) -> Result<BenchmarkResult> {
        info!(
            "Starting benchmark evaluation for search mode: {}",
            search_mode
        );

        if !self.search_client.health_check().await? {
            return Err(anyhow::anyhow!("Search service is not healthy"));
        }

        let queries: Vec<Result<Query>> = dataset_loader.stream_queries().collect().await;
        info!("Loaded dataset with {} queries", queries.len());

        // Run warmup phase
        if warmup_queries > 0 {
            let warmup_count = warmup_queries.min(queries.len());
            info!("Running warmup phase ({} queries)...", warmup_count);
            let warmup_bar = ProgressBar::new(warmup_count as u64);
            warmup_bar.set_style(
                ProgressStyle::default_bar()
                    .template("{spinner:.yellow} Warmup [{bar:40.yellow}] {pos}/{len}")
                    .unwrap()
                    .progress_chars("=>-"),
            );

            let mode = Self::parse_search_mode(search_mode);
            for query_result in queries.iter().take(warmup_count) {
                if let Ok(q) = query_result {
                    let request = create_search_request(q.text.clone(), mode.clone());
                    let _ = self.search_client.search(&request).await;
                    warmup_bar.inc(1);
                }
            }
            warmup_bar.finish_with_message("Warmup completed");
        }

        let progress_bar = ProgressBar::new(queries.len() as u64);
        progress_bar.set_style(
            ProgressStyle::default_bar()
                .template(
                    "{spinner:.green} [{elapsed_precise}] [{bar:40.cyan/blue}] {pos}/{len} ({eta})",
                )
                .unwrap()
                .progress_chars("#>-"),
        );

        let benchmark_start = Instant::now();

        let query_results = stream::iter(queries.into_iter())
            .map(|query| {
                let search_client = &self.search_client;
                let search_mode = search_mode.to_string();
                let config = config.clone();
                let progress_bar = progress_bar.clone();

                async move {
                    match query {
                        Ok(q) => {
                            let result = self
                                .process_query(search_client, &q, &search_mode, &config)
                                .await;
                            progress_bar.inc(1);
                            result
                        }
                        Err(e) => {
                            warn!("Failed to load query: {}", e);
                            Err(e)
                        }
                    }
                }
            })
            .buffer_unordered(config.concurrent_queries)
            .collect::<Vec<_>>()
            .await;

        let total_duration_secs = benchmark_start.elapsed().as_secs_f64();
        progress_bar.finish_with_message("Benchmark completed");

        let mut successful_results = Vec::new();
        let mut latency_measurements = Vec::new();
        let mut failed_queries = 0;

        for result in query_results {
            match result {
                Ok((query_result, measurement)) => {
                    successful_results.push(query_result);
                    latency_measurements.push(measurement);
                }
                Err(e) => {
                    failed_queries += 1;
                    warn!("Query failed: {}", e);
                }
            }
        }

        if failed_queries > 0 {
            warn!("Failed to process {} queries", failed_queries);
        }

        info!(
            "Successfully processed {} queries",
            successful_results.len()
        );

        let query_metrics: Vec<EvaluationMetrics> = successful_results
            .iter()
            .map(|query_result| MetricsCalculator::calculate_metrics(query_result))
            .collect();

        let aggregated_metrics = MetricsCalculator::aggregate_metrics(
            query_metrics,
            dataset_loader.get_name(),
            search_mode.to_string(),
        );

        let latency_stats =
            LatencyCalculator::calculate_stats(&latency_measurements, total_duration_secs);

        info!("Benchmark evaluation completed successfully");

        Ok(BenchmarkResult {
            relevance: aggregated_metrics,
            latency: latency_stats,
            system_info: None,
            config_summary: BenchmarkConfigSummary {
                dataset_name: dataset_loader.get_name(),
                search_mode: search_mode.to_string(),
                total_queries: successful_results.len(),
                concurrent_queries: config.concurrent_queries,
                warmup_queries,
            },
            run_timestamp: Utc::now(),
        })
    }

    fn parse_search_mode(search_mode: &str) -> SearchMode {
        match search_mode.to_lowercase().as_str() {
            "fulltext" => SearchMode::Fulltext,
            "semantic" => SearchMode::Semantic,
            "hybrid" => SearchMode::Hybrid,
            _ => SearchMode::Fulltext,
        }
    }

    async fn process_query(
        &self,
        search_client: &OmniSearchClient,
        query: &crate::datasets::Query,
        search_mode: &str,
        config: &BenchmarkConfig,
    ) -> Result<(QueryResult, LatencyMeasurement)> {
        if config.rate_limit_delay_ms > 0 {
            sleep(Duration::from_millis(config.rate_limit_delay_ms)).await;
        }

        let mode = Self::parse_search_mode(search_mode);

        let search_request = create_search_request(query.text.clone(), mode);
        let search_request = with_limit(search_request, config.max_results_per_query);
        let search_request = with_offset(search_request, 0);

        let start = Instant::now();
        let search_response = search_client.search(&search_request).await?;
        let latency_ms = start.elapsed().as_secs_f64() * 1000.0;

        let result_count = search_response.results.len();

        let retrieved_docs: Vec<RetrievedDocument> = search_response
            .results
            .into_iter()
            .enumerate()
            .map(|(rank, result)| RetrievedDocument {
                doc_id: result.document.external_id,
                rank: rank + 1,
                score: result.score as f64,
            })
            .collect();

        let relevant_docs: Vec<RelevantDocument> = query
            .relevant_docs
            .iter()
            .map(|doc| RelevantDocument {
                doc_id: doc.doc_id.clone(),
                relevance_score: doc.relevance_score,
            })
            .collect();

        let query_result = QueryResult {
            query_id: query.id.clone(),
            query_text: query.text.clone(),
            retrieved_docs,
            relevant_docs,
        };

        let measurement = LatencyMeasurement {
            query_id: query.id.clone(),
            query_text: query.text.clone(),
            latency_ms,
            result_count,
            timestamp: Utc::now(),
            error: None,
        };

        Ok((query_result, measurement))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_benchmark_evaluator() {
        let client = OmniSearchClient::new("http://localhost:3001").unwrap();
        let _evaluator = BenchmarkEvaluator::new(client);
    }
}

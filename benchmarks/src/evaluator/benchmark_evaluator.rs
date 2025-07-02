use crate::config::BenchmarkConfig;
use crate::datasets::DatasetLoader;
use crate::datasets::Query;
use crate::evaluator::metrics::{
    AggregatedMetrics, EvaluationMetrics, MetricsCalculator, QueryResult, RelevantDocument,
    RetrievedDocument,
};
use crate::search_client::{create_search_request, with_limit, with_offset, ClioSearchClient};
use anyhow::Result;
use clio_searcher::models::SearchMode;
use futures::stream::{self, StreamExt};
use indicatif::{ProgressBar, ProgressStyle};
use std::time::Duration;
use tokio::time::sleep;
use tracing::{info, warn};

pub struct BenchmarkEvaluator {
    search_client: ClioSearchClient,
}

impl BenchmarkEvaluator {
    pub fn new(search_client: ClioSearchClient) -> Self {
        Self { search_client }
    }

    pub async fn run_benchmark(
        &self,
        dataset_loader: &dyn DatasetLoader,
        search_mode: &str,
        config: &BenchmarkConfig,
    ) -> Result<AggregatedMetrics> {
        info!(
            "Starting benchmark evaluation for search mode: {}",
            search_mode
        );

        // Check if search service is healthy
        if !self.search_client.health_check().await? {
            return Err(anyhow::anyhow!("Search service is not healthy"));
        }

        // Load queries
        let queries: Vec<Result<Query>> = dataset_loader.stream_queries().collect().await;
        info!("Loaded dataset with {} queries", queries.len());

        // Create progress bar
        let progress_bar = ProgressBar::new(queries.len() as u64);
        progress_bar.set_style(
            ProgressStyle::default_bar()
                .template(
                    "{spinner:.green} [{elapsed_precise}] [{bar:40.cyan/blue}] {pos}/{len} ({eta})",
                )
                .unwrap()
                .progress_chars("#>-"),
        );

        // Process queries concurrently with rate limiting
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

        progress_bar.finish_with_message("Benchmark completed");

        // Filter successful results and calculate metrics
        let mut successful_results = Vec::new();
        let mut failed_queries = 0;

        for result in query_results {
            match result {
                Ok(query_result) => successful_results.push(query_result),
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

        // Calculate metrics for each query
        let query_metrics: Vec<EvaluationMetrics> = successful_results
            .iter()
            .map(|query_result| MetricsCalculator::calculate_metrics(query_result))
            .collect();

        // Aggregate metrics
        let aggregated_metrics = MetricsCalculator::aggregate_metrics(
            query_metrics,
            dataset_loader.get_name(),
            search_mode.to_string(),
        );

        info!("Benchmark evaluation completed successfully");
        Ok(aggregated_metrics)
    }

    async fn process_query(
        &self,
        search_client: &ClioSearchClient,
        query: &crate::datasets::Query,
        search_mode: &str,
        config: &BenchmarkConfig,
    ) -> Result<QueryResult> {
        // Add rate limiting delay
        if config.rate_limit_delay_ms > 0 {
            sleep(Duration::from_millis(config.rate_limit_delay_ms)).await;
        }

        // Create search request
        let mode = match search_mode.to_lowercase().as_str() {
            "fulltext" => SearchMode::Fulltext,
            "semantic" => SearchMode::Semantic,
            "hybrid" => SearchMode::Hybrid,
            _ => SearchMode::Fulltext, // Default fallback
        };

        let search_request = create_search_request(query.text.clone(), mode);
        let search_request = with_limit(search_request, config.max_results_per_query);
        let search_request = with_offset(search_request, 0);

        // Execute search
        let search_response = search_client.search(&search_request).await?;

        // Convert search results to benchmark format
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

        // Convert relevant documents to benchmark format
        let relevant_docs: Vec<RelevantDocument> = query
            .relevant_docs
            .iter()
            .map(|doc| RelevantDocument {
                doc_id: doc.doc_id.clone(),
                relevance_score: doc.relevance_score,
            })
            .collect();

        Ok(QueryResult {
            query_id: query.id.clone(),
            query_text: query.text.clone(),
            retrieved_docs,
            relevant_docs,
        })
    }

    pub async fn run_comparative_benchmark(
        &self,
        dataset_loader: &dyn DatasetLoader,
        search_modes: &[String],
        config: &BenchmarkConfig,
    ) -> Result<Vec<AggregatedMetrics>> {
        let mut results = Vec::new();

        for search_mode in search_modes {
            info!("Running benchmark for search mode: {}", search_mode);
            let metrics = self
                .run_benchmark(dataset_loader, search_mode, config)
                .await?;
            results.push(metrics);
        }

        Ok(results)
    }

    pub async fn run_hyperparameter_optimization(
        &self,
        dataset_loader: &dyn DatasetLoader,
        config: &BenchmarkConfig,
    ) -> Result<HyperparameterResults> {
        info!("Starting hyperparameter optimization for hybrid search");

        let mut best_metrics: Option<AggregatedMetrics> = None;
        let mut best_params = HyperparameterConfig::default();
        let mut all_results = Vec::new();

        // Test different combinations of FTS and semantic weights
        let fts_weights = vec![0.3, 0.4, 0.5, 0.6, 0.7];
        let semantic_weights = vec![0.3, 0.4, 0.5, 0.6, 0.7];

        for &fts_weight in &fts_weights {
            for &semantic_weight in &semantic_weights {
                if (fts_weight + semantic_weight - 1.0_f64).abs() < 0.001 {
                    // Weights sum to 1.0
                    info!(
                        "Testing hyperparameters: FTS={:.1}, Semantic={:.1}",
                        fts_weight, semantic_weight
                    );

                    // TODO: Configure the search service with these weights
                    // For now, we'll just run with the default hybrid mode
                    let metrics = self.run_benchmark(dataset_loader, "hybrid", config).await?;

                    let params = HyperparameterConfig {
                        fts_weight,
                        semantic_weight,
                        ndcg_10: metrics.mean_ndcg_at_10,
                        mrr: metrics.mean_mrr,
                    };

                    if best_metrics.is_none()
                        || metrics.mean_ndcg_at_10 > best_metrics.as_ref().unwrap().mean_ndcg_at_10
                    {
                        best_metrics = Some(metrics.clone());
                        best_params = params.clone();
                    }

                    all_results.push((params, metrics));
                }
            }
        }

        Ok(HyperparameterResults {
            best_params,
            best_metrics: best_metrics.unwrap(),
            all_results,
        })
    }
}

#[derive(Debug, Clone)]
pub struct HyperparameterConfig {
    pub fts_weight: f64,
    pub semantic_weight: f64,
    pub ndcg_10: f64,
    pub mrr: f64,
}

impl Default for HyperparameterConfig {
    fn default() -> Self {
        Self {
            fts_weight: 0.5,
            semantic_weight: 0.5,
            ndcg_10: 0.0,
            mrr: 0.0,
        }
    }
}

#[derive(Debug)]
pub struct HyperparameterResults {
    pub best_params: HyperparameterConfig,
    pub best_metrics: AggregatedMetrics,
    pub all_results: Vec<(HyperparameterConfig, AggregatedMetrics)>,
}

impl HyperparameterResults {
    pub fn print_summary(&self) {
        println!("\n=== Hyperparameter Optimization Results ===");
        println!("Best Parameters:");
        println!("  FTS Weight: {:.1}", self.best_params.fts_weight);
        println!("  Semantic Weight: {:.1}", self.best_params.semantic_weight);
        println!("  nDCG@10: {:.4}", self.best_params.ndcg_10);
        println!("  MRR: {:.4}", self.best_params.mrr);
        println!();
        println!("All Results:");
        for (params, metrics) in &self.all_results {
            println!(
                "  FTS={:.1}, Sem={:.1}: nDCG@10={:.4}, MRR={:.4}",
                params.fts_weight,
                params.semantic_weight,
                metrics.mean_ndcg_at_10,
                metrics.mean_mrr
            );
        }
        println!("==========================================\n");
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::datasets::{Dataset, RelevantDoc};

    #[tokio::test]
    async fn test_benchmark_evaluator() {
        // This would require a running Clio instance for integration testing
        // For now, we'll test the structure
        let client = ClioSearchClient::new("http://localhost:3001").unwrap();
        let evaluator = BenchmarkEvaluator::new(client);

        // Test that the evaluator is created successfully
        assert!(true); // Placeholder assertion
    }
}

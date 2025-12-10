use anyhow::Result;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct QueryResult {
    pub query_id: String,
    pub query_text: String,
    pub retrieved_docs: Vec<RetrievedDocument>,
    pub relevant_docs: Vec<RelevantDocument>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RetrievedDocument {
    pub doc_id: String,
    pub rank: usize,
    pub score: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RelevantDocument {
    pub doc_id: String,
    pub relevance_score: f64, // 0.0 to 1.0, where 1.0 is most relevant
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EvaluationMetrics {
    pub query_id: String,
    pub ndcg_at_1: f64,
    pub ndcg_at_5: f64,
    pub ndcg_at_10: f64,
    pub ndcg_at_20: f64,
    pub mrr: f64,
    pub map_at_5: f64,
    pub map_at_10: f64,
    pub map_at_20: f64,
    pub precision_at_1: f64,
    pub precision_at_5: f64,
    pub precision_at_10: f64,
    pub precision_at_20: f64,
    pub recall_at_5: f64,
    pub recall_at_10: f64,
    pub recall_at_20: f64,
    pub num_relevant: usize,
    pub num_retrieved: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AggregatedMetrics {
    pub dataset_name: String,
    pub search_mode: String,
    pub total_queries: usize,
    pub mean_ndcg_at_1: f64,
    pub mean_ndcg_at_5: f64,
    pub mean_ndcg_at_10: f64,
    pub mean_ndcg_at_20: f64,
    pub mean_mrr: f64,
    pub mean_map_at_5: f64,
    pub mean_map_at_10: f64,
    pub mean_map_at_20: f64,
    pub mean_precision_at_1: f64,
    pub mean_precision_at_5: f64,
    pub mean_precision_at_10: f64,
    pub mean_precision_at_20: f64,
    pub mean_recall_at_5: f64,
    pub mean_recall_at_10: f64,
    pub mean_recall_at_20: f64,
    pub query_metrics: Vec<EvaluationMetrics>,
}

pub struct MetricsCalculator;

impl MetricsCalculator {
    pub fn calculate_metrics(query_result: &QueryResult) -> EvaluationMetrics {
        let relevant_docs_map: HashMap<String, f64> = query_result
            .relevant_docs
            .iter()
            .map(|doc| (doc.doc_id.clone(), doc.relevance_score))
            .collect();

        let retrieved_docs = &query_result.retrieved_docs;
        let num_relevant = query_result.relevant_docs.len();
        let num_retrieved = retrieved_docs.len();

        EvaluationMetrics {
            query_id: query_result.query_id.clone(),
            ndcg_at_1: Self::calculate_ndcg(&retrieved_docs, &relevant_docs_map, 1),
            ndcg_at_5: Self::calculate_ndcg(&retrieved_docs, &relevant_docs_map, 5),
            ndcg_at_10: Self::calculate_ndcg(&retrieved_docs, &relevant_docs_map, 10),
            ndcg_at_20: Self::calculate_ndcg(&retrieved_docs, &relevant_docs_map, 20),
            mrr: Self::calculate_mrr(&retrieved_docs, &relevant_docs_map),
            map_at_5: Self::calculate_map(&retrieved_docs, &relevant_docs_map, 5),
            map_at_10: Self::calculate_map(&retrieved_docs, &relevant_docs_map, 10),
            map_at_20: Self::calculate_map(&retrieved_docs, &relevant_docs_map, 20),
            precision_at_1: Self::calculate_precision(&retrieved_docs, &relevant_docs_map, 1),
            precision_at_5: Self::calculate_precision(&retrieved_docs, &relevant_docs_map, 5),
            precision_at_10: Self::calculate_precision(&retrieved_docs, &relevant_docs_map, 10),
            precision_at_20: Self::calculate_precision(&retrieved_docs, &relevant_docs_map, 20),
            recall_at_5: Self::calculate_recall(
                &retrieved_docs,
                &relevant_docs_map,
                5,
                num_relevant,
            ),
            recall_at_10: Self::calculate_recall(
                &retrieved_docs,
                &relevant_docs_map,
                10,
                num_relevant,
            ),
            recall_at_20: Self::calculate_recall(
                &retrieved_docs,
                &relevant_docs_map,
                20,
                num_relevant,
            ),
            num_relevant,
            num_retrieved,
        }
    }

    pub fn aggregate_metrics(
        query_metrics: Vec<EvaluationMetrics>,
        dataset_name: String,
        search_mode: String,
    ) -> AggregatedMetrics {
        let total_queries = query_metrics.len();

        if total_queries == 0 {
            return AggregatedMetrics {
                dataset_name,
                search_mode,
                total_queries: 0,
                mean_ndcg_at_1: 0.0,
                mean_ndcg_at_5: 0.0,
                mean_ndcg_at_10: 0.0,
                mean_ndcg_at_20: 0.0,
                mean_mrr: 0.0,
                mean_map_at_5: 0.0,
                mean_map_at_10: 0.0,
                mean_map_at_20: 0.0,
                mean_precision_at_1: 0.0,
                mean_precision_at_5: 0.0,
                mean_precision_at_10: 0.0,
                mean_precision_at_20: 0.0,
                mean_recall_at_5: 0.0,
                mean_recall_at_10: 0.0,
                mean_recall_at_20: 0.0,
                query_metrics,
            };
        }

        let total_queries_f = total_queries as f64;

        AggregatedMetrics {
            dataset_name,
            search_mode,
            total_queries,
            mean_ndcg_at_1: query_metrics.iter().map(|m| m.ndcg_at_1).sum::<f64>()
                / total_queries_f,
            mean_ndcg_at_5: query_metrics.iter().map(|m| m.ndcg_at_5).sum::<f64>()
                / total_queries_f,
            mean_ndcg_at_10: query_metrics.iter().map(|m| m.ndcg_at_10).sum::<f64>()
                / total_queries_f,
            mean_ndcg_at_20: query_metrics.iter().map(|m| m.ndcg_at_20).sum::<f64>()
                / total_queries_f,
            mean_mrr: query_metrics.iter().map(|m| m.mrr).sum::<f64>() / total_queries_f,
            mean_map_at_5: query_metrics.iter().map(|m| m.map_at_5).sum::<f64>() / total_queries_f,
            mean_map_at_10: query_metrics.iter().map(|m| m.map_at_10).sum::<f64>()
                / total_queries_f,
            mean_map_at_20: query_metrics.iter().map(|m| m.map_at_20).sum::<f64>()
                / total_queries_f,
            mean_precision_at_1: query_metrics.iter().map(|m| m.precision_at_1).sum::<f64>()
                / total_queries_f,
            mean_precision_at_5: query_metrics.iter().map(|m| m.precision_at_5).sum::<f64>()
                / total_queries_f,
            mean_precision_at_10: query_metrics.iter().map(|m| m.precision_at_10).sum::<f64>()
                / total_queries_f,
            mean_precision_at_20: query_metrics.iter().map(|m| m.precision_at_20).sum::<f64>()
                / total_queries_f,
            mean_recall_at_5: query_metrics.iter().map(|m| m.recall_at_5).sum::<f64>()
                / total_queries_f,
            mean_recall_at_10: query_metrics.iter().map(|m| m.recall_at_10).sum::<f64>()
                / total_queries_f,
            mean_recall_at_20: query_metrics.iter().map(|m| m.recall_at_20).sum::<f64>()
                / total_queries_f,
            query_metrics,
        }
    }

    fn calculate_ndcg(
        retrieved_docs: &[RetrievedDocument],
        relevant_docs: &HashMap<String, f64>,
        k: usize,
    ) -> f64 {
        let k = k.min(retrieved_docs.len());
        if k == 0 {
            return 0.0;
        }

        // Calculate DCG@k
        let mut dcg = 0.0;
        for (i, doc) in retrieved_docs.iter().take(k).enumerate() {
            if let Some(&relevance) = relevant_docs.get(&doc.doc_id) {
                let gain = relevance;
                let discount = if i == 0 { 1.0 } else { (1.0 + i as f64).log2() };
                dcg += gain / discount;
            }
        }

        // Calculate IDCG@k (Ideal DCG)
        let mut ideal_relevances: Vec<f64> = relevant_docs.values().cloned().collect();
        ideal_relevances.sort_by(|a, b| b.partial_cmp(a).unwrap());

        let mut idcg = 0.0;
        for (i, &relevance) in ideal_relevances.iter().take(k).enumerate() {
            let gain = relevance;
            let discount = if i == 0 { 1.0 } else { (1.0 + i as f64).log2() };
            idcg += gain / discount;
        }

        if idcg == 0.0 {
            0.0
        } else {
            dcg / idcg
        }
    }

    fn calculate_mrr(
        retrieved_docs: &[RetrievedDocument],
        relevant_docs: &HashMap<String, f64>,
    ) -> f64 {
        for (i, doc) in retrieved_docs.iter().enumerate() {
            if relevant_docs.contains_key(&doc.doc_id) {
                return 1.0 / (i + 1) as f64;
            }
        }
        0.0
    }

    fn calculate_map(
        retrieved_docs: &[RetrievedDocument],
        relevant_docs: &HashMap<String, f64>,
        k: usize,
    ) -> f64 {
        let k = k.min(retrieved_docs.len());
        if k == 0 {
            return 0.0;
        }

        let mut sum_precision = 0.0;
        let mut num_relevant_found = 0;

        for (i, doc) in retrieved_docs.iter().take(k).enumerate() {
            if relevant_docs.contains_key(&doc.doc_id) {
                num_relevant_found += 1;
                let precision_at_i = num_relevant_found as f64 / (i + 1) as f64;
                sum_precision += precision_at_i;
            }
        }

        if num_relevant_found == 0 {
            0.0
        } else {
            sum_precision / num_relevant_found as f64
        }
    }

    fn calculate_precision(
        retrieved_docs: &[RetrievedDocument],
        relevant_docs: &HashMap<String, f64>,
        k: usize,
    ) -> f64 {
        let k = k.min(retrieved_docs.len());
        if k == 0 {
            return 0.0;
        }

        let relevant_in_top_k = retrieved_docs
            .iter()
            .take(k)
            .filter(|doc| relevant_docs.contains_key(&doc.doc_id))
            .count();

        relevant_in_top_k as f64 / k as f64
    }

    fn calculate_recall(
        retrieved_docs: &[RetrievedDocument],
        relevant_docs: &HashMap<String, f64>,
        k: usize,
        total_relevant: usize,
    ) -> f64 {
        if total_relevant == 0 {
            return 0.0;
        }

        let k = k.min(retrieved_docs.len());
        let relevant_in_top_k = retrieved_docs
            .iter()
            .take(k)
            .filter(|doc| relevant_docs.contains_key(&doc.doc_id))
            .count();

        relevant_in_top_k as f64 / total_relevant as f64
    }
}

impl AggregatedMetrics {
    pub fn save_to_file(&self, path: &str) -> Result<()> {
        let json = serde_json::to_string_pretty(self)?;
        // Frist check if path exists, create if it doesn't
        let base_dir = std::path::Path::new(path).parent().unwrap();
        if !base_dir.exists() {
            std::fs::create_dir_all(base_dir)?;
        }
        std::fs::write(path, json)?;
        Ok(())
    }

    pub fn print_summary(&self) {
        println!("\n=== Benchmark Results Summary ===");
        println!("Dataset: {}", self.dataset_name);
        println!("Search Mode: {}", self.search_mode);
        println!("Total Queries: {}", self.total_queries);
        println!();
        println!("nDCG Metrics:");
        println!("  nDCG@1:  {:.4}", self.mean_ndcg_at_1);
        println!("  nDCG@5:  {:.4}", self.mean_ndcg_at_5);
        println!("  nDCG@10: {:.4}", self.mean_ndcg_at_10);
        println!("  nDCG@20: {:.4}", self.mean_ndcg_at_20);
        println!();
        println!("Other Metrics:");
        println!("  MRR:     {:.4}", self.mean_mrr);
        println!("  MAP@10:  {:.4}", self.mean_map_at_10);
        println!("  P@1:     {:.4}", self.mean_precision_at_1);
        println!("  P@5:     {:.4}", self.mean_precision_at_5);
        println!("  P@10:    {:.4}", self.mean_precision_at_10);
        println!("  R@10:    {:.4}", self.mean_recall_at_10);
        println!("================================\n");
    }
}

// ============================================================================
// Latency Metrics
// ============================================================================

/// A single latency measurement for one query
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LatencyMeasurement {
    pub query_id: String,
    pub query_text: String,
    pub latency_ms: f64,
    pub result_count: usize,
    pub timestamp: DateTime<Utc>,
    pub error: Option<String>,
}

/// Aggregated latency statistics
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LatencyStats {
    pub count: usize,
    pub successful: usize,
    pub failed: usize,
    pub min_ms: f64,
    pub max_ms: f64,
    pub mean_ms: f64,
    pub median_ms: f64, // p50
    pub p95_ms: f64,
    pub p99_ms: f64,
    pub std_dev_ms: f64,
    pub total_duration_secs: f64,
    pub throughput_qps: f64,
}

impl Default for LatencyStats {
    fn default() -> Self {
        Self {
            count: 0,
            successful: 0,
            failed: 0,
            min_ms: 0.0,
            max_ms: 0.0,
            mean_ms: 0.0,
            median_ms: 0.0,
            p95_ms: 0.0,
            p99_ms: 0.0,
            std_dev_ms: 0.0,
            total_duration_secs: 0.0,
            throughput_qps: 0.0,
        }
    }
}

/// System information for context
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SystemInfo {
    pub total_documents: i64,
    pub total_embeddings: i64,
    pub index_size_bytes: Option<i64>,
    pub postgres_version: Option<String>,
}

/// Complete latency benchmark result
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LatencyBenchmarkResult {
    pub dataset_name: String,
    pub search_mode: String,
    pub config: LatencyBenchmarkConfig,
    pub latency_stats: LatencyStats,
    pub measurements: Vec<LatencyMeasurement>,
    pub system_info: SystemInfo,
    pub run_timestamp: DateTime<Utc>,
}

/// Configuration for latency benchmark
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LatencyBenchmarkConfig {
    pub num_queries: usize,
    pub concurrency: usize,
    pub warmup_queries: usize,
    pub timeout_ms: u64,
    /// Target queries per second. 0 = unlimited (burst mode)
    pub target_qps: f64,
}

impl Default for LatencyBenchmarkConfig {
    fn default() -> Self {
        Self {
            num_queries: 1000,
            concurrency: 10,
            warmup_queries: 100,
            timeout_ms: 30000,
            target_qps: 0.0, // unlimited by default
        }
    }
}

impl LatencyBenchmarkConfig {
    /// Calculate inter-query delay in milliseconds based on target QPS
    pub fn inter_query_delay_ms(&self) -> Option<u64> {
        if self.target_qps <= 0.0 {
            None
        } else {
            Some((1000.0 / self.target_qps) as u64)
        }
    }
}

pub struct LatencyCalculator;

impl LatencyCalculator {
    /// Calculate latency statistics from measurements
    pub fn calculate_stats(
        measurements: &[LatencyMeasurement],
        total_duration_secs: f64,
    ) -> LatencyStats {
        if measurements.is_empty() {
            return LatencyStats::default();
        }

        // Collect successful latencies
        let mut latencies: Vec<f64> = measurements
            .iter()
            .filter(|m| m.error.is_none())
            .map(|m| m.latency_ms)
            .collect();

        let successful = latencies.len();
        let failed = measurements.len() - successful;

        if latencies.is_empty() {
            return LatencyStats {
                count: measurements.len(),
                successful: 0,
                failed,
                total_duration_secs,
                throughput_qps: 0.0,
                ..Default::default()
            };
        }

        // Sort for percentile calculations
        latencies.sort_by(|a, b| a.partial_cmp(b).unwrap());

        let min_ms = latencies[0];
        let max_ms = latencies[latencies.len() - 1];
        let mean_ms = latencies.iter().sum::<f64>() / latencies.len() as f64;
        let median_ms = Self::percentile(&latencies, 50.0);
        let p95_ms = Self::percentile(&latencies, 95.0);
        let p99_ms = Self::percentile(&latencies, 99.0);

        // Calculate standard deviation
        let variance =
            latencies.iter().map(|x| (x - mean_ms).powi(2)).sum::<f64>() / latencies.len() as f64;
        let std_dev_ms = variance.sqrt();

        // Calculate throughput
        let throughput_qps = if total_duration_secs > 0.0 {
            successful as f64 / total_duration_secs
        } else {
            0.0
        };

        LatencyStats {
            count: measurements.len(),
            successful,
            failed,
            min_ms,
            max_ms,
            mean_ms,
            median_ms,
            p95_ms,
            p99_ms,
            std_dev_ms,
            total_duration_secs,
            throughput_qps,
        }
    }

    /// Calculate a percentile value from sorted data
    fn percentile(sorted_data: &[f64], percentile: f64) -> f64 {
        if sorted_data.is_empty() {
            return 0.0;
        }
        if sorted_data.len() == 1 {
            return sorted_data[0];
        }

        let index = (percentile / 100.0) * (sorted_data.len() - 1) as f64;
        let lower = index.floor() as usize;
        let upper = index.ceil() as usize;

        if lower == upper {
            sorted_data[lower]
        } else {
            let weight = index - lower as f64;
            sorted_data[lower] * (1.0 - weight) + sorted_data[upper] * weight
        }
    }
}

impl LatencyBenchmarkResult {
    pub fn save_to_file(&self, path: &str) -> Result<()> {
        let json = serde_json::to_string_pretty(self)?;
        let base_dir = std::path::Path::new(path).parent().unwrap();
        if !base_dir.exists() {
            std::fs::create_dir_all(base_dir)?;
        }
        std::fs::write(path, json)?;
        Ok(())
    }

    pub fn print_summary(&self) {
        println!("\n=== Latency Benchmark Results ===");
        println!("Dataset: {}", self.dataset_name);
        println!("Search Mode: {}", self.search_mode);
        println!("Timestamp: {}", self.run_timestamp);
        println!();
        println!("Configuration:");
        println!("  Queries: {}", self.config.num_queries);
        println!("  Concurrency: {}", self.config.concurrency);
        println!("  Warmup: {}", self.config.warmup_queries);
        println!();
        println!("Query Statistics:");
        println!("  Total: {}", self.latency_stats.count);
        println!("  Successful: {}", self.latency_stats.successful);
        println!("  Failed: {}", self.latency_stats.failed);
        println!();
        println!("Latency (ms):");
        println!("  Min:    {:.2}", self.latency_stats.min_ms);
        println!("  Max:    {:.2}", self.latency_stats.max_ms);
        println!("  Mean:   {:.2}", self.latency_stats.mean_ms);
        println!("  Median: {:.2} (p50)", self.latency_stats.median_ms);
        println!("  p95:    {:.2}", self.latency_stats.p95_ms);
        println!("  p99:    {:.2}", self.latency_stats.p99_ms);
        println!("  StdDev: {:.2}", self.latency_stats.std_dev_ms);
        println!();
        println!("Throughput:");
        println!(
            "  Total Duration: {:.2}s",
            self.latency_stats.total_duration_secs
        );
        println!("  QPS: {:.2}", self.latency_stats.throughput_qps);
        println!();
        println!("System Info:");
        println!("  Documents: {}", self.system_info.total_documents);
        println!("  Embeddings: {}", self.system_info.total_embeddings);
        if let Some(size) = self.system_info.index_size_bytes {
            println!("  Index Size: {:.2} MB", size as f64 / 1024.0 / 1024.0);
        }
        println!("=================================\n");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_calculate_ndcg() {
        let retrieved_docs = vec![
            RetrievedDocument {
                doc_id: "doc1".to_string(),
                rank: 1,
                score: 0.9,
            },
            RetrievedDocument {
                doc_id: "doc2".to_string(),
                rank: 2,
                score: 0.8,
            },
            RetrievedDocument {
                doc_id: "doc3".to_string(),
                rank: 3,
                score: 0.7,
            },
        ];

        let mut relevant_docs = HashMap::new();
        relevant_docs.insert("doc1".to_string(), 1.0);
        relevant_docs.insert("doc3".to_string(), 0.5);

        let ndcg = MetricsCalculator::calculate_ndcg(&retrieved_docs, &relevant_docs, 3);
        assert!(ndcg > 0.0 && ndcg <= 1.0);
    }

    #[test]
    fn test_calculate_mrr() {
        let retrieved_docs = vec![
            RetrievedDocument {
                doc_id: "doc1".to_string(),
                rank: 1,
                score: 0.9,
            },
            RetrievedDocument {
                doc_id: "doc2".to_string(),
                rank: 2,
                score: 0.8,
            },
            RetrievedDocument {
                doc_id: "doc3".to_string(),
                rank: 3,
                score: 0.7,
            },
        ];

        let mut relevant_docs = HashMap::new();
        relevant_docs.insert("doc2".to_string(), 1.0);

        let mrr = MetricsCalculator::calculate_mrr(&retrieved_docs, &relevant_docs);
        assert_eq!(mrr, 0.5); // First relevant doc at rank 2, so MRR = 1/2 = 0.5
    }

    #[test]
    fn test_calculate_precision() {
        let retrieved_docs = vec![
            RetrievedDocument {
                doc_id: "doc1".to_string(),
                rank: 1,
                score: 0.9,
            },
            RetrievedDocument {
                doc_id: "doc2".to_string(),
                rank: 2,
                score: 0.8,
            },
            RetrievedDocument {
                doc_id: "doc3".to_string(),
                rank: 3,
                score: 0.7,
            },
        ];

        let mut relevant_docs = HashMap::new();
        relevant_docs.insert("doc1".to_string(), 1.0);
        relevant_docs.insert("doc3".to_string(), 1.0);

        let precision = MetricsCalculator::calculate_precision(&retrieved_docs, &relevant_docs, 3);
        assert_eq!(precision, 2.0 / 3.0); // 2 relevant docs out of 3 retrieved
    }

    #[test]
    fn test_latency_stats_calculation() {
        let measurements = vec![
            LatencyMeasurement {
                query_id: "q1".to_string(),
                query_text: "test".to_string(),
                latency_ms: 10.0,
                result_count: 5,
                timestamp: Utc::now(),
                error: None,
            },
            LatencyMeasurement {
                query_id: "q2".to_string(),
                query_text: "test2".to_string(),
                latency_ms: 20.0,
                result_count: 3,
                timestamp: Utc::now(),
                error: None,
            },
            LatencyMeasurement {
                query_id: "q3".to_string(),
                query_text: "test3".to_string(),
                latency_ms: 30.0,
                result_count: 7,
                timestamp: Utc::now(),
                error: None,
            },
        ];

        let stats = LatencyCalculator::calculate_stats(&measurements, 1.0);

        assert_eq!(stats.count, 3);
        assert_eq!(stats.successful, 3);
        assert_eq!(stats.failed, 0);
        assert_eq!(stats.min_ms, 10.0);
        assert_eq!(stats.max_ms, 30.0);
        assert_eq!(stats.mean_ms, 20.0);
        assert_eq!(stats.median_ms, 20.0);
        assert_eq!(stats.throughput_qps, 3.0);
    }

    #[test]
    fn test_latency_percentiles() {
        // Create 100 measurements with values 1-100
        let measurements: Vec<LatencyMeasurement> = (1..=100)
            .map(|i| LatencyMeasurement {
                query_id: format!("q{}", i),
                query_text: format!("test{}", i),
                latency_ms: i as f64,
                result_count: 1,
                timestamp: Utc::now(),
                error: None,
            })
            .collect();

        let stats = LatencyCalculator::calculate_stats(&measurements, 10.0);

        assert_eq!(stats.min_ms, 1.0);
        assert_eq!(stats.max_ms, 100.0);
        assert!((stats.median_ms - 50.5).abs() < 0.01);
        assert!((stats.p95_ms - 95.05).abs() < 0.1);
        assert!((stats.p99_ms - 99.01).abs() < 0.1);
    }

    #[test]
    fn test_latency_with_errors() {
        let measurements = vec![
            LatencyMeasurement {
                query_id: "q1".to_string(),
                query_text: "test".to_string(),
                latency_ms: 10.0,
                result_count: 5,
                timestamp: Utc::now(),
                error: None,
            },
            LatencyMeasurement {
                query_id: "q2".to_string(),
                query_text: "test2".to_string(),
                latency_ms: 0.0,
                result_count: 0,
                timestamp: Utc::now(),
                error: Some("timeout".to_string()),
            },
        ];

        let stats = LatencyCalculator::calculate_stats(&measurements, 1.0);

        assert_eq!(stats.count, 2);
        assert_eq!(stats.successful, 1);
        assert_eq!(stats.failed, 1);
        assert_eq!(stats.min_ms, 10.0);
        assert_eq!(stats.max_ms, 10.0);
    }
}

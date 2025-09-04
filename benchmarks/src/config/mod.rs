use anyhow::Result;
use serde::{Deserialize, Serialize};
use std::fs;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BenchmarkConfig {
    pub searcher_url: String,
    pub indexer_url: String,
    pub database_url: String,
    pub redis_url: String,
    pub max_results_per_query: i64,
    pub concurrent_queries: usize,
    pub rate_limit_delay_ms: u64,
    pub timeout_seconds: u64,
    pub use_separate_db: bool,
    pub reset_db_on_start: bool,
    pub index_documents_before_search: bool,
    pub datasets: DatasetsConfig,
    pub evaluation: EvaluationConfig,
    pub hyperparameter_optimization: HyperparameterConfig,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DatasetsConfig {
    pub beir: BeirConfig,
    pub msmarco: MsMarcoConfig,
    pub custom: CustomDatasetConfig,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BeirConfig {
    pub datasets: Vec<String>,
    pub download_url_base: String,
    pub cache_dir: String,
    pub selected_dataset: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MsMarcoConfig {
    pub dataset_type: String, // "passage" or "document"
    pub download_urls: MsMarcoUrls,
    pub cache_dir: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MsMarcoUrls {
    pub queries: String,
    pub corpus: String,
    pub qrels: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CustomDatasetConfig {
    pub data_dir: String,
    pub generate_synthetic: bool,
    pub num_synthetic_queries: usize,
    pub enterprise_domains: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EvaluationConfig {
    pub metrics: Vec<String>,
    pub cutoff_values: Vec<usize>,
    pub statistical_tests: bool,
    pub significance_level: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HyperparameterConfig {
    pub enable_optimization: bool,
    pub fts_weight_range: (f64, f64),
    pub semantic_weight_range: (f64, f64),
    pub weight_step: f64,
    pub optimization_metric: String, // "ndcg@10", "mrr", etc.
}

impl Default for BenchmarkConfig {
    fn default() -> Self {
        Self {
            searcher_url: "http://localhost:3001".to_string(),
            indexer_url: "http://localhost:3002".to_string(),
            database_url: "postgresql://postgres:password@localhost:5432/omni_benchmark"
                .to_string(),
            redis_url: "redis://localhost:6379".to_string(),
            max_results_per_query: 100,
            concurrent_queries: 5,
            rate_limit_delay_ms: 100,
            timeout_seconds: 30,
            use_separate_db: true,
            reset_db_on_start: true,
            index_documents_before_search: true,
            datasets: DatasetsConfig::default(),
            evaluation: EvaluationConfig::default(),
            hyperparameter_optimization: HyperparameterConfig::default(),
        }
    }
}

impl Default for DatasetsConfig {
    fn default() -> Self {
        Self {
            beir: BeirConfig::default(),
            msmarco: MsMarcoConfig::default(),
            custom: CustomDatasetConfig::default(),
        }
    }
}

impl Default for BeirConfig {
    fn default() -> Self {
        Self {
            datasets: vec![
                "nfcorpus".to_string(),
                "fiqa".to_string(),
                "trec-covid".to_string(),
                "scifact".to_string(),
                "scidocs".to_string(),
                "nq".to_string(),
                "hotpotqa".to_string(),
                "climate-fever".to_string(),
                "fever".to_string(),
                "dbpedia-entity".to_string(),
                "webis-touche2020".to_string(),
                "quora".to_string(),
            ],
            download_url_base: "https://public.ukp.informatik.tu-darmstadt.de/thakur/BEIR/datasets"
                .to_string(),
            cache_dir: "benchmarks/data/beir".to_string(),
            selected_dataset: None,
        }
    }
}

impl Default for MsMarcoConfig {
    fn default() -> Self {
        Self {
            dataset_type: "passage".to_string(),
            download_urls: MsMarcoUrls {
                queries: "https://msmarco.blob.core.windows.net/msmarcoranking/queries.tar.gz"
                    .to_string(),
                corpus: "https://msmarco.blob.core.windows.net/msmarcoranking/collection.tar.gz"
                    .to_string(),
                qrels: "https://msmarco.blob.core.windows.net/msmarcoranking/qrels.dev.small.tsv"
                    .to_string(),
            },
            cache_dir: "benchmarks/data/msmarco".to_string(),
        }
    }
}

impl Default for CustomDatasetConfig {
    fn default() -> Self {
        Self {
            data_dir: "benchmarks/data/custom".to_string(),
            generate_synthetic: false,
            num_synthetic_queries: 100,
            enterprise_domains: vec![
                "google_drive".to_string(),
                "slack".to_string(),
                "confluence".to_string(),
                "github".to_string(),
            ],
        }
    }
}

impl Default for EvaluationConfig {
    fn default() -> Self {
        Self {
            metrics: vec![
                "ndcg".to_string(),
                "mrr".to_string(),
                "map".to_string(),
                "precision".to_string(),
                "recall".to_string(),
            ],
            cutoff_values: vec![1, 5, 10, 20],
            statistical_tests: true,
            significance_level: 0.05,
        }
    }
}

impl Default for HyperparameterConfig {
    fn default() -> Self {
        Self {
            enable_optimization: false,
            fts_weight_range: (0.1, 0.9),
            semantic_weight_range: (0.1, 0.9),
            weight_step: 0.1,
            optimization_metric: "ndcg@10".to_string(),
        }
    }
}

impl BenchmarkConfig {
    pub fn from_file(path: &str) -> Result<Self> {
        let contents = fs::read_to_string(path)?;
        let config: BenchmarkConfig = toml::from_str(&contents)?;
        Ok(config)
    }

    pub fn from_env() -> Result<Self> {
        let mut config = Self::default();

        // Override with environment variables if present
        if let Ok(url) = std::env::var("CLIO_SEARCHER_URL") {
            config.searcher_url = url;
        }

        if let Ok(max_results) = std::env::var("BENCHMARK_MAX_RESULTS") {
            config.max_results_per_query =
                max_results.parse().unwrap_or(config.max_results_per_query);
        }

        if let Ok(concurrent) = std::env::var("BENCHMARK_CONCURRENT_QUERIES") {
            config.concurrent_queries = concurrent.parse().unwrap_or(config.concurrent_queries);
        }

        if let Ok(delay) = std::env::var("BENCHMARK_RATE_LIMIT_MS") {
            config.rate_limit_delay_ms = delay.parse().unwrap_or(config.rate_limit_delay_ms);
        }

        Ok(config)
    }

    pub fn save_to_file(&self, path: &str) -> Result<()> {
        let contents = toml::to_string_pretty(self)?;
        fs::write(path, contents)?;
        Ok(())
    }

    pub fn validate(&self) -> Result<()> {
        if self.searcher_url.is_empty() {
            return Err(anyhow::anyhow!("searcher_url cannot be empty"));
        }

        if self.max_results_per_query <= 0 {
            return Err(anyhow::anyhow!("max_results_per_query must be positive"));
        }

        if self.concurrent_queries == 0 {
            return Err(anyhow::anyhow!("concurrent_queries must be positive"));
        }

        if self.evaluation.cutoff_values.is_empty() {
            return Err(anyhow::anyhow!("cutoff_values cannot be empty"));
        }

        if self.hyperparameter_optimization.enable_optimization {
            let (min_fts, max_fts) = self.hyperparameter_optimization.fts_weight_range;
            let (min_sem, max_sem) = self.hyperparameter_optimization.semantic_weight_range;

            if min_fts >= max_fts || min_sem >= max_sem {
                return Err(anyhow::anyhow!(
                    "Invalid weight ranges for hyperparameter optimization"
                ));
            }

            if self.hyperparameter_optimization.weight_step <= 0.0 {
                return Err(anyhow::anyhow!("weight_step must be positive"));
            }
        }

        Ok(())
    }
}

// Add toml dependency to Cargo.toml

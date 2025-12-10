use anyhow::Result;
use clap::{Parser, Subcommand};
use futures::StreamExt;
use omni_searcher::models::SearchMode;
use tracing::{info, warn};

mod config;
mod datasets;
mod evaluator;
mod indexer;
mod prepare_nq;
mod reporter;
mod search_client;

use config::BenchmarkConfig;
use datasets::{BeirDataset, DatasetLoader, MsMarcoDataset, NaturalQuestionsDataset};
use evaluator::{BenchmarkEvaluator, LatencyBenchmarkConfig, LatencyBenchmarkEvaluator};
use indexer::BenchmarkIndexer;
use reporter::BenchmarkReporter;
use search_client::OmniSearchClient;

#[derive(Parser)]
#[command(author, version, about, long_about = None)]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    /// Download and prepare benchmark datasets
    Setup {
        /// Dataset to download (beir, msmarco, all)
        #[arg(short, long, default_value = "all")]
        dataset: String,
    },
    /// Run benchmark evaluation
    Run {
        /// Configuration file path
        #[arg(short, long, default_value = "benchmarks/config/default.toml")]
        config: String,
        /// Dataset to benchmark against
        #[arg(short, long, default_value = "beir")]
        dataset: String,
        /// Search mode to test (fulltext, semantic, hybrid, all)
        #[arg(short, long, default_value = "all")]
        search_mode: String,
    },
    /// Generate benchmark report
    Report {
        /// Results directory
        #[arg(short, long, default_value = "benchmarks/results")]
        results_dir: String,
        /// Output format (json, html, csv)
        #[arg(short, long, default_value = "html")]
        format: String,
    },
    /// Run latency performance benchmark
    Latency {
        /// Configuration file path
        #[arg(short, long, default_value = "benchmarks/config/latency.toml")]
        config: String,
        /// Dataset directory (prepared NQ data)
        #[arg(long, default_value = "benchmarks/data/nq_benchmark")]
        data_dir: String,
        /// Number of queries to run
        #[arg(long, default_value = "1000")]
        num_queries: usize,
        /// Concurrent query execution (only used in burst mode)
        #[arg(long, default_value = "10")]
        concurrency: usize,
        /// Warmup queries (not measured)
        #[arg(long, default_value = "100")]
        warmup: usize,
        /// Skip indexing (use existing data)
        #[arg(long)]
        skip_indexing: bool,
        /// Maximum documents to index
        #[arg(long)]
        max_documents: Option<usize>,
        /// Target queries per second (0 = unlimited burst mode)
        #[arg(long, default_value = "0")]
        target_qps: f64,
        /// Clean up benchmark database after run
        #[arg(long)]
        cleanup: bool,
    },
    /// Prepare Natural Questions dataset (fast Rust implementation)
    PrepareNq {
        /// Input directory containing raw NQ JSONL.gz files
        #[arg(long, default_value = "benchmarks/data/v1.0")]
        input_dir: String,
        /// Output directory for prepared data
        #[arg(long, default_value = "benchmarks/data/nq_benchmark")]
        output_dir: String,
        /// Maximum number of documents to extract
        #[arg(long)]
        max_documents: Option<usize>,
        /// Maximum number of queries to extract
        #[arg(long)]
        max_queries: Option<usize>,
    },
}

#[tokio::main]
async fn main() -> Result<()> {
    // Initialize tracing
    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        .init();

    let cli = Cli::parse();

    match &cli.command {
        Commands::Setup { dataset } => {
            info!("Setting up benchmark datasets: {}", dataset);
            setup_datasets(dataset).await?;
        }
        Commands::Run {
            config,
            dataset,
            search_mode,
        } => {
            info!(
                "Running benchmarks with config: {}, dataset: {}, mode: {}",
                config, dataset, search_mode
            );
            run_benchmarks(config, dataset, search_mode).await?;
        }
        Commands::Report {
            results_dir,
            format,
        } => {
            info!(
                "Generating report from: {} in format: {}",
                results_dir, format
            );
            generate_report(results_dir, format).await?;
        }
        Commands::Latency {
            config,
            data_dir,
            num_queries,
            concurrency,
            warmup,
            skip_indexing,
            max_documents,
            target_qps,
            cleanup,
        } => {
            info!(
                "Running latency benchmark: queries={}, concurrency={}, warmup={}, target_qps={}",
                num_queries, concurrency, warmup, target_qps
            );
            run_latency_benchmark(
                config,
                data_dir,
                *num_queries,
                *concurrency,
                *warmup,
                *skip_indexing,
                *max_documents,
                *target_qps,
                *cleanup,
            )
            .await?;
        }
        Commands::PrepareNq {
            input_dir,
            output_dir,
            max_documents,
            max_queries,
        } => {
            info!("Preparing NQ dataset: {:?} -> {:?}", input_dir, output_dir);
            prepare_nq::prepare_nq_data(
                std::path::Path::new(input_dir),
                std::path::Path::new(output_dir),
                *max_documents,
                *max_queries,
            )?;
        }
    }

    Ok(())
}

async fn setup_datasets(dataset: &str) -> Result<()> {
    match dataset {
        "beir" => {
            info!("Setting up BEIR datasets...");
            let beir_loader = BeirDataset::new("benchmarks/data/beir".to_string());
            beir_loader.download_all().await?;
        }
        "msmarco" => {
            info!("Setting up MS MARCO datasets...");
            let msmarco_loader = MsMarcoDataset::new("benchmarks/data/msmarco".to_string());
            msmarco_loader.download().await?;
        }
        "all" => {
            info!("Setting up all datasets...");
            let beir_loader = BeirDataset::new("benchmarks/data/beir".to_string());
            beir_loader.download_all().await?;

            let msmarco_loader = MsMarcoDataset::new("benchmarks/data/msmarco".to_string());
            msmarco_loader.download().await?;
        }
        _ => {
            warn!("Unknown dataset: {}", dataset);
            return Err(anyhow::anyhow!("Unsupported dataset: {}", dataset));
        }
    }

    info!("Dataset setup completed successfully");
    Ok(())
}

async fn run_benchmarks(config_path: &str, dataset: &str, search_mode: &str) -> Result<()> {
    // Load configuration
    let config = BenchmarkConfig::from_file(config_path)?;

    info!("Starting benchmark run");
    info!("Dataset: {}, Search mode: {}", dataset, search_mode);
    info!("Searcher URL: {}", config.searcher_url);
    info!("Database URL: {}", config.database_url);

    // Initialize indexer for database setup and data loading
    let indexer = BenchmarkIndexer::new(config.clone()).await?;

    // Setup benchmark database
    indexer.setup_benchmark_database().await?;

    // Load the specified dataset
    let dataset_loader: Box<dyn DatasetLoader> = match dataset {
        "beir" => {
            let mut beir_dataset = BeirDataset::new(config.datasets.beir.cache_dir.clone())
                .with_datasets(config.datasets.beir.datasets.clone())
                .with_download_url(config.datasets.beir.download_url_base.clone());

            if let Some(selected) = &config.datasets.beir.selected_dataset {
                beir_dataset = beir_dataset.with_selected_dataset(selected.clone());
            }

            Box::new(beir_dataset)
        }
        "msmarco" => Box::new(MsMarcoDataset::new(
            config.datasets.msmarco.cache_dir.clone(),
        )),
        _ => return Err(anyhow::anyhow!("Unsupported dataset: {}", dataset)),
    };

    // Use streaming for document indexing if required
    let _source_id = if config.index_documents_before_search {
        info!("Streaming documents to Omni database (memory-efficient mode)");
        let document_stream = dataset_loader.stream_documents();
        let source_id = indexer
            .index_document_stream(&dataset, document_stream)
            .await?;
        indexer.wait_for_indexing_completion(&source_id).await?;

        let stats = indexer.get_index_stats(&source_id).await?;
        info!(
            "Index stats: {} documents, {} embeddings",
            stats.total_documents, stats.total_embeddings
        );
        Some(source_id)
    } else {
        None
    };

    // Initialize search client and evaluator
    let search_client = OmniSearchClient::new(&config.searcher_url)?;
    let evaluator = BenchmarkEvaluator::new(search_client);

    // Run benchmarks based on search mode
    let search_modes = if search_mode == "all" {
        vec!["fulltext", "semantic", "hybrid"]
    } else {
        vec![search_mode]
    };

    for mode in search_modes {
        info!("Running benchmark for search mode: {}", mode);
        let results = evaluator
            .run_benchmark(dataset_loader.as_ref(), mode, &config)
            .await?;

        // Save results
        let timestamp = chrono::Utc::now().format("%Y%m%d_%H%M%S");
        let results_file = format!(
            "benchmarks/results/{}_{}_{}_results.json",
            dataset, mode, timestamp
        );
        results.save_to_file(&results_file)?;

        info!("Results saved to: {}", results_file);

        // Print summary
        results.print_summary();
    }

    info!("Benchmark run completed successfully!");
    Ok(())
}

async fn generate_report(results_dir: &str, format: &str) -> Result<()> {
    let reporter = BenchmarkReporter::new(results_dir.to_string());

    match format {
        "json" => {
            let report = reporter.generate_json_report().await?;
            println!("{}", serde_json::to_string_pretty(&report)?);
        }
        "html" => {
            let html_path = reporter.generate_html_report().await?;
            info!("HTML report generated: {}", html_path);
        }
        "csv" => {
            let csv_path = reporter.generate_csv_report().await?;
            info!("CSV report generated: {}", csv_path);
        }
        _ => {
            return Err(anyhow::anyhow!("Unsupported format: {}", format));
        }
    }

    Ok(())
}

async fn run_latency_benchmark(
    config_path: &str,
    data_dir: &str,
    num_queries: usize,
    concurrency: usize,
    warmup: usize,
    skip_indexing: bool,
    max_documents: Option<usize>,
    target_qps: f64,
    cleanup: bool,
) -> Result<()> {
    // Load configuration
    let config = BenchmarkConfig::from_file(config_path)?;

    info!("=== Omni Perf Benchmark ===");
    info!("Data directory: {}", data_dir);
    info!("Searcher URL: {}", config.searcher_url);
    info!("Database URL: {}", config.database_url);
    if target_qps > 0.0 {
        info!("Target QPS: {:.1}", target_qps);
    } else {
        info!("Mode: burst (unlimited QPS)");
    }

    // Create NQ dataset loader
    let mut dataset = NaturalQuestionsDataset::new(data_dir.to_string());
    if let Some(max) = max_documents {
        dataset = dataset.with_max_documents(max);
    }

    // Verify dataset exists
    dataset.download().await?;

    // Initialize indexer
    let indexer = BenchmarkIndexer::new(config.clone()).await?;

    // Index documents if not skipping
    if !skip_indexing {
        info!("Setting up benchmark database...");
        indexer.setup_benchmark_database().await?;

        info!("Indexing documents...");
        let document_stream = dataset.stream_documents();
        let (source_id, indexing_stats) = indexer
            .index_document_stream_with_stats("natural-questions", document_stream)
            .await?;

        indexing_stats.print_summary();

        let index_stats = indexer.get_index_stats(&source_id).await?;
        info!(
            "Index ready: {} documents, {} embeddings",
            index_stats.total_documents, index_stats.total_embeddings
        );
    } else {
        info!("Skipping indexing, using existing data");
    }

    // Load queries
    info!("Loading queries...");
    let queries: Vec<_> = dataset
        .stream_queries()
        .filter_map(|r| async { r.ok() })
        .take(num_queries + warmup)
        .collect()
        .await;

    info!("Loaded {} queries", queries.len());

    if queries.len() < warmup {
        return Err(anyhow::anyhow!(
            "Not enough queries. Have {}, need at least {} for warmup",
            queries.len(),
            warmup
        ));
    }

    // Create search client and latency evaluator
    let search_client = OmniSearchClient::new(&config.searcher_url)?;

    // Create database pool for system info queries
    let db_pool = sqlx::postgres::PgPoolOptions::new()
        .max_connections(5)
        .connect(&config.database_url)
        .await?;

    let evaluator = LatencyBenchmarkEvaluator::new(search_client, db_pool);

    // Configure benchmark
    let benchmark_config = LatencyBenchmarkConfig {
        num_queries,
        concurrency,
        warmup_queries: warmup,
        timeout_ms: 30000,
        target_qps,
    };

    // Run benchmark (FTS mode for ParadeDB benchmark)
    info!("Running latency benchmark (FTS mode)...");
    let result = evaluator
        .run_benchmark(
            queries,
            &benchmark_config,
            "natural-questions",
            SearchMode::Fulltext,
        )
        .await?;

    // Print results
    result.print_summary();

    // Save results
    let timestamp = chrono::Utc::now().format("%Y%m%d_%H%M%S");
    let results_file = format!(
        "benchmarks/results/latency/nq_fulltext_{}_results.json",
        timestamp
    );
    result.save_to_file(&results_file)?;
    info!("Results saved to: {}", results_file);

    // Cleanup if requested
    if cleanup {
        info!("Cleaning up benchmark database...");
        indexer.cleanup_benchmark_data().await?;
        info!("Cleanup completed");
    }

    info!("Latency benchmark completed!");
    Ok(())
}

use anyhow::Result;
use clap::{Parser, Subcommand};
use tracing::{info, warn};

mod config;
mod datasets;
mod evaluator;
mod indexer;
mod prepare_nq;
mod reporter;
mod search_client;

use config::BenchmarkConfig;
use datasets::{BeirDataset, DatasetLoader, MsMarcoDataset};
use evaluator::BenchmarkEvaluator;
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
    /// Run benchmark evaluation (relevance + latency)
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
        /// Number of warmup queries (not measured)
        #[arg(long, default_value = "50")]
        warmup: usize,
        /// Concurrent query execution
        #[arg(long)]
        concurrency: Option<usize>,
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
            warmup,
            concurrency,
        } => {
            info!(
                "Running benchmarks with config: {}, dataset: {}, mode: {}",
                config, dataset, search_mode
            );
            run_benchmarks(config, dataset, search_mode, *warmup, *concurrency).await?;
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

async fn run_benchmarks(
    config_path: &str,
    dataset: &str,
    search_mode: &str,
    warmup: usize,
    concurrency_override: Option<usize>,
) -> Result<()> {
    let mut config = BenchmarkConfig::from_file(config_path)?;

    if let Some(concurrency) = concurrency_override {
        config.concurrent_queries = concurrency;
    }

    info!("Starting benchmark run");
    info!("Dataset: {}, Search mode: {}", dataset, search_mode);
    info!("Searcher URL: {}", config.searcher_url);
    info!("Database URL: {}", config.database_url);
    info!(
        "Warmup: {} queries, Concurrency: {}",
        warmup, config.concurrent_queries
    );

    let indexer = BenchmarkIndexer::new(config.clone()).await?;

    indexer.setup_benchmark_database().await?;

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

    let system_info = indexer.get_system_info().await.ok();

    let search_client = OmniSearchClient::new(&config.searcher_url)?;
    let evaluator = BenchmarkEvaluator::new(search_client);

    let search_modes = if search_mode == "all" {
        vec!["fulltext", "semantic", "hybrid"]
    } else {
        vec![search_mode]
    };

    for mode in search_modes {
        info!("Running benchmark for search mode: {}", mode);
        let mut result = evaluator
            .run_benchmark(dataset_loader.as_ref(), mode, &config, warmup)
            .await?;

        result.system_info = system_info.clone();

        let timestamp = chrono::Utc::now().format("%Y%m%d_%H%M%S");
        let results_file = format!(
            "benchmarks/results/{}_{}_{}_results.json",
            dataset, mode, timestamp
        );
        result.save_to_file(&results_file)?;

        info!("Results saved to: {}", results_file);

        result.print_summary();
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

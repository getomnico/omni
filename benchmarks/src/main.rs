use anyhow::Result;
use clap::{Parser, Subcommand};
use tracing::{info, warn};

mod config;
mod datasets;
mod evaluator;
mod indexer;
mod reporter;
mod search_client;

use config::BenchmarkConfig;
use datasets::{BeirDataset, DatasetLoader, MsMarcoDataset};
use evaluator::BenchmarkEvaluator;
use indexer::BenchmarkIndexer;
use reporter::BenchmarkReporter;
use search_client::ClioSearchClient;

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
        info!("Streaming documents to Clio database (memory-efficient mode)");
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
    let search_client = ClioSearchClient::new(&config.searcher_url)?;
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

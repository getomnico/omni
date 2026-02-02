use anyhow::Result;
use dotenvy::dotenv;
use shared::DatabasePool;
use tracing::info;
use tracing_subscriber::{layer::SubscriberExt, util::SubscriberInitExt};

mod config;
mod models;
mod scanner;
mod sync;
mod watcher;

use config::FileSystemConnectorConfig;

use shared::queue::EventQueue;
use sync::FileSystemSyncManager;

#[tokio::main]
async fn main() -> Result<()> {
    dotenv().ok();

    tracing_subscriber::registry()
        .with(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "filesystem_connector=info".into()),
        )
        .with(tracing_subscriber::fmt::layer())
        .init();

    info!("Starting FileSystem Connector");

    let config = FileSystemConnectorConfig::from_env();
    let db_pool = DatabasePool::from_config(&config.database).await?;
    let event_queue = EventQueue::new(db_pool.pool().clone());

    let mut sync_manager = FileSystemSyncManager::new(db_pool.pool().clone(), event_queue).await?;

    // Load filesystem sources from database
    sync_manager.load_sources().await?;

    // Start the sync manager (this will run indefinitely)
    info!("Starting filesystem sync processes");
    sync_manager.start_sync_manager().await?;

    info!("FileSystem Connector stopped");
    Ok(())
}

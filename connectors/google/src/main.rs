use anyhow::Result;
use dotenvy::dotenv;
use shared::{DatabasePool, GoogleConnectorConfig};
use std::sync::Arc;
use tokio::time::{interval, Duration};
use tracing::{error, info};
use tracing_subscriber::{layer::SubscriberExt, util::SubscriberInitExt};

mod auth;
mod drive;
mod models;
mod sync;

use sync::SyncManager;

#[tokio::main]
async fn main() -> Result<()> {
    dotenv().ok();

    tracing_subscriber::registry()
        .with(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "google_connector=info".into()),
        )
        .with(tracing_subscriber::fmt::layer())
        .init();

    info!("Starting Google Connector");

    let config = GoogleConnectorConfig::from_env();

    let redis_client = redis::Client::open(config.base.redis.redis_url)?;

    let db_pool = DatabasePool::from_config(&config.database).await?;

    let sync_manager = Arc::new(SyncManager::new(db_pool.pool().clone(), redis_client).await?);

    let mut sync_interval = interval(Duration::from_secs(300));

    loop {
        sync_interval.tick().await;

        info!("Starting sync cycle");

        let sync_manager_clone = Arc::clone(&sync_manager);
        tokio::spawn(async move {
            if let Err(e) = sync_manager_clone.sync_all_sources().await {
                error!("Sync cycle failed: {}", e);
            }
        });
    }
}

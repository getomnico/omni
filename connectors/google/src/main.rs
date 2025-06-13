use anyhow::Result;
use dotenvy::dotenv;
use sqlx::postgres::PgPoolOptions;
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

    let database_url = std::env::var("DATABASE_URL")?;
    let redis_url = std::env::var("REDIS_URL")?;

    let pool = PgPoolOptions::new()
        .max_connections(5)
        .connect(&database_url)
        .await?;

    let redis_client = redis::Client::open(redis_url)?;
    
    let sync_manager = Arc::new(SyncManager::new(pool, redis_client).await?);

    let mut sync_interval = interval(Duration::from_secs(60));

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
use anyhow::Result;
use dotenvy::dotenv;
use shared::{DatabasePool, GoogleConnectorConfig};
use std::sync::Arc;
use tokio::time::{interval, Duration};
use tracing::{error, info};
use tracing_subscriber::{layer::SubscriberExt, util::SubscriberInitExt};

mod api;
mod auth;
mod drive;
mod models;
mod sync;

use api::{create_router, ApiState};
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

    // Create API state
    let api_state = ApiState {
        sync_manager: Arc::clone(&sync_manager),
    };

    // Create HTTP server
    let app = create_router(api_state);
    let port = std::env::var("PORT")?.parse::<u16>()?;
    let addr = std::net::SocketAddr::from(([0, 0, 0, 0], port));
    let listener = tokio::net::TcpListener::bind(addr).await?;

    info!("HTTP server listening on {}", addr);

    // Run HTTP server and sync loop concurrently
    let http_server = axum::serve(listener, app);
    let sync_loop = async {
        let sync_interval_seconds = std::env::var("GOOGLE_SYNC_INTERVAL_SECONDS")
            .unwrap_or_else(|_| "86400".to_string())
            .parse::<u64>()
            .expect("SYNC_INTERVAL_SECONDS must be a valid number");

        // Check for initial sync on startup
        info!("Checking if initial sync is needed on startup");
        let sync_manager_clone = Arc::clone(&sync_manager);
        tokio::spawn(async move {
            if let Err(e) = sync_manager_clone.sync_all_sources().await {
                error!("Initial sync check failed: {}", e);
            }
        });

        // Continue with regular interval syncs
        let mut sync_interval = interval(Duration::from_secs(sync_interval_seconds));
        loop {
            sync_interval.tick().await;
            info!("Starting scheduled sync cycle");

            let sync_manager_clone = Arc::clone(&sync_manager);
            tokio::spawn(async move {
                if let Err(e) = sync_manager_clone.sync_all_sources().await {
                    error!("Sync cycle failed: {}", e);
                }
            });
        }
    };

    // Run both tasks concurrently
    tokio::select! {
        result = http_server => {
            error!("HTTP server stopped: {:?}", result);
        }
        _ = sync_loop => {
            error!("Sync loop stopped unexpectedly");
        }
    }

    Ok(())
}

use anyhow::Result;
use dotenvy::dotenv;
use shared::{
    telemetry::{self, TelemetryConfig},
    DatabasePool, WebConnectorConfig,
};
use std::sync::Arc;
use tokio::time::{interval, Duration};
use tracing::{error, info};

mod api;
mod config;
mod models;
mod sync;

use api::{create_router, ApiState};
use sync::SyncManager;

#[tokio::main]
async fn main() -> Result<()> {
    dotenv().ok();

    let telemetry_config = TelemetryConfig::from_env("omni-web-connector");
    telemetry::init_telemetry(telemetry_config)?;

    info!("Starting Web Connector");

    let config = WebConnectorConfig::from_env();

    let redis_client = redis::Client::open(config.base.redis.redis_url)?;

    let db_pool = DatabasePool::from_config(&config.database).await?;

    let sync_manager = Arc::new(SyncManager::new(db_pool.pool().clone(), redis_client).await?);

    let api_state = ApiState {
        sync_manager: Arc::clone(&sync_manager),
    };

    let app = create_router(api_state);
    let port = std::env::var("PORT")?.parse::<u16>()?;
    let addr = std::net::SocketAddr::from(([0, 0, 0, 0], port));
    let listener = tokio::net::TcpListener::bind(addr).await?;

    info!("HTTP server listening on {}", addr);

    let sync_interval_secs = std::env::var("WEB_SYNC_INTERVAL_SECONDS")
        .ok()
        .and_then(|s| s.parse::<u64>().ok())
        .unwrap_or(86400); // Default to daily

    let http_server = axum::serve(listener, app);
    let sync_loop = async move {
        let mut sync_interval = interval(Duration::from_secs(sync_interval_secs));
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

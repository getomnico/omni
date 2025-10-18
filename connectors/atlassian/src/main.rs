use anyhow::Result;
use dotenvy::dotenv;
use shared::{
    telemetry::{self, TelemetryConfig},
    AtlassianConnectorConfig, DatabasePool,
};
use std::sync::Arc;
use tokio::sync::Mutex;
use tokio::time::{interval, Duration};
use tracing::{error, info};

mod api;
mod auth;
mod client;
mod confluence;
mod jira;
mod models;
mod sync;

use api::{create_router, ApiState};
use sync::SyncManager;

#[tokio::main]
async fn main() -> Result<()> {
    dotenv().ok();

    let telemetry_config = TelemetryConfig::from_env("omni-atlassian-connector");
    telemetry::init_telemetry(telemetry_config)?;

    info!("Starting Atlassian Connector");

    let config = AtlassianConnectorConfig::from_env();

    let redis_client = redis::Client::open(config.base.redis.redis_url)?;

    let db_pool = DatabasePool::from_config(&config.database).await?;

    let sync_manager = Arc::new(Mutex::new(
        SyncManager::new(db_pool.pool().clone(), redis_client).await?,
    ));

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
        let mut sync_interval = interval(Duration::from_secs(300)); // 5 minutes
        loop {
            sync_interval.tick().await;
            info!("Starting periodic sync cycle");

            let sync_manager_clone = Arc::clone(&sync_manager);
            tokio::spawn(async move {
                let mut sync_manager = sync_manager_clone.lock().await;
                if let Err(e) = sync_manager.sync_all_sources().await {
                    error!("Sync cycle failed: {}", e);
                } else {
                    info!("Sync cycle completed successfully");
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

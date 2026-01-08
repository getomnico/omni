use anyhow::Result;
use dashmap::DashSet;
use dotenvy::dotenv;
use shared::{
    telemetry::{self, TelemetryConfig},
    DatabasePool, WebConnectorConfig,
};
use std::sync::Arc;
use tracing::info;

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
        active_syncs: Arc::new(DashSet::new()),
    };

    let app = create_router(api_state);
    let port = std::env::var("PORT")?.parse::<u16>()?;
    let addr = std::net::SocketAddr::from(([0, 0, 0, 0], port));
    let listener = tokio::net::TcpListener::bind(addr).await?;

    info!("HTTP server listening on {}", addr);

    axum::serve(listener, app).await?;

    Ok(())
}

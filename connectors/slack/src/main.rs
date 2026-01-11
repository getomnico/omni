use anyhow::Result;
use dashmap::DashSet;
use dotenvy::dotenv;
use shared::telemetry::{self, TelemetryConfig};
use std::sync::Arc;
use tokio::sync::Mutex;
use tracing::info;

mod api;
mod auth;
mod client;
mod content;
mod models;
mod sync;

use api::{create_router, ApiState};
use shared::SdkClient;
use sync::SyncManager;

#[tokio::main]
async fn main() -> Result<()> {
    dotenv().ok();

    let telemetry_config = TelemetryConfig::from_env("omni-slack-connector");
    telemetry::init_telemetry(telemetry_config)?;

    info!("Starting Slack Connector");

    let redis_url = std::env::var("REDIS_URL")?;
    let redis_client = redis::Client::open(redis_url)?;

    let sdk_client = SdkClient::from_env()?;

    let sync_manager = Arc::new(Mutex::new(SyncManager::new(redis_client, sdk_client)));

    // Create API state
    let api_state = ApiState {
        sync_manager,
        active_syncs: Arc::new(DashSet::new()),
    };

    // Create HTTP server
    let app = create_router(api_state);
    let port = std::env::var("PORT")?.parse::<u16>()?;
    let addr = std::net::SocketAddr::from(([0, 0, 0, 0], port));
    let listener = tokio::net::TcpListener::bind(addr).await?;

    info!("HTTP server listening on {}", addr);

    // Run HTTP server
    axum::serve(listener, app).await?;

    Ok(())
}

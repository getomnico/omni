use anyhow::Result;
use dotenvy::dotenv;
use shared::telemetry::{self, TelemetryConfig};
use std::sync::Arc;
use tokio::sync::Mutex;
use tracing::{error, info};

mod api;
mod client;
mod config;
mod models;
mod sync;

use shared::SdkClient;

use api::{build_manifest, create_router, ApiState};
use sync::SyncManager;

#[tokio::main]
async fn main() -> Result<()> {
    dotenv().ok();

    let telemetry_config = TelemetryConfig::from_env("omni-fireflies-connector");
    telemetry::init_telemetry(telemetry_config)?;

    info!("Starting Fireflies Connector");

    let sdk_client = SdkClient::from_env()?;
    let sync_manager = Arc::new(Mutex::new(SyncManager::new(sdk_client)));

    let api_state = ApiState {
        sync_manager: Arc::clone(&sync_manager),
    };

    shared::start_registration_loop(build_manifest(None));

    let app = create_router(api_state);
    let port = std::env::var("PORT")?.parse::<u16>()?;
    let addr = std::net::SocketAddr::from(([0, 0, 0, 0], port));
    let listener = tokio::net::TcpListener::bind(addr).await?;

    info!("HTTP server listening on {}", addr);

    if let Err(e) = axum::serve(listener, app).await {
        error!("HTTP server stopped: {:?}", e);
    }

    Ok(())
}

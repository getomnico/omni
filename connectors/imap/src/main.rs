use anyhow::Result;
use dotenvy::dotenv;
use omni_imap_connector::api::{build_manifest, create_router, ApiState};
use omni_imap_connector::sync::SyncManager;
use shared::telemetry::{self, TelemetryConfig};
use shared::SdkClient;
use std::sync::Arc;
use tracing::{error, info};

#[tokio::main]
async fn main() -> Result<()> {
    dotenv().ok();

    let telemetry_config = TelemetryConfig::from_env("omni-imap-connector");
    telemetry::init_telemetry(telemetry_config)?;

    info!("Starting IMAP Connector");

    let sdk_client = SdkClient::from_env()?;
    let sync_manager = Arc::new(SyncManager::new(sdk_client));

    let api_state = ApiState {
        sync_manager: Arc::clone(&sync_manager),
    };

    shared::start_registration_loop(build_manifest(shared::build_connector_url()));

    let app = create_router(api_state);
    let port = std::env::var("PORT")
        .expect("PORT environment variable must be set")
        .parse::<u16>()?;
    let addr = std::net::SocketAddr::from(([0, 0, 0, 0], port));
    let listener = tokio::net::TcpListener::bind(addr).await?;

    info!("HTTP server listening on {}", addr);

    if let Err(e) = axum::serve(listener, app).await {
        error!("HTTP server stopped: {:?}", e);
    }

    Ok(())
}

use anyhow::Result;
use dashmap::DashSet;
use dotenvy::dotenv;
use shared::telemetry::{self, TelemetryConfig};
use std::sync::Arc;
use tracing::{info, warn};

mod api;
mod socket;

use api::{create_router, maybe_start_socket_with_sync, ApiState};
use omni_slack_connector::models::SlackConnectorState;
use omni_slack_connector::sync::SyncManager;
use shared::SdkClient;
use socket::SocketModeManager;

#[tokio::main]
async fn main() -> Result<()> {
    rustls::crypto::ring::default_provider()
        .install_default()
        .expect("Failed to install rustls crypto provider");

    dotenv().ok();

    let telemetry_config = TelemetryConfig::from_env("omni-slack-connector");
    telemetry::init_telemetry(telemetry_config)?;

    info!("Starting Slack Connector");

    let sdk_client = SdkClient::from_env()?;
    let socket_manager = Arc::new(SocketModeManager::new());

    let sync_manager = Arc::new(SyncManager::new(sdk_client.clone()));

    // Create API state
    let api_state = ApiState {
        sync_manager,
        active_syncs: Arc::new(DashSet::new()),
        socket_manager: socket_manager.clone(),
    };

    // Reconnect Socket Mode for existing sources that have completed a sync
    let startup_sdk = sdk_client.clone();
    let startup_sm = socket_manager.clone();
    let startup_sync = api_state.sync_manager.clone();
    tokio::spawn(async move {
        reconnect_existing_sources(&startup_sdk, &startup_sm, &startup_sync).await;
    });

    // Create HTTP server
    let app = create_router(api_state);
    let port = std::env::var("PORT")?.parse::<u16>()?;
    let addr = std::net::SocketAddr::from(([0, 0, 0, 0], port));
    let listener = tokio::net::TcpListener::bind(addr).await?;

    info!("HTTP server listening on {}", addr);

    // Run HTTP server (blocks until shutdown)
    axum::serve(listener, app).await?;

    // Graceful shutdown: close all WebSocket connections
    socket_manager.stop_all().await;

    Ok(())
}

async fn reconnect_existing_sources(
    sdk_client: &SdkClient,
    socket_manager: &SocketModeManager,
    sync_manager: &Arc<SyncManager>,
) {
    let sources = match sdk_client.get_sources_by_type("slack").await {
        Ok(s) => s,
        Err(e) => {
            warn!("Failed to list existing Slack sources on startup: {}", e);
            return;
        }
    };

    for source in sources {
        let state: Option<SlackConnectorState> = sdk_client
            .get_connector_state(&source.id)
            .await
            .ok()
            .flatten()
            .and_then(|v| serde_json::from_value(v).ok());

        if let Some(state) = state {
            if state.team_id.is_some() {
                maybe_start_socket_with_sync(
                    &source.id,
                    sdk_client,
                    socket_manager,
                    Some(sync_manager.clone()),
                )
                .await;
            }
        }
    }
}

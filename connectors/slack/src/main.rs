use anyhow::Result;
use dotenvy::dotenv;
use omni_connector_sdk::{serve_with_config, ServerConfig};
use shared::telemetry::{self, TelemetryConfig};
use shared::SdkClient;
use std::sync::Arc;
use tracing::{info, warn};

use omni_slack_connector::connector::{start_socket_for_source, SlackConnector};
use omni_slack_connector::models::SlackConnectorState;
use omni_slack_connector::socket::SocketModeManager;
use omni_slack_connector::sync::SyncManager;

#[tokio::main]
async fn main() -> Result<()> {
    rustls::crypto::ring::default_provider()
        .install_default()
        .expect("Failed to install rustls crypto provider");

    dotenv().ok();

    telemetry::init_telemetry(TelemetryConfig::from_env("omni-slack-connector"))?;

    info!("Starting Slack Connector");

    let sdk_client = SdkClient::from_env()?;
    let socket_manager = Arc::new(SocketModeManager::new());
    let sync_manager = Arc::new(SyncManager::new(sdk_client.clone()));

    // Reconnect Socket Mode for existing sources whose first sync has completed.
    // Runs in the background so it doesn't block the HTTP server from starting.
    {
        let sdk = sdk_client.clone();
        let sm = socket_manager.clone();
        let sync = sync_manager.clone();
        tokio::spawn(async move {
            reconnect_existing_sources(&sdk, &sm, &sync).await;
        });
    }

    let connector = SlackConnector::new(sync_manager, socket_manager);

    // SDK provides /health, /manifest, /sync, /cancel, /action, registration loop,
    // request decoding, sync registration, and cancellation plumbing.
    serve_with_config(connector, ServerConfig::from_env()?).await
}

async fn reconnect_existing_sources(
    sdk_client: &SdkClient,
    socket_manager: &Arc<SocketModeManager>,
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
                start_socket_for_source(
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

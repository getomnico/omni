use anyhow::Result;
use dotenvy::dotenv;
use omni_connector_sdk::{serve_with_config, ServerConfig};
use shared::telemetry::{self, TelemetryConfig};
use shared::SdkClient;
use std::sync::Arc;
use tracing::info;

use omni_slack_connector::connector::SlackConnector;
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

    let connector = SlackConnector::new(sync_manager, socket_manager);

    // SDK provides /health, /manifest, /sync, /cancel, /action, registration loop,
    // request decoding, sync registration, and cancellation plumbing. Realtime
    // (Socket Mode) sessions start when the connector-manager triggers a
    // `Realtime` sync; no separate startup reconnect is needed.
    serve_with_config(connector, ServerConfig::from_env()?).await
}

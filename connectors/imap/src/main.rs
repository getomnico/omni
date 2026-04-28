use anyhow::Result;
use dotenvy::dotenv;
use omni_connector_sdk::serve;
use shared::telemetry::{self, TelemetryConfig};
use std::sync::Arc;
use tracing::info;

use omni_imap_connector::connector::ImapConnector;
use omni_imap_connector::sync::SyncManager;

#[tokio::main]
async fn main() -> Result<()> {
    dotenv().ok();

    let telemetry_config = TelemetryConfig::from_env("omni-imap-connector");
    telemetry::init_telemetry(telemetry_config)?;

    info!("Starting IMAP Connector");

    let sync_manager = Arc::new(SyncManager::new());
    let connector = ImapConnector::new(sync_manager);
    serve(connector).await
}

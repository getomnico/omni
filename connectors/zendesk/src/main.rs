use anyhow::Result;
use dotenvy::dotenv;
use omni_connector_sdk::telemetry::{self, TelemetryConfig};
use omni_connector_sdk::{ServerConfig, serve_with_config};
use tracing::info;

use omni_zendesk_connector::connector::ZendeskConnector;

#[tokio::main]
async fn main() -> Result<()> {
    dotenv().ok();

    telemetry::init_telemetry(TelemetryConfig::from_env("omni-zendesk-connector"))?;

    info!("Starting Zendesk Connector");

    serve_with_config(ZendeskConnector::new(), ServerConfig::from_env()?).await
}

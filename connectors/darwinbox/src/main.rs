use anyhow::Result;
use dotenvy::dotenv;
use omni_connector_sdk::telemetry::{self, TelemetryConfig};
use omni_connector_sdk::{serve_with_config, ServerConfig};
use tracing::info;

use omni_darwinbox_connector::connector::DarwinboxConnector;

#[tokio::main]
async fn main() -> Result<()> {
    dotenv().ok();

    telemetry::init_telemetry(TelemetryConfig::from_env("omni-darwinbox-connector"))?;

    info!("Starting Darwinbox Connector");

    serve_with_config(DarwinboxConnector::new(), ServerConfig::from_env()?).await
}

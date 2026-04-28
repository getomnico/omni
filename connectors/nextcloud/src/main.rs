use anyhow::Result;
use dotenvy::dotenv;
use omni_connector_sdk::{serve_with_config, ServerConfig};
use omni_nextcloud_connector::connector::NextcloudConnector;
use shared::telemetry::{self, TelemetryConfig};
use tracing::info;

#[tokio::main]
async fn main() -> Result<()> {
    dotenv().ok();

    telemetry::init_telemetry(TelemetryConfig::from_env("omni-nextcloud-connector"))?;

    info!("Starting Nextcloud Connector");

    serve_with_config(NextcloudConnector::new(), ServerConfig::from_env()?).await
}

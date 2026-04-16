use anyhow::Result;
use dotenvy::dotenv;
use omni_connector_sdk::serve;
use omni_filesystem_connector::connector::FileSystemConnector;
use shared::telemetry::{self, TelemetryConfig};
use tracing::info;

#[tokio::main]
async fn main() -> Result<()> {
    dotenv().ok();

    let telemetry_config = TelemetryConfig::from_env("omni-filesystem-connector");
    telemetry::init_telemetry(telemetry_config)?;

    info!("Starting FileSystem Connector");

    serve(FileSystemConnector).await
}

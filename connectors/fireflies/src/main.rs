use anyhow::Result;
use dotenvy::dotenv;
use omni_connector_sdk::telemetry::{self, TelemetryConfig};
use omni_connector_sdk::{serve_with_config, ServerConfig};
use tracing::info;

mod client;
mod config;
mod connector;
mod models;
mod sync;

use connector::FirefliesConnector;

#[tokio::main]
async fn main() -> Result<()> {
    dotenv().ok();

    telemetry::init_telemetry(TelemetryConfig::from_env("omni-fireflies-connector"))?;

    info!("Starting Fireflies Connector");

    serve_with_config(FirefliesConnector::new(), ServerConfig::from_env()?).await
}

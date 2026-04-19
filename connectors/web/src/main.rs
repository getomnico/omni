use anyhow::{Context, Result};
use dotenvy::dotenv;
use omni_connector_sdk::serve;
use omni_web_connector::connector::WebConnector;
use shared::telemetry::{self, TelemetryConfig};
use shared::SdkClient;
use tracing::info;

#[tokio::main]
async fn main() -> Result<()> {
    dotenv().ok();

    let telemetry_config = TelemetryConfig::from_env("omni-web-connector");
    telemetry::init_telemetry(telemetry_config)?;

    info!("Starting Web Connector");

    let redis_url =
        std::env::var("REDIS_URL").unwrap_or_else(|_| "redis://localhost:6379".to_string());
    let redis_client = redis::Client::open(redis_url).context("Failed to create Redis client")?;

    let sdk_client = SdkClient::from_env()?;

    serve(WebConnector::new(redis_client, sdk_client)).await
}

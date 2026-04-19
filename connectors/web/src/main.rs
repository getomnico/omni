use anyhow::Result;
use dotenvy::dotenv;
use omni_connector_sdk::serve;
use omni_connector_sdk::telemetry::{self, TelemetryConfig};
use omni_connector_sdk::SdkClient;
use omni_web_connector::connector::WebConnector;
use tracing::info;

#[tokio::main]
async fn main() -> Result<()> {
    dotenv().ok();

    let telemetry_config = TelemetryConfig::from_env("omni-web-connector");
    telemetry::init_telemetry(telemetry_config)?;

    info!("Starting Web Connector");

    let sdk_client = SdkClient::from_env()?;
    serve(WebConnector::new(sdk_client)).await
}

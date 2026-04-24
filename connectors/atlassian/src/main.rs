use anyhow::Result;
use dotenvy::dotenv;
use omni_atlassian_connector::config::AtlassianConnectorConfig;
use omni_atlassian_connector::connector::AtlassianConnector;
use omni_atlassian_connector::routes;
use omni_atlassian_connector::sync::SyncManager;
use omni_connector_sdk::{serve_with_extra_routes, ServerConfig};
use shared::telemetry::{self, TelemetryConfig};
use shared::SdkClient;
use std::sync::Arc;
use std::time::Duration;
use tokio::time::sleep;
use tracing::info;

const WEBHOOK_RENEWAL_INTERVAL_SECS: u64 = 3600;

#[tokio::main]
async fn main() -> Result<()> {
    dotenv().ok();

    let telemetry_config = TelemetryConfig::from_env("omni-atlassian-connector");
    telemetry::init_telemetry(telemetry_config)?;

    info!("Starting Atlassian Connector");

    let config = AtlassianConnectorConfig::from_env();
    let sdk_client = SdkClient::from_env()?;

    let sync_manager = Arc::new(SyncManager::new(sdk_client, config.webhook_url.clone()));

    if config.webhook_url.is_some() {
        let renewal_manager = Arc::clone(&sync_manager);
        tokio::spawn(async move {
            loop {
                sleep(Duration::from_secs(WEBHOOK_RENEWAL_INTERVAL_SECS)).await;
                info!("Running periodic webhook check");
                renewal_manager.ensure_webhooks_for_all_sources().await;
            }
        });
    }

    let extra_routes = routes::build_router(Arc::clone(&sync_manager));
    let connector = AtlassianConnector::new(sync_manager);

    serve_with_extra_routes(connector, ServerConfig::from_env()?, extra_routes).await
}

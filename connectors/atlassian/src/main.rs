use anyhow::Result;
use dotenvy::dotenv;
use shared::telemetry::{self, TelemetryConfig};
use std::sync::Arc;
use tokio::sync::Mutex;
use tracing::{error, info};

mod api;
mod auth;
mod client;
mod config;
mod confluence;
mod jira;
mod models;
mod sync;

use config::AtlassianConnectorConfig;
use shared::SdkClient;

use api::{create_router, ApiState};
use sync::SyncManager;

const WEBHOOK_RENEWAL_INTERVAL_SECS: u64 = 3600;

#[tokio::main]
async fn main() -> Result<()> {
    dotenv().ok();

    let telemetry_config = TelemetryConfig::from_env("omni-atlassian-connector");
    telemetry::init_telemetry(telemetry_config)?;

    info!("Starting Atlassian Connector");

    let config = AtlassianConnectorConfig::from_env();

    let redis_client = redis::Client::open(config.base.redis.redis_url)?;

    let sdk_client = SdkClient::from_env()?;

    let sync_manager = Arc::new(Mutex::new(SyncManager::new(
        redis_client,
        sdk_client,
        config.webhook_url.clone(),
    )));

    if config.webhook_url.is_some() {
        let renewal_manager = Arc::clone(&sync_manager);
        tokio::spawn(async move {
            loop {
                tokio::time::sleep(tokio::time::Duration::from_secs(
                    WEBHOOK_RENEWAL_INTERVAL_SECS,
                ))
                .await;

                info!("Running periodic webhook check");
                let mut manager = renewal_manager.lock().await;
                manager.ensure_webhooks_for_all_sources().await;
            }
        });
    }

    let api_state = ApiState {
        sync_manager: Arc::clone(&sync_manager),
    };

    let app = create_router(api_state);
    let port = std::env::var("PORT")?.parse::<u16>()?;
    let addr = std::net::SocketAddr::from(([0, 0, 0, 0], port));
    let listener = tokio::net::TcpListener::bind(addr).await?;

    info!("HTTP server listening on {}", addr);

    if let Err(e) = axum::serve(listener, app).await {
        error!("HTTP server stopped: {:?}", e);
    }

    Ok(())
}

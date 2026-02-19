use anyhow::Result;
use dashmap::DashSet;
use dotenvy::dotenv;
use shared::models::SourceType;
use shared::telemetry::{self, TelemetryConfig};
use std::sync::Arc;
use std::time::Duration;
use time::OffsetDateTime;
use tokio::time::sleep;
use tracing::{error, info, warn};

mod admin;
mod api;
mod auth;
mod cache;
mod config;
mod drive;
mod gmail;
mod models;
mod sync;

use config::GoogleConnectorConfig;

use shared::SdkClient;

use admin::AdminClient;
use api::{create_router, ApiState};
use shared::RateLimiter;
use sync::SyncManager;

#[tokio::main]
async fn main() -> Result<()> {
    dotenv().ok();

    let telemetry_config = TelemetryConfig::from_env("omni-google-connector");
    telemetry::init_telemetry(telemetry_config)?;

    info!("Starting Google Connector");

    let config = GoogleConnectorConfig::from_env();

    let redis_client = redis::Client::open(config.redis.redis_url.clone())?;

    // Create shared AdminClient with rate limiter
    let api_rate_limit = std::env::var("GOOGLE_API_RATE_LIMIT")
        .unwrap_or_else(|_| "180".to_string())
        .parse::<u32>()
        .unwrap_or(180);
    let max_retries = std::env::var("GOOGLE_MAX_RETRIES")
        .unwrap_or_else(|_| "5".to_string())
        .parse::<u32>()
        .unwrap_or(5);
    let rate_limiter = Arc::new(RateLimiter::new(api_rate_limit, max_retries));
    let admin_client = Arc::new(AdminClient::with_rate_limiter(rate_limiter.clone()));

    let sdk_client = SdkClient::from_env()?;

    let sync_manager = Arc::new(SyncManager::new(
        redis_client,
        config.ai_service_url.clone(),
        Arc::clone(&admin_client),
        sdk_client,
        config.webhook_url.clone(),
    ));

    // Spawn webhook renewal loop if webhooks are enabled
    if config.webhook_url.is_some() {
        let renewal_sync_manager = Arc::clone(&sync_manager);
        let renewal_interval = config.webhook_renewal_interval_seconds;
        tokio::spawn(async move {
            webhook_renewal_loop(renewal_sync_manager, renewal_interval).await;
        });
        info!(
            "Webhook renewal loop started (interval: {}s)",
            config.webhook_renewal_interval_seconds
        );
    } else {
        info!("Webhooks disabled, no renewal loop started");
    }

    // Create API state with shared services
    let api_state = ApiState {
        sync_manager: Arc::clone(&sync_manager),
        admin_client: Arc::clone(&admin_client),
        active_syncs: Arc::new(DashSet::new()),
    };

    // Create HTTP server
    let app = create_router(api_state);
    let port = std::env::var("PORT")?.parse::<u16>()?;
    let addr = std::net::SocketAddr::from(([0, 0, 0, 0], port));
    let listener = tokio::net::TcpListener::bind(addr).await?;

    info!("HTTP server listening on {}", addr);

    // Run HTTP server (connector-manager handles scheduling)
    if let Err(e) = axum::serve(listener, app).await {
        error!("HTTP server stopped: {:?}", e);
    }

    Ok(())
}

async fn webhook_renewal_loop(sync_manager: Arc<SyncManager>, interval_seconds: u64) {
    // Wait before first check to let the system stabilize
    sleep(Duration::from_secs(60)).await;

    let mut interval = tokio::time::interval(Duration::from_secs(interval_seconds));
    interval.tick().await; // consume first immediate tick

    loop {
        interval.tick().await;
        info!("Running webhook renewal check");

        let source_types = [SourceType::GoogleDrive];

        for source_type in &source_types {
            let type_str = serde_json::to_value(source_type)
                .ok()
                .and_then(|v| v.as_str().map(String::from))
                .unwrap_or_default();

            let sources = match sync_manager.sdk_client.get_sources_by_type(&type_str).await {
                Ok(sources) => sources,
                Err(e) => {
                    error!("Failed to get sources for type {}: {}", type_str, e);
                    continue;
                }
            };

            for source in sources {
                let state: models::GoogleConnectorState = match sync_manager
                    .sdk_client
                    .get_connector_state(&source.id)
                    .await
                {
                    Ok(Some(raw_state)) => match serde_json::from_value(raw_state) {
                        Ok(s) => s,
                        Err(e) => {
                            warn!(
                                "Failed to parse connector state for source {}: {}",
                                source.id, e
                            );
                            continue;
                        }
                    },
                    Ok(None) => continue,
                    Err(e) => {
                        warn!(
                            "Failed to get connector state for source {}: {}",
                            source.id, e
                        );
                        continue;
                    }
                };

                let expires_at = state.webhook_expires_at;

                let should_renew = match expires_at {
                    Some(exp_millis) => {
                        let exp_secs = exp_millis / 1000;
                        let now = OffsetDateTime::now_utc().unix_timestamp();
                        let hours_until_expiry = (exp_secs - now) / 3600;
                        hours_until_expiry < 48
                    }
                    None => false,
                };

                if should_renew {
                    info!("Renewing webhook for source {} (expiring soon)", source.id);
                    sync_manager.ensure_webhook_registered(&source.id).await;
                }
            }
        }
    }
}

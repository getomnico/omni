use anyhow::Result;
use dotenvy::dotenv;
use omni_connector_sdk::{serve_with_extra_routes, ServerConfig};
use shared::models::SourceType;
use shared::telemetry::{self, TelemetryConfig};
use shared::SdkClient;
use std::sync::Arc;
use std::time::Duration;
use time::OffsetDateTime;
use tokio::time::sleep;
use tracing::{error, info, warn};

use omni_google_connector::admin::AdminClient;
use omni_google_connector::config::GoogleConnectorConfig;
use omni_google_connector::connector::GoogleConnector;
use omni_google_connector::models;
use omni_google_connector::routes;
use omni_google_connector::sync::SyncManager;
use shared::RateLimiter;

#[tokio::main]
async fn main() -> Result<()> {
    dotenv().ok();

    let telemetry_config = TelemetryConfig::from_env("omni-google-connector");
    telemetry::init_telemetry(telemetry_config)?;

    info!("Starting Google Connector");

    let config = GoogleConnectorConfig::from_env();

    let redis_client = redis::Client::open(config.redis.redis_url.clone())?;

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

    {
        let processor_sync_manager = Arc::clone(&sync_manager);
        tokio::spawn(async move {
            processor_sync_manager.run_webhook_processor().await;
        });
        info!("Webhook debounce processor started");
    }

    let extra_routes = routes::build_router(Arc::clone(&sync_manager), Arc::clone(&admin_client));
    let connector = GoogleConnector::new(Arc::clone(&sync_manager), Arc::clone(&admin_client));

    serve_with_extra_routes(connector, ServerConfig::from_env()?, extra_routes).await
}

async fn webhook_renewal_loop(sync_manager: Arc<SyncManager>, interval_seconds: u64) {
    sleep(Duration::from_secs(60)).await;

    let mut interval = tokio::time::interval(Duration::from_secs(interval_seconds));
    interval.tick().await;

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

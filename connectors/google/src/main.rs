use anyhow::Result;
use dotenvy::dotenv;
use shared::{
    telemetry::{self, TelemetryConfig},
    DatabasePool, GoogleConnectorConfig,
};
use std::sync::Arc;
use tokio::time::{interval, Duration};
use tracing::{error, info};

mod admin;
mod api;
mod auth;
mod cache;
mod drive;
mod gmail;
mod models;
mod sync;

use admin::AdminClient;
use api::{create_router, ApiState};
use auth::GoogleCredentialsService;
use shared::RateLimiter;
use sync::SyncManager;

#[tokio::main]
async fn main() -> Result<()> {
    dotenv().ok();

    let telemetry_config = TelemetryConfig::from_env("omni-google-connector");
    telemetry::init_telemetry(telemetry_config)?;

    info!("Starting Google Connector");

    let config = GoogleConnectorConfig::from_env();

    let redis_client = redis::Client::open(config.base.redis.redis_url)?;

    let db_pool = DatabasePool::from_config(&config.database).await?;

    // Create shared services
    let credentials_service = Arc::new(GoogleCredentialsService::new(db_pool.pool().clone())?);

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

    let sync_manager = Arc::new(
        SyncManager::new(
            db_pool.pool().clone(),
            redis_client,
            config.ai_service_url.clone(),
            Arc::clone(&admin_client),
        )
        .await?,
    );

    // Create API state with shared services
    let api_state = ApiState {
        sync_manager: Arc::clone(&sync_manager),
        credentials_service: Arc::clone(&credentials_service),
        admin_client: Arc::clone(&admin_client),
    };

    // Create HTTP server
    let app = create_router(api_state);
    let port = std::env::var("PORT")?.parse::<u16>()?;
    let addr = std::net::SocketAddr::from(([0, 0, 0, 0], port));
    let listener = tokio::net::TcpListener::bind(addr).await?;

    info!("HTTP server listening on {}", addr);

    // Run HTTP server, sync loop, and webhook renewal concurrently
    let http_server = axum::serve(listener, app);

    let sync_loop = async {
        let sync_interval_seconds = std::env::var("GOOGLE_SYNC_INTERVAL_SECONDS")
            .unwrap_or_else(|_| "86400".to_string())
            .parse::<u64>()
            .expect("GOOGLE_SYNC_INTERVAL_SECONDS must be a valid number");
        info!("Sync interval set to {} secs.", sync_interval_seconds);

        // Combined startup sync check: recover interrupted syncs and check sync schedule
        info!("Running combined startup sync check");
        let sync_manager_clone = Arc::clone(&sync_manager);
        tokio::spawn(async move {
            if let Err(e) = sync_manager_clone.startup_sync_check().await {
                error!("Startup sync check failed: {}", e);
            }
        });

        // Auto-register webhooks on startup
        info!("Auto-registering webhooks on startup");
        let sync_manager_clone = Arc::clone(&sync_manager);
        tokio::spawn(async move {
            if let Err(e) = sync_manager_clone.auto_register_webhooks().await {
                error!("Auto webhook registration failed: {}", e);
            }
        });

        // Continue with regular interval syncs
        let mut sync_interval = interval(Duration::from_secs(sync_interval_seconds));
        loop {
            sync_interval.tick().await;
            info!("Starting scheduled sync cycle");

            let sync_manager_clone = Arc::clone(&sync_manager);
            tokio::spawn(async move {
                if let Err(e) = sync_manager_clone.sync_all_sources().await {
                    error!("Sync cycle failed: {}", e);
                }
            });
        }
    };

    let webhook_renewal_loop = async {
        let renewal_check_interval_seconds =
            std::env::var("WEBHOOK_RENEWAL_CHECK_INTERVAL_SECONDS")
                .unwrap_or_else(|_| "3600".to_string()) // Default: check every hour
                .parse::<u64>()
                .expect("WEBHOOK_RENEWAL_CHECK_INTERVAL_SECONDS must be a valid number");
        info!(
            "Webhook renewal check interval set to {} secs.",
            renewal_check_interval_seconds
        );

        let mut renewal_interval = interval(Duration::from_secs(renewal_check_interval_seconds));

        loop {
            renewal_interval.tick().await;
            info!("Checking for expiring webhook channels");

            let sync_manager_clone = Arc::clone(&sync_manager);
            tokio::spawn(async move {
                // Check for channels expiring in the next 24 hours
                match sync_manager_clone.get_expiring_webhook_channels(24).await {
                    Ok(expiring_channels) => {
                        if !expiring_channels.is_empty() {
                            info!(
                                "Found {} expiring webhook channels",
                                expiring_channels.len()
                            );

                            for channel in expiring_channels {
                                if let Err(e) =
                                    sync_manager_clone.renew_webhook_channel(&channel).await
                                {
                                    error!(
                                        "Failed to renew webhook channel {}: {}",
                                        channel.channel_id, e
                                    );
                                }
                            }
                        }
                    }
                    Err(e) => {
                        error!("Failed to check for expiring webhook channels: {}", e);
                    }
                }
            });
        }
    };

    // Run all tasks concurrently
    tokio::select! {
        result = http_server => {
            error!("HTTP server stopped: {:?}", result);
        }
        _ = sync_loop => {
            error!("Sync loop stopped unexpectedly");
        }
        _ = webhook_renewal_loop => {
            error!("Webhook renewal loop stopped unexpectedly");
        }
    }

    Ok(())
}

use anyhow::Result;
use dotenvy::dotenv;
use shared::{
    telemetry::{self, TelemetryConfig},
    GoogleConnectorConfig,
};
use std::sync::Arc;
use tracing::{error, info};

mod admin;
mod api;
mod auth;
mod cache;
mod drive;
mod gmail;
mod models;
mod sync;

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

    let redis_client = redis::Client::open(config.base.redis.redis_url)?;

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
    ));

    // Create API state with shared services
    let api_state = ApiState {
        sync_manager: Arc::clone(&sync_manager),
        admin_client: Arc::clone(&admin_client),
        active_syncs: Arc::new(dashmap::DashSet::new()),
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

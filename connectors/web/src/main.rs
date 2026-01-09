use anyhow::Result;
use dashmap::DashSet;
use dotenvy::dotenv;
use omni_web_connector::api::{create_router, ApiState};
use omni_web_connector::sync::SyncManager;
use shared::telemetry::{self, TelemetryConfig};
use shared::{DatabasePool, SdkClient};
use std::sync::Arc;
use tracing::info;

#[tokio::main]
async fn main() -> Result<()> {
    dotenv().ok();

    let telemetry_config = TelemetryConfig::from_env("omni-web-connector");
    telemetry::init_telemetry(telemetry_config)?;

    info!("Starting Web Connector");

    // Connect to database
    let database_url = std::env::var("DATABASE_URL")?;
    let db_pool = DatabasePool::new(&database_url).await?;

    let redis_url =
        std::env::var("REDIS_URL").unwrap_or_else(|_| "redis://localhost:6379".to_string());
    let redis_client = redis::Client::open(redis_url)?;

    let sdk_client = SdkClient::from_env()?;

    let sync_manager = Arc::new(SyncManager::new(
        db_pool.pool().clone(),
        redis_client,
        sdk_client,
    ));

    let api_state = ApiState {
        sync_manager: Arc::clone(&sync_manager),
        active_syncs: Arc::new(DashSet::new()),
    };

    let app = create_router(api_state);
    let port = std::env::var("PORT")?.parse::<u16>()?;
    let addr = std::net::SocketAddr::from(([0, 0, 0, 0], port));
    let listener = tokio::net::TcpListener::bind(addr).await?;

    info!("HTTP server listening on {}", addr);

    axum::serve(listener, app).await?;

    Ok(())
}

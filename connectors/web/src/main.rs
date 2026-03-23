use anyhow::{Context, Result};
use dotenvy::dotenv;
use omni_web_connector::api::{build_manifest, create_router, ApiState};
use omni_web_connector::sync::SyncManager;
use shared::telemetry::{self, TelemetryConfig};
use shared::SdkClient;
use std::sync::Arc;
use tracing::info;

fn get_env(name: &str) -> Result<String> {
    std::env::var(name).with_context(|| format!("{} environment variable not set", name))
}

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

    let sync_manager = Arc::new(SyncManager::new(redis_client, sdk_client));

    let api_state = ApiState {
        sync_manager: Arc::clone(&sync_manager),
    };

    shared::start_registration_loop(build_manifest(None));

    let app = create_router(api_state);
    let port = get_env("PORT")?
        .parse::<u16>()
        .context("PORT must be a valid number")?;
    let addr = std::net::SocketAddr::from(([0, 0, 0, 0], port));
    let listener = tokio::net::TcpListener::bind(addr).await?;

    info!("HTTP server listening on {}", addr);

    axum::serve(listener, app).await?;

    Ok(())
}

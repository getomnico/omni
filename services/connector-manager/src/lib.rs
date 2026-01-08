pub mod config;
pub mod connector_client;
pub mod handlers;
pub mod models;
pub mod scheduler;
pub mod sync_manager;

use anyhow::Result as AnyhowResult;
use axum::{
    middleware,
    routing::{get, post},
    Router,
};
use config::ConnectorManagerConfig;
use shared::{
    telemetry::{self, TelemetryConfig},
    DatabasePool,
};
use std::net::SocketAddr;
use std::sync::Arc;
use sync_manager::SyncManager;
use tower::ServiceBuilder;
use tower_http::{cors::CorsLayer, trace::TraceLayer};
use tracing::info;

#[derive(Clone)]
pub struct AppState {
    pub db_pool: DatabasePool,
    pub config: ConnectorManagerConfig,
    pub sync_manager: Arc<SyncManager>,
}

pub fn create_app(state: AppState) -> Router {
    Router::new()
        .route("/health", get(handlers::health_check))
        .route("/sync", post(handlers::trigger_sync))
        .route("/sync/:id/cancel", post(handlers::cancel_sync))
        .route("/sync/:id/progress", get(handlers::get_sync_progress))
        .route("/schedules", get(handlers::list_schedules))
        .route("/connectors", get(handlers::list_connectors))
        .route("/action", post(handlers::execute_action))
        .route("/actions", get(handlers::list_actions))
        .layer(
            ServiceBuilder::new()
                .layer(middleware::from_fn(telemetry::middleware::trace_layer))
                .layer(TraceLayer::new_for_http())
                .layer(CorsLayer::permissive()),
        )
        .with_state(state)
}

pub async fn run_server() -> AnyhowResult<()> {
    dotenvy::dotenv().ok();

    let telemetry_config = TelemetryConfig::from_env("omni-connector-manager");
    telemetry::init_telemetry(telemetry_config)?;

    info!("Connector Manager service starting...");

    let config = ConnectorManagerConfig::from_env();
    info!("Configuration loaded");
    info!(
        "Registered connectors: {:?}",
        config.connector_urls.keys().collect::<Vec<_>>()
    );

    let db_pool = DatabasePool::from_config(&config.database)
        .await
        .map_err(|e| anyhow::anyhow!("Failed to create database pool: {}", e))?;
    info!("Database pool initialized");

    let sync_manager = Arc::new(SyncManager::new(&db_pool, config.clone()));

    let app_state = AppState {
        db_pool: db_pool.clone(),
        config: config.clone(),
        sync_manager: sync_manager.clone(),
    };

    // Start scheduler in background
    let scheduler = scheduler::Scheduler::new(db_pool.pool().clone(), config.clone(), sync_manager);
    tokio::spawn(async move {
        scheduler.run().await;
    });
    info!("Scheduler started");

    let app = create_app(app_state);

    let addr = SocketAddr::from(([0, 0, 0, 0], config.port));
    info!("Connector Manager service listening on {}", addr);

    let listener = tokio::net::TcpListener::bind(addr).await?;
    axum::serve(listener, app).await?;

    Ok(())
}

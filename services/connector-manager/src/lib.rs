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
    DatabasePool, ObjectStorage,
};
use std::net::SocketAddr;
use std::sync::Arc;
use sync_manager::SyncManager;
use tower::ServiceBuilder;
use tower_http::cors::CorsLayer;
use tracing::info;

#[derive(Clone)]
pub struct AppState {
    pub db_pool: DatabasePool,
    pub config: ConnectorManagerConfig,
    pub sync_manager: Arc<SyncManager>,
    pub content_storage: Arc<dyn ObjectStorage>,
}

pub fn create_app(state: AppState) -> Router {
    Router::new()
        // Health and management endpoints
        .route("/health", get(handlers::health_check))
        .route("/sync", post(handlers::trigger_sync))
        .route("/sync/:source_id", post(handlers::trigger_sync_by_id))
        .route("/sync/:id/cancel", post(handlers::cancel_sync))
        .route("/sync/:id/progress", get(handlers::get_sync_progress))
        .route("/schedules", get(handlers::list_schedules))
        .route("/connectors", get(handlers::list_connectors))
        .route("/action", post(handlers::execute_action))
        .route("/actions", get(handlers::list_actions))
        // SDK endpoints - called by connectors
        .route("/sdk/events", post(handlers::sdk_emit_event))
        .route("/sdk/content", post(handlers::sdk_store_content))
        .route("/sdk/sync/:id/heartbeat", post(handlers::sdk_heartbeat))
        .route("/sdk/sync/:id/complete", post(handlers::sdk_complete))
        .route("/sdk/sync/:id/fail", post(handlers::sdk_fail))
        .route(
            "/sdk/sync/:id/scanned",
            post(handlers::sdk_increment_scanned),
        )
        .route("/sdk/source/:source_id", get(handlers::sdk_get_source))
        .route(
            "/sdk/credentials/:source_id",
            get(handlers::sdk_get_credentials),
        )
        .route(
            "/sdk/source/:source_id/sync-config",
            get(handlers::sdk_get_source_sync_config),
        )
        .route("/sdk/sync/create", post(handlers::sdk_create_sync))
        .route("/sdk/sync/cancel", post(handlers::sdk_cancel_sync))
        // User email endpoint
        .route(
            "/sdk/source/:source_id/user-email",
            get(handlers::sdk_get_user_email),
        )
        // Webhook notification endpoint
        .route("/sdk/webhook/notify", post(handlers::sdk_notify_webhook))
        // Webhook channel management
        .route(
            "/sdk/webhook/channel",
            post(handlers::sdk_save_webhook_channel),
        )
        .route(
            "/sdk/webhook/channel/:channel_id",
            get(handlers::sdk_get_webhook_channel_by_id),
        )
        .route(
            "/sdk/webhook/channel/:channel_id",
            axum::routing::delete(handlers::sdk_delete_webhook_channel),
        )
        .route(
            "/sdk/webhook/channel/by-source/:source_id",
            get(handlers::sdk_get_webhook_channel_by_source),
        )
        .route(
            "/sdk/webhook/channels/expiring",
            post(handlers::sdk_get_expiring_webhook_channels),
        )
        .layer(
            ServiceBuilder::new()
                .layer(middleware::from_fn(telemetry::middleware::trace_layer))
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

    let content_storage = shared::StorageFactory::from_env(db_pool.pool().clone())
        .await
        .map_err(|e| anyhow::anyhow!("Failed to create content storage: {}", e))?;
    info!("Content storage initialized");

    let sync_manager = Arc::new(SyncManager::new(&db_pool, config.clone()));

    let app_state = AppState {
        db_pool: db_pool.clone(),
        config: config.clone(),
        sync_manager: sync_manager.clone(),
        content_storage,
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

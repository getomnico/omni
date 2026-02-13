use axum::{
    extract::State,
    http::StatusCode,
    middleware,
    response::{IntoResponse, Json},
    routing::{get, post},
    Router,
};
use serde_json::json;
use shared::telemetry;
use std::sync::Arc;
use tower::ServiceBuilder;
use tower_http::cors::CorsLayer;
use tracing::{error, info};

use crate::models::{
    ActionRequest, ActionResponse, CancelRequest, CancelResponse, ConnectorManifest, SyncRequest,
    SyncResponse, SyncResponseExt,
};
use crate::sync::SyncManager;

#[derive(Clone)]
pub struct ApiState {
    pub sync_manager: Arc<SyncManager>,
}

pub fn create_router(state: ApiState) -> Router {
    Router::new()
        .route("/health", get(health))
        .route("/manifest", get(manifest))
        .route("/sync", post(trigger_sync))
        .route("/cancel", post(cancel_sync))
        .route("/action", post(execute_action))
        .layer(
            ServiceBuilder::new()
                .layer(middleware::from_fn(telemetry::middleware::trace_layer))
                .layer(CorsLayer::permissive()),
        )
        .with_state(state)
}

async fn health() -> impl IntoResponse {
    Json(json!({ "status": "healthy", "service": "web-connector" }))
}

async fn manifest() -> impl IntoResponse {
    let manifest = ConnectorManifest {
        name: "web".to_string(),
        version: "1.0.0".to_string(),
        sync_modes: vec!["full".to_string()],
        actions: vec![], // Web connector has no actions
    };
    Json(manifest)
}

async fn trigger_sync(
    State(state): State<ApiState>,
    Json(request): Json<SyncRequest>,
) -> Result<Json<SyncResponse>, (StatusCode, Json<SyncResponse>)> {
    let sync_run_id = request.sync_run_id.clone();
    let source_id = request.source_id.clone();

    info!(
        "Sync triggered for source {} (sync_run_id: {})",
        source_id, sync_run_id
    );

    let sync_manager = state.sync_manager.clone();

    tokio::spawn(async move {
        if let Err(e) = sync_manager.sync_source(request).await {
            error!("Sync {} failed: {}", sync_run_id, e);
        }
    });

    Ok(Json(SyncResponse::started()))
}

async fn cancel_sync(
    State(state): State<ApiState>,
    Json(request): Json<CancelRequest>,
) -> impl IntoResponse {
    info!("Cancel requested for sync {}", request.sync_run_id);

    let cancelled = state.sync_manager.cancel_sync(&request.sync_run_id);

    Json(CancelResponse {
        status: if cancelled { "cancelled" } else { "not_found" }.to_string(),
    })
}

async fn execute_action(Json(request): Json<ActionRequest>) -> impl IntoResponse {
    info!("Action requested: {}", request.action);

    // Web connector doesn't support any actions
    Json(ActionResponse::not_supported(&request.action))
}

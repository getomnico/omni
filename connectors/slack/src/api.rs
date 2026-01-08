use axum::{
    extract::{Path, State},
    http::StatusCode,
    response::{IntoResponse, Json},
    routing::{get, post},
    Router,
};
use serde_json::json;
use std::sync::Arc;
use tower::ServiceBuilder;
use tower_http::{cors::CorsLayer, trace::TraceLayer};
use tracing::{error, info};

use crate::models::{
    ActionRequest, ActionResponse, CancelRequest, CancelResponse, ConnectorManifest, SyncResponse,
};
use crate::sync::SyncManager;

#[derive(Clone)]
pub struct ApiState {
    pub sync_manager: Arc<SyncManager>,
}

pub fn create_router(state: ApiState) -> Router {
    Router::new()
        // Protocol endpoints
        .route("/health", get(health))
        .route("/manifest", get(manifest))
        .route("/sync", post(sync_all))
        .route("/sync/:source_id", post(sync_source))
        .route("/cancel", post(cancel_sync))
        .route("/action", post(execute_action))
        .layer(
            ServiceBuilder::new()
                .layer(TraceLayer::new_for_http())
                .layer(CorsLayer::permissive()),
        )
        .with_state(state)
}

async fn health() -> impl IntoResponse {
    Json(json!({
        "status": "healthy",
        "service": "slack-connector"
    }))
}

async fn manifest() -> impl IntoResponse {
    let manifest = ConnectorManifest {
        name: "slack".to_string(),
        version: "1.0.0".to_string(),
        sync_modes: vec!["full".to_string(), "incremental".to_string()],
        actions: vec![], // Slack connector has no actions yet
    };
    Json(manifest)
}

async fn sync_all(State(state): State<ApiState>) -> Result<Json<SyncResponse>, StatusCode> {
    info!("Manual sync triggered for all sources");

    let sync_manager = state.sync_manager.clone();

    tokio::spawn(async move {
        if let Err(e) = sync_manager.sync_all_sources().await {
            error!("Manual sync failed: {}", e);
        }
    });

    Ok(Json(SyncResponse::started()))
}

async fn sync_source(
    Path(source_id): Path<String>,
    State(state): State<ApiState>,
) -> Result<Json<SyncResponse>, StatusCode> {
    info!("Manual sync triggered for source: {}", source_id);

    let sync_manager = state.sync_manager.clone();
    let source_id_clone = source_id.clone();

    tokio::spawn(async move {
        if let Err(e) = sync_manager
            .sync_source_by_id(source_id_clone.clone())
            .await
        {
            error!("Manual sync failed for source {}: {}", source_id_clone, e);
        }
    });

    Ok(Json(SyncResponse::started()))
}

async fn cancel_sync(Json(request): Json<CancelRequest>) -> impl IntoResponse {
    info!("Cancel requested for sync {}", request.sync_run_id);

    // Slack connector doesn't support cancellation yet
    Json(CancelResponse {
        status: "not_supported".to_string(),
    })
}

async fn execute_action(Json(request): Json<ActionRequest>) -> impl IntoResponse {
    info!("Action requested: {}", request.action);

    // Slack connector doesn't support any actions yet
    Json(ActionResponse::not_supported(&request.action))
}

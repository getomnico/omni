use axum::{
    extract::{Path, State},
    http::StatusCode,
    response::Json,
    routing::{get, post},
    Router,
};
use serde::Serialize;
use std::sync::Arc;
use tower::ServiceBuilder;
use tower_http::{cors::CorsLayer, trace::TraceLayer};
use tracing::{error, info};

use crate::sync::SyncManager;

#[derive(Clone)]
pub struct ApiState {
    pub sync_manager: Arc<SyncManager>,
}

#[derive(Serialize)]
pub struct HealthResponse {
    status: String,
    service: String,
}

#[derive(Serialize)]
pub struct SyncResponse {
    message: String,
    source_id: String,
}

#[derive(Serialize)]
pub struct ErrorResponse {
    error: String,
}

pub fn create_router(state: ApiState) -> Router {
    Router::new()
        .route("/health", get(health))
        .route("/sync", post(sync_all))
        .route("/sync/:source_id", post(sync_source))
        .layer(
            ServiceBuilder::new()
                .layer(TraceLayer::new_for_http())
                .layer(CorsLayer::permissive()),
        )
        .with_state(state)
}

async fn health() -> Json<HealthResponse> {
    Json(HealthResponse {
        status: "healthy".to_string(),
        service: "web-connector".to_string(),
    })
}

async fn sync_all(State(state): State<ApiState>) -> Result<Json<SyncResponse>, StatusCode> {
    info!("Manual sync triggered for all sources");

    match state.sync_manager.sync_all_sources().await {
        Ok(_) => Ok(Json(SyncResponse {
            message: "Sync completed successfully".to_string(),
            source_id: "all".to_string(),
        })),
        Err(e) => {
            error!("Manual sync failed: {}", e);
            Err(StatusCode::INTERNAL_SERVER_ERROR)
        }
    }
}

async fn sync_source(
    Path(source_id): Path<String>,
    State(state): State<ApiState>,
) -> Result<Json<SyncResponse>, StatusCode> {
    info!("Manual sync triggered for source: {}", source_id);

    match state.sync_manager.sync_source_by_id(&source_id).await {
        Ok(_) => Ok(Json(SyncResponse {
            message: "Sync completed successfully".to_string(),
            source_id,
        })),
        Err(e) => {
            error!("Manual sync failed for source {}: {}", source_id, e);
            Err(StatusCode::INTERNAL_SERVER_ERROR)
        }
    }
}

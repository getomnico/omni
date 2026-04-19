use axum::{
    extract::State,
    http::StatusCode,
    middleware,
    response::{IntoResponse, Json},
    routing::{get, post},
    Router,
};
use serde::{Deserialize, Serialize};
use serde_json::json;
use shared::models::{SourceType, SyncRequest, SyncType};
use shared::telemetry;
use std::sync::Arc;
use tokio::sync::Mutex;
use tower::ServiceBuilder;
use tower_http::cors::CorsLayer;
use tracing::{error, info};

use crate::sync::SyncManager;

#[derive(Clone)]
pub struct ApiState {
    pub sync_manager: Arc<Mutex<SyncManager>>,
}

use shared::models::ConnectorManifest;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SyncResponse {
    pub status: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
}

impl SyncResponse {
    pub fn started() -> Self {
        Self {
            status: "started".to_string(),
            message: None,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CancelRequest {
    pub sync_run_id: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CancelResponse {
    pub status: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ActionRequest {
    pub action: String,
    pub params: serde_json::Value,
    pub credentials: serde_json::Value,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ActionResponse {
    pub status: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
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
    Json(json!({
        "status": "healthy",
        "version": env!("CARGO_PKG_VERSION"),
        "service": "fireflies-connector"
    }))
}

pub fn build_manifest(connector_url: String) -> ConnectorManifest {
    ConnectorManifest {
        name: "fireflies".to_string(),
        display_name: "Fireflies".to_string(),
        version: env!("CARGO_PKG_VERSION").to_string(),
        sync_modes: vec![SyncType::Full, SyncType::Incremental],
        connector_id: "fireflies".to_string(),
        connector_url,
        source_types: vec![SourceType::Fireflies],
        description: Some("Index meeting transcripts from Fireflies.ai".to_string()),
        actions: vec![],
        search_operators: vec![],
        read_only: false,
        extra_schema: None,
        attributes_schema: None,
        mcp_enabled: false,
        resources: vec![],
        prompts: vec![],
    }
}

async fn manifest() -> impl IntoResponse {
    Json(build_manifest(shared::build_connector_url()))
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
        let mut manager = sync_manager.lock().await;
        if let Err(e) = manager.sync_source(request).await {
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

    let sync_manager = state.sync_manager.lock().await;
    let cancelled = sync_manager.cancel_sync(&request.sync_run_id);

    Json(CancelResponse {
        status: if cancelled { "cancelled" } else { "not_found" }.to_string(),
    })
}

async fn execute_action(Json(request): Json<ActionRequest>) -> impl IntoResponse {
    info!("Action requested: {}", request.action);

    Json(ActionResponse {
        status: "error".to_string(),
        error: Some(format!("Action not supported: {}", request.action)),
    })
}

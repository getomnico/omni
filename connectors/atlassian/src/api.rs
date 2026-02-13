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
use shared::models::SyncRequest;
use shared::telemetry;
use std::sync::Arc;
use tokio::sync::Mutex;
use tower::ServiceBuilder;
use tower_http::cors::CorsLayer;
use tracing::{error, info};

use crate::models::{
    ActionRequest, ActionResponse, CancelRequest, CancelResponse, ConnectorManifest, SyncResponse,
};
use crate::sync::SyncManager;

#[derive(Clone)]
pub struct ApiState {
    pub sync_manager: Arc<Mutex<SyncManager>>,
}

#[derive(Serialize)]
pub struct HealthResponse {
    pub status: String,
    pub version: String,
    pub service: String,
}

#[derive(Deserialize)]
pub struct TestConnectionRequest {
    pub base_url: String,
    pub user_email: String,
    pub api_token: String,
}

#[derive(Serialize)]
pub struct TestConnectionResponse {
    pub success: bool,
    pub message: String,
    pub jira_projects: Vec<String>,
    pub confluence_spaces: Vec<String>,
}

#[derive(Serialize)]
pub struct ErrorResponse {
    pub error: String,
}

pub fn create_router(state: ApiState) -> Router {
    Router::new()
        // Protocol endpoints
        .route("/health", get(health))
        .route("/manifest", get(manifest))
        .route("/sync", post(trigger_sync))
        .route("/cancel", post(cancel_sync))
        .route("/action", post(execute_action))
        // Admin endpoints
        .route("/test-connection", post(test_connection))
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
        "service": "atlassian-connector"
    }))
}

async fn manifest() -> impl IntoResponse {
    let manifest = ConnectorManifest {
        name: "atlassian".to_string(),
        version: "1.0.0".to_string(),
        sync_modes: vec!["full".to_string(), "incremental".to_string()],
        actions: vec![], // Atlassian connector has no actions yet
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

    // Atlassian connector doesn't support any actions yet
    Json(ActionResponse::not_supported(&request.action))
}

async fn test_connection(
    State(state): State<ApiState>,
    Json(request): Json<TestConnectionRequest>,
) -> Result<Json<TestConnectionResponse>, (StatusCode, Json<ErrorResponse>)> {
    info!("Testing connection to Atlassian: {}", request.base_url);

    let config = (request.base_url, request.user_email, request.api_token);

    let sync_manager = state.sync_manager.lock().await;
    match sync_manager.test_connection(&config).await {
        Ok((jira_projects, confluence_spaces)) => {
            info!(
                "Connection test successful: {} JIRA projects, {} Confluence spaces",
                jira_projects.len(),
                confluence_spaces.len()
            );
            Ok(Json(TestConnectionResponse {
                success: true,
                message: format!(
                    "Successfully connected. Found {} JIRA projects and {} Confluence spaces.",
                    jira_projects.len(),
                    confluence_spaces.len()
                ),
                jira_projects,
                confluence_spaces,
            }))
        }
        Err(e) => {
            error!("Connection test failed: {}", e);
            Ok(Json(TestConnectionResponse {
                success: false,
                message: format!("Connection failed: {}", e),
                jira_projects: vec![],
                confluence_spaces: vec![],
            }))
        }
    }
}

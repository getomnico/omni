use axum::{
    extract::{Path, State},
    http::StatusCode,
    response::{IntoResponse, Json},
    routing::{get, post},
    Router,
};
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::sync::Arc;
use tokio::sync::Mutex;
use tower::ServiceBuilder;
use tower_http::{cors::CorsLayer, trace::TraceLayer};
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
        .route("/sync/:source_id", post(trigger_sync))
        .route("/sync", post(trigger_full_sync))
        .route("/cancel", post(cancel_sync))
        .route("/action", post(execute_action))
        // Admin endpoints
        .route("/test-connection", post(test_connection))
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
    Path(source_id): Path<String>,
) -> Json<SyncResponse> {
    info!("Received sync request for source: {}", source_id);

    let sync_manager = state.sync_manager.clone();
    let source_id_clone = source_id.clone();

    tokio::spawn(async move {
        let mut manager = sync_manager.lock().await;
        match manager.sync_source_by_id(source_id_clone.clone()).await {
            Ok(_) => {
                info!(
                    "Successfully completed sync for source: {}",
                    source_id_clone
                );
            }
            Err(e) => {
                error!(
                    "Failed to complete sync for source {}: {}",
                    source_id_clone, e
                );
            }
        }
    });

    Json(SyncResponse::started())
}

async fn trigger_full_sync(State(state): State<ApiState>) -> Json<SyncResponse> {
    info!("Manual sync triggered via API");

    let sync_manager = state.sync_manager.clone();

    tokio::spawn(async move {
        let mut manager = sync_manager.lock().await;
        match manager.sync_all_sources().await {
            Ok(()) => {
                info!("Manual sync completed successfully");
            }
            Err(e) => {
                error!("Manual sync failed: {}", e);
            }
        }
    });

    Json(SyncResponse::started())
}

async fn cancel_sync(Json(request): Json<CancelRequest>) -> impl IntoResponse {
    info!("Cancel requested for sync {}", request.sync_run_id);

    // Atlassian connector doesn't support cancellation yet
    Json(CancelResponse {
        status: "not_supported".to_string(),
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

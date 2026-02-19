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

use crate::auth::AtlassianCredentials;
use crate::client::AtlassianClient;
use crate::models::{
    ActionDefinition, ActionRequest, ActionResponse, CancelRequest, CancelResponse,
    ConnectorManifest, SyncResponse,
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
        actions: vec![ActionDefinition {
            name: "search_spaces".to_string(),
            description: "Search Confluence spaces or Jira projects".to_string(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "query": { "type": "string", "description": "Search query to filter by name or key" },
                    "type": { "type": "string", "enum": ["confluence", "jira"], "description": "Whether to search Confluence spaces or Jira projects" }
                },
                "required": ["type"]
            }),
        }],
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

    match request.action.as_str() {
        "search_spaces" => Json(handle_search_spaces(request.params, request.credentials).await),
        _ => Json(ActionResponse::not_supported(&request.action)),
    }
}

async fn handle_search_spaces(
    params: serde_json::Value,
    credentials: serde_json::Value,
) -> ActionResponse {
    let query = params
        .get("query")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_lowercase();
    let search_type = match params.get("type").and_then(|v| v.as_str()) {
        Some(t) => t.to_string(),
        None => return ActionResponse::error("Missing required parameter: type"),
    };

    let base_url = match credentials
        .get("config")
        .and_then(|c| c.get("base_url"))
        .and_then(|v| v.as_str())
    {
        Some(u) => u.to_string(),
        None => return ActionResponse::error("Missing base_url in credentials config"),
    };
    let user_email = match credentials.get("principal_email").and_then(|v| v.as_str()) {
        Some(e) => e.to_string(),
        None => return ActionResponse::error("Missing principal_email in credentials"),
    };
    let api_token = match credentials
        .get("credentials")
        .and_then(|c| c.get("api_token"))
        .and_then(|v| v.as_str())
    {
        Some(t) => t.to_string(),
        None => return ActionResponse::error("Missing api_token in credentials"),
    };

    let creds = AtlassianCredentials::new(base_url, user_email, api_token);
    let client = AtlassianClient::new();

    match search_type.as_str() {
        "confluence" => match client.get_confluence_spaces(&creds).await {
            Ok(spaces) => {
                let results: Vec<serde_json::Value> = spaces
                    .into_iter()
                    .filter(|s| {
                        s.r#type != "personal"
                            && (query.is_empty()
                                || s.key.to_lowercase().contains(&query)
                                || s.name.to_lowercase().contains(&query))
                    })
                    .map(|s| {
                        json!({
                            "key": s.key,
                            "name": s.name,
                            "type": "confluence"
                        })
                    })
                    .collect();
                ActionResponse::success(json!(results))
            }
            Err(e) => ActionResponse::error(format!("Failed to fetch Confluence spaces: {}", e)),
        },
        "jira" => match client.get_jira_projects(&creds, &[]).await {
            Ok(projects) => {
                let results: Vec<serde_json::Value> = projects
                    .into_iter()
                    .filter(|p| {
                        if query.is_empty() {
                            return true;
                        }
                        let key = p
                            .get("key")
                            .and_then(|v| v.as_str())
                            .unwrap_or("")
                            .to_lowercase();
                        let name = p
                            .get("name")
                            .and_then(|v| v.as_str())
                            .unwrap_or("")
                            .to_lowercase();
                        key.contains(&query) || name.contains(&query)
                    })
                    .map(|p| {
                        json!({
                            "key": p.get("key").and_then(|v| v.as_str()).unwrap_or(""),
                            "name": p.get("name").and_then(|v| v.as_str()).unwrap_or(""),
                            "type": "jira"
                        })
                    })
                    .collect();
                ActionResponse::success(json!(results))
            }
            Err(e) => ActionResponse::error(format!("Failed to fetch Jira projects: {}", e)),
        },
        _ => ActionResponse::error(format!(
            "Invalid type: {}. Must be 'confluence' or 'jira'",
            search_type
        )),
    }
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

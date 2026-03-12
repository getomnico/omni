use anyhow::anyhow;
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
use tower::ServiceBuilder;
use tower_http::cors::CorsLayer;
use tracing::{error, info};

use crate::client::ImapSession;
use crate::config::ImapAccountConfig;
use crate::sync::SyncManager;

#[derive(Clone)]
pub struct ApiState {
    pub sync_manager: Arc<SyncManager>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConnectorManifest {
    pub name: String,
    pub version: String,
    pub sync_modes: Vec<String>,
    pub read_only: bool,
    #[serde(default)]
    pub search_operators: Vec<shared::models::SearchOperator>,
}

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
    pub data: Option<serde_json::Value>,
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
        "service": "imap-connector"
    }))
}

async fn manifest() -> impl IntoResponse {
    Json(ConnectorManifest {
        name: "imap".to_string(),
        version: env!("CARGO_PKG_VERSION").to_string(),
        sync_modes: vec!["full".to_string(), "incremental".to_string()],
        read_only: true,
        search_operators: vec![],
    })
}

async fn trigger_sync(
    State(state): State<ApiState>,
    Json(request): Json<SyncRequest>,
) -> Result<Json<SyncResponse>, (StatusCode, Json<SyncResponse>)> {
    let sync_run_id = request.sync_run_id.clone();
    let source_id = request.source_id.clone();

    info!(
        "IMAP sync triggered for source {} (sync_run_id: {})",
        source_id, sync_run_id
    );

    let sync_manager = Arc::clone(&state.sync_manager);

    tokio::spawn(async move {
        if let Err(e) = sync_manager.sync_source(request).await {
            error!("IMAP sync {} failed: {}", sync_run_id, e);
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
    info!("IMAP action requested: {}", request.action);
    Json(match run_action(&request).await {
        Ok(data) => ActionResponse {
            status: "ok".into(),
            data: Some(data),
            error: None,
        },
        Err(e) => ActionResponse {
            status: "error".into(),
            data: None,
            error: Some(e.to_string()),
        },
    })
}

async fn run_action(request: &ActionRequest) -> anyhow::Result<serde_json::Value> {
    match request.action.as_str() {
        "validate_credentials" | "list_folders" => {
            let mut session = open_session(request).await?;
            let result = if request.action == "validate_credentials" {
                Ok(json!({ "authenticated": true }))
            } else {
                session
                    .list_folders()
                    .await
                    .map(|f| json!({ "folders": f }))
            };
            session.logout().await;
            result
        }
        other => Err(anyhow!("Action not supported: {}", other)),
    }
}

async fn open_session(request: &ActionRequest) -> anyhow::Result<ImapSession> {
    let config = ImapAccountConfig::from_source_config(&request.params)?;
    let username = request
        .credentials
        .get("username")
        .and_then(|v| v.as_str())
        .ok_or_else(|| anyhow!("Missing 'username' in credentials"))?
        .to_string();
    let password = request
        .credentials
        .get("password")
        .and_then(|v| v.as_str())
        .ok_or_else(|| anyhow!("Missing 'password' in credentials"))?
        .to_string();
    ImapSession::connect(&config, &username, &password).await
}

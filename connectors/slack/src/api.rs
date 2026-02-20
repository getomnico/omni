use axum::{
    extract::State,
    http::StatusCode,
    middleware,
    response::{IntoResponse, Json},
    routing::{get, post},
    Router,
};
use dashmap::DashSet;
use serde_json::json;
use shared::models::SyncRequest;
use shared::telemetry;
use std::sync::Arc;
use tower::ServiceBuilder;
use tower_http::cors::CorsLayer;
use tracing::{debug, error, info};

use omni_slack_connector::models::{
    ActionRequest, ActionResponse, CancelRequest, CancelResponse, ConnectorManifest, SyncResponse,
};
use omni_slack_connector::sync::SyncManager;

use crate::socket::SocketModeManager;

#[derive(Clone)]
pub struct ApiState {
    pub sync_manager: Arc<SyncManager>,
    pub active_syncs: Arc<DashSet<String>>,
    pub socket_manager: Arc<SocketModeManager>,
}

pub fn create_router(state: ApiState) -> Router {
    Router::new()
        // Protocol endpoints
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

    // Check if already syncing this source
    if state.active_syncs.contains(&source_id) {
        return Err((
            StatusCode::CONFLICT,
            Json(SyncResponse::error(
                "Sync already in progress for this source",
            )),
        ));
    }

    // Mark as active
    state.active_syncs.insert(source_id.clone());

    // Spawn sync task
    let sync_manager = state.sync_manager.clone();
    let active_syncs = state.active_syncs.clone();
    let socket_manager = state.socket_manager.clone();
    tokio::spawn(async move {
        let result = sync_manager.sync_source_from_request(request).await;

        // Remove from active syncs when done
        active_syncs.remove(&source_id);

        match result {
            Ok(()) => {
                if !socket_manager.is_connected(&source_id).await {
                    maybe_start_socket_with_sync(
                        &source_id,
                        sync_manager.sdk_client(),
                        &socket_manager,
                        Some(sync_manager.clone()),
                    )
                    .await;
                }
            }
            Err(e) => {
                error!("Sync {} failed: {}", sync_run_id, e);
            }
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

    // Slack connector doesn't support any actions yet
    Json(ActionResponse::not_supported(&request.action))
}

pub async fn maybe_start_socket_with_sync(
    source_id: &str,
    sdk_client: &shared::SdkClient,
    socket_manager: &SocketModeManager,
    sync_manager: Option<Arc<SyncManager>>,
) {
    let app_token = match get_app_token(source_id, sdk_client).await {
        Some(token) => token,
        None => return,
    };

    info!(source_id, "Starting Socket Mode connection");
    socket_manager
        .start_connection(
            source_id.to_string(),
            app_token,
            sdk_client.clone(),
            sync_manager,
        )
        .await;
}

async fn get_app_token(source_id: &str, sdk_client: &shared::SdkClient) -> Option<String> {
    let creds = match sdk_client.get_credentials(source_id).await {
        Ok(c) => c,
        Err(e) => {
            debug!("Could not fetch credentials for {}: {}", source_id, e);
            return None;
        }
    };

    creds
        .credentials
        .get("app_token")
        .and_then(|v| v.as_str())
        .map(|s| s.to_string())
}

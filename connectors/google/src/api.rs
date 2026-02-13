use axum::{
    extract::{Path, Query, State},
    http::{HeaderMap, StatusCode},
    middleware,
    response::{IntoResponse, Json},
    routing::{get, post},
    Json as JsonExtractor, Router,
};
use dashmap::DashSet;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use shared::telemetry;
use std::sync::Arc;
use tower::ServiceBuilder;
use tower_http::cors::CorsLayer;
use tracing::{debug, error, info, warn};

use crate::admin::AdminClient;
use crate::auth::ServiceAccountAuth;
use crate::models::{
    ActionRequest, ActionResponse, CancelRequest, CancelResponse, ConnectorManifest, SyncRequest,
    SyncResponse, SyncResponseExt, WebhookNotification,
};
use crate::sync::SyncManager;
use shared::models::{ServiceProvider, SourceType};

#[derive(Clone)]
pub struct ApiState {
    pub sync_manager: Arc<SyncManager>,
    pub admin_client: Arc<AdminClient>,
    pub active_syncs: Arc<DashSet<String>>,
}

pub fn create_router(state: ApiState) -> Router {
    Router::new()
        // Protocol endpoints
        .route("/health", get(health_check))
        .route("/manifest", get(manifest))
        .route("/sync", post(trigger_sync))
        .route("/cancel", post(cancel_sync))
        .route("/action", post(execute_action))
        // Webhook endpoints
        .route("/webhook", post(handle_webhook))
        .route("/webhook/register/:source_id", post(register_webhook))
        .route("/webhook/stop/:source_id", post(stop_webhook))
        // Admin endpoints
        .route("/users/search/:source_id", get(search_users))
        .layer(
            ServiceBuilder::new()
                .layer(middleware::from_fn(telemetry::middleware::trace_layer))
                .layer(CorsLayer::permissive()),
        )
        .with_state(state)
}

async fn health_check() -> impl IntoResponse {
    Json(json!({
        "status": "healthy",
        "service": "google-connector"
    }))
}

async fn manifest() -> impl IntoResponse {
    let manifest = ConnectorManifest {
        name: "google".to_string(),
        version: "1.0.0".to_string(),
        sync_modes: vec!["full".to_string(), "incremental".to_string()],
        actions: vec![], // Google connector has no actions yet
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

    tokio::spawn(async move {
        let result = sync_manager.sync_source_from_request(request).await;

        // Remove from active syncs when done
        active_syncs.remove(&source_id);

        if let Err(e) = result {
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

    // Google connector doesn't support any actions yet
    Json(ActionResponse::not_supported(&request.action))
}

async fn handle_webhook(
    State(state): State<ApiState>,
    headers: HeaderMap,
) -> Result<StatusCode, StatusCode> {
    debug!("Received webhook notification");

    // Parse webhook notification from headers
    let notification = match WebhookNotification::from_headers(&headers) {
        Some(notification) => notification,
        None => {
            warn!("Failed to parse webhook notification from headers");
            return Err(StatusCode::BAD_REQUEST);
        }
    };

    info!(
        "Processing webhook notification: channel_id={}, resource_state={}, resource_id={:?}",
        notification.channel_id, notification.resource_state, notification.resource_id
    );

    // Handle different resource states
    match notification.resource_state.as_str() {
        "sync" => {
            // This is a sync message, just acknowledge it
            debug!(
                "Received sync message for channel: {}",
                notification.channel_id
            );
        }
        "add" | "update" | "remove" | "trash" | "untrash" => {
            // Trigger incremental sync for the affected resource
            info!(
                "Triggering incremental sync for resource state: {}",
                notification.resource_state
            );

            let sync_manager = state.sync_manager.clone();
            let notification_clone = notification.clone();

            tokio::spawn(async move {
                if let Err(e) = sync_manager
                    .handle_webhook_notification(notification_clone)
                    .await
                {
                    error!("Failed to handle webhook notification: {}", e);
                }
            });
        }
        _ => {
            warn!(
                "Unknown resource state in webhook notification: {}",
                notification.resource_state
            );
        }
    }

    // Return success response to Google
    Ok(StatusCode::OK)
}

#[derive(Deserialize)]
struct RegisterWebhookRequest {
    webhook_url: String,
}

#[derive(Deserialize)]
struct StopWebhookRequest {
    channel_id: String,
    resource_id: String,
}

async fn register_webhook(
    State(state): State<ApiState>,
    Path(source_id): Path<String>,
    JsonExtractor(payload): JsonExtractor<RegisterWebhookRequest>,
) -> Result<Json<Value>, StatusCode> {
    info!("Registering webhook for source: {}", source_id);

    match state
        .sync_manager
        .register_webhook_for_source(&source_id, payload.webhook_url)
        .await
    {
        Ok(webhook_response) => Ok(Json(json!({
            "success": true,
            "message": "Webhook registered successfully",
            "channel_id": webhook_response.id,
            "resource_id": webhook_response.resource_id,
            "resource_uri": webhook_response.resource_uri
        }))),
        Err(e) => {
            error!("Failed to register webhook for source {}: {}", source_id, e);
            Err(StatusCode::INTERNAL_SERVER_ERROR)
        }
    }
}

async fn stop_webhook(
    State(state): State<ApiState>,
    Path(source_id): Path<String>,
    JsonExtractor(payload): JsonExtractor<StopWebhookRequest>,
) -> Result<Json<Value>, StatusCode> {
    info!("Stopping webhook for source: {}", source_id);

    match state
        .sync_manager
        .stop_webhook_for_source(&source_id, &payload.channel_id, &payload.resource_id)
        .await
    {
        Ok(()) => Ok(Json(json!({
            "success": true,
            "message": "Webhook stopped successfully"
        }))),
        Err(e) => {
            error!("Failed to stop webhook for source {}: {}", source_id, e);
            Err(StatusCode::INTERNAL_SERVER_ERROR)
        }
    }
}

#[derive(Debug, Deserialize)]
pub struct UserSearchQuery {
    q: Option<String>,          // Search query
    limit: Option<u32>,         // Max results (default 50, max 100)
    page_token: Option<String>, // Pagination token
}

#[derive(Debug, Serialize)]
pub struct UserSearchResponse {
    users: Vec<UserSearchResult>,
    next_page_token: Option<String>,
    has_more: bool,
}

#[derive(Debug, Serialize)]
pub struct UserSearchResult {
    id: String,
    email: String,
    name: String,
    org_unit: String,
    suspended: bool,
    is_admin: bool,
}

async fn search_users(
    State(state): State<ApiState>,
    Path(source_id): Path<String>,
    Query(params): Query<UserSearchQuery>,
) -> Result<Json<UserSearchResponse>, StatusCode> {
    info!("Searching users for source: {}", source_id);

    // Get credentials via SDK
    let creds = match state
        .sync_manager
        .sdk_client
        .get_credentials(&source_id)
        .await
    {
        Ok(creds) => creds,
        Err(e) => {
            error!("Failed to get credentials for source {}: {}", source_id, e);
            return Err(StatusCode::UNAUTHORIZED);
        }
    };

    // Verify it's a Google credential
    if creds.provider != ServiceProvider::Google {
        error!(
            "Expected Google credentials for source {}, found {:?}",
            source_id, creds.provider
        );
        return Err(StatusCode::BAD_REQUEST);
    }

    // Get the service account key and domain
    let service_account_key = match creds
        .credentials
        .get("service_account_key")
        .and_then(|v| v.as_str())
    {
        Some(key) => key,
        None => {
            error!(
                "Missing service_account_key in credentials for source {}",
                source_id
            );
            return Err(StatusCode::BAD_REQUEST);
        }
    };

    let domain = match creds.config.get("domain").and_then(|v| v.as_str()) {
        Some(d) => d.to_string(),
        None => {
            error!(
                "Missing domain in credentials config for source {}",
                source_id
            );
            return Err(StatusCode::BAD_REQUEST);
        }
    };

    // Get user email via SDK
    let principal_email = match state
        .sync_manager
        .sdk_client
        .get_user_email_for_source(&source_id)
        .await
    {
        Ok(email) => email,
        Err(e) => {
            error!("Failed to get user email for source {}: {}", source_id, e);
            return Err(StatusCode::INTERNAL_SERVER_ERROR);
        }
    };

    // Create auth with admin directory scopes
    let admin_scopes = crate::auth::get_scopes_for_source_type(SourceType::GoogleDrive);
    let auth = match ServiceAccountAuth::new(service_account_key, admin_scopes) {
        Ok(auth) => auth,
        Err(e) => {
            error!("Failed to create auth for source {}: {}", source_id, e);
            return Err(StatusCode::INTERNAL_SERVER_ERROR);
        }
    };

    // Get access token for the principal user (admin)
    let token = match auth.get_access_token(&principal_email).await {
        Ok(token) => token,
        Err(e) => {
            error!("Failed to get access token for source {}: {}", source_id, e);
            return Err(StatusCode::UNAUTHORIZED);
        }
    };

    // Validate and set limits
    let limit = params.limit.unwrap_or(50).min(100);
    let query = params.q.as_deref();
    let page_token = params.page_token.as_deref();

    // Use the admin client to search users
    match state
        .admin_client
        .search_users(&token, &domain, query, Some(limit), page_token)
        .await
    {
        Ok(response) => {
            let users: Vec<UserSearchResult> = response
                .users
                .unwrap_or_default()
                .into_iter()
                .map(|user| UserSearchResult {
                    id: user.id,
                    email: user.primary_email,
                    name: user
                        .name
                        .and_then(|n| n.full_name)
                        .unwrap_or_else(|| "Unknown".to_string()),
                    org_unit: user.org_unit_path.unwrap_or_else(|| "/".to_string()),
                    suspended: user.suspended.unwrap_or(false),
                    is_admin: user.is_admin.unwrap_or(false),
                })
                .collect();

            let has_more = response.next_page_token.is_some();

            Ok(Json(UserSearchResponse {
                users,
                next_page_token: response.next_page_token,
                has_more,
            }))
        }
        Err(e) => {
            error!("Failed to search users for source {}: {}", source_id, e);
            Err(StatusCode::INTERNAL_SERVER_ERROR)
        }
    }
}

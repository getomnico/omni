use axum::{
    extract::{Path, Query, State},
    http::{HeaderMap, StatusCode},
    response::Json,
    routing::{get, post},
    Json as JsonExtractor, Router,
};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::sync::Arc;
use tracing::{debug, error, info, warn};

use crate::admin::AdminClient;
use crate::auth::GoogleCredentialsService;
use crate::models::WebhookNotification;
use crate::sync::SyncManager;

#[derive(Clone)]
pub struct ApiState {
    pub sync_manager: Arc<SyncManager>,
    pub credentials_service: Arc<GoogleCredentialsService>,
    pub admin_client: Arc<AdminClient>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct SyncResponse {
    pub success: bool,
    pub message: String,
}

pub fn create_router(state: ApiState) -> Router {
    Router::new()
        .route("/health", get(health_check))
        .route("/sync/:source_id", post(trigger_sync))
        .route("/sync", post(trigger_full_sync))
        .route("/webhook", post(handle_webhook))
        // Manual webhook management endpoints (primarily for debugging/operations)
        // Note: Webhooks are automatically registered on startup if GOOGLE_WEBHOOK_URL is set
        .route("/webhook/register/:source_id", post(register_webhook))
        .route("/webhook/stop/:source_id", post(stop_webhook))
        .route("/users/search/:source_id", get(search_users))
        .with_state(state)
}

async fn health_check() -> Json<Value> {
    Json(json!({
        "status": "healthy",
        "service": "omni-google-connector"
    }))
}

async fn trigger_sync(
    State(state): State<ApiState>,
    Path(source_id): Path<String>,
) -> Json<SyncResponse> {
    info!("Received sync request for source: {}", source_id);

    let sync_manager = state.sync_manager.clone();
    let source_id_clone = source_id.clone();

    tokio::spawn(async move {
        match sync_manager
            .sync_source_by_id(source_id_clone.clone())
            .await
        {
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

    Json(SyncResponse {
        success: true,
        message: format!(
            "Sync triggered successfully for source: {}. Running in background.",
            source_id
        ),
    })
}

async fn trigger_full_sync(State(state): State<ApiState>) -> Json<SyncResponse> {
    info!("Received full sync request for all sources");

    let sync_manager = state.sync_manager.clone();

    tokio::spawn(async move {
        match sync_manager.sync_all_sources().await {
            Ok(_) => {
                info!("Successfully completed full sync for all sources");
            }
            Err(e) => {
                error!("Failed to complete full sync: {}", e);
            }
        }
    });

    Json(SyncResponse {
        success: true,
        message: "Full sync triggered successfully for all sources. Running in background."
            .to_string(),
    })
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

    // Get authentication setup for this source (admin operations only need directory scope)
    let (auth, domain, principal_email) = match state
        .credentials_service
        .setup_admin_auth_for_source(&source_id)
        .await
    {
        Ok(setup) => setup,
        Err(e) => {
            error!("Failed to setup auth for source {}: {}", source_id, e);
            return Err(StatusCode::UNAUTHORIZED);
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

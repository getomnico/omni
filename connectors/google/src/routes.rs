//! Connector-specific HTTP routes that live outside the SDK protocol
//! surface: Google Drive push-notification webhook, admin user search, and
//! the binary `/action` handler for `fetch_file` (which returns raw file
//! bytes, not JSON — `GoogleConnector::owns_action_route` is true so the
//! SDK skips its default JSON `/action`).

use std::sync::Arc;

use axum::{
    extract::{Path, Query, State},
    http::{HeaderMap, StatusCode},
    response::Json,
    routing::{get, post},
    Router,
};
use omni_connector_sdk::{ActionRequest, ActionResponse};
use serde::{Deserialize, Serialize};
use shared::models::{ServiceProvider, SourceType};
use tracing::{debug, error, info, warn};

use crate::admin::AdminClient;
use crate::auth::{GoogleAuth, ServiceAccountAuth};
use crate::drive::DriveClient;
use crate::models::WebhookNotification;
use crate::sync::SyncManager;

#[derive(Clone)]
pub struct RoutesState {
    pub sync_manager: Arc<SyncManager>,
    pub admin_client: Arc<AdminClient>,
}

pub fn build_router(sync_manager: Arc<SyncManager>, admin_client: Arc<AdminClient>) -> Router {
    let state = RoutesState {
        sync_manager,
        admin_client,
    };
    Router::new()
        .route("/action", post(execute_action))
        .route("/webhook", post(handle_webhook))
        .route("/users/search/:source_id", get(search_users))
        .with_state(state)
}

// ---------------------------------------------------------------------------
// /action — binary passthrough for fetch_file
// ---------------------------------------------------------------------------

async fn execute_action(
    Json(request): Json<ActionRequest>,
) -> Result<axum::response::Response, (StatusCode, Json<ActionResponse>)> {
    info!("Action requested: {}", request.action);

    match request.action.as_str() {
        "fetch_file" => execute_fetch_file(request).await,
        _ => {
            let resp = ActionResponse::not_supported(&request.action);
            Err((StatusCode::BAD_REQUEST, Json(resp)))
        }
    }
}

async fn execute_fetch_file(
    request: ActionRequest,
) -> Result<axum::response::Response, (StatusCode, Json<ActionResponse>)> {
    let file_id = request
        .params
        .get("file_id")
        .and_then(|v| v.as_str())
        .ok_or_else(|| {
            let resp = ActionResponse::failure("Missing required parameter: file_id".to_string());
            (StatusCode::BAD_REQUEST, Json(resp))
        })?
        .to_string();

    let service_account_key = request
        .credentials
        .get("credentials")
        .and_then(|c| c.get("service_account_key"))
        .and_then(|v| v.as_str())
        .ok_or_else(|| {
            let resp =
                ActionResponse::failure("Missing service_account_key in credentials".to_string());
            (StatusCode::BAD_REQUEST, Json(resp))
        })?;

    let principal_email = request
        .credentials
        .get("principal_email")
        .and_then(|v| v.as_str())
        .ok_or_else(|| {
            let resp =
                ActionResponse::failure("Missing principal_email in credentials".to_string());
            (StatusCode::BAD_REQUEST, Json(resp))
        })?;

    let scopes = crate::auth::get_scopes_for_source_type(SourceType::GoogleDrive);
    let auth = ServiceAccountAuth::new(service_account_key, scopes).map_err(|e| {
        error!("Failed to create auth: {}", e);
        let resp = ActionResponse::failure(format!("Authentication failed: {}", e));
        (StatusCode::INTERNAL_SERVER_ERROR, Json(resp))
    })?;

    let google_auth = GoogleAuth::ServiceAccount(auth);
    let drive_client = DriveClient::new();

    let file_meta = drive_client
        .get_file_metadata(&google_auth, principal_email, &file_id)
        .await
        .map_err(|e| {
            error!("Failed to get file metadata: {}", e);
            let resp = ActionResponse::failure(format!("Failed to get file metadata: {}", e));
            (StatusCode::INTERNAL_SERVER_ERROR, Json(resp))
        })?;

    let mime_type = &file_meta.mime_type;
    let file_name = &file_meta.name;

    let export_mapping: Option<(&str, &str)> = match mime_type.as_str() {
        "application/vnd.google-apps.spreadsheet" => Some((
            "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet",
            ".xlsx",
        )),
        "application/vnd.google-apps.document" => Some((
            "application/vnd.openxmlformats-officedocument.wordprocessingml.document",
            ".docx",
        )),
        "application/vnd.google-apps.presentation" => Some((
            "application/vnd.openxmlformats-officedocument.presentationml.presentation",
            ".pptx",
        )),
        _ => None,
    };

    let (bytes, content_type, download_name) = if let Some((export_mime, ext)) = export_mapping {
        let bytes = drive_client
            .export_file(&google_auth, principal_email, &file_id, export_mime)
            .await
            .map_err(|e| {
                error!("Failed to export file: {}", e);
                let resp = ActionResponse::failure(format!("Failed to export file: {}", e));
                (StatusCode::INTERNAL_SERVER_ERROR, Json(resp))
            })?;
        (
            bytes,
            export_mime.to_string(),
            ensure_extension(file_name, ext),
        )
    } else {
        let bytes = drive_client
            .download_file_binary(&google_auth, principal_email, &file_id)
            .await
            .map_err(|e| {
                error!("Failed to download file: {}", e);
                let resp = ActionResponse::failure(format!("Failed to download file: {}", e));
                (StatusCode::INTERNAL_SERVER_ERROR, Json(resp))
            })?;
        (bytes, mime_type.clone(), file_name.clone())
    };

    info!(
        "Returning binary response for file '{}' ({} bytes, {})",
        download_name,
        bytes.len(),
        content_type
    );

    let response = axum::response::Response::builder()
        .status(StatusCode::OK)
        .header("Content-Type", &content_type)
        .header("X-File-Name", &download_name)
        .header("Content-Length", bytes.len().to_string())
        .body(axum::body::Body::from(bytes))
        .unwrap();

    Ok(response)
}

fn ensure_extension(name: &str, ext: &str) -> String {
    if name.ends_with(ext) {
        name.to_string()
    } else {
        format!("{}{}", name, ext)
    }
}

// ---------------------------------------------------------------------------
// /webhook — Google Drive push notifications
// ---------------------------------------------------------------------------

async fn handle_webhook(
    State(state): State<RoutesState>,
    headers: HeaderMap,
) -> Result<StatusCode, StatusCode> {
    debug!("Received webhook notification");

    let notification = match WebhookNotification::from_headers(&headers) {
        Some(notification) => notification,
        None => {
            warn!("Failed to parse webhook notification from headers");
            return Err(StatusCode::BAD_REQUEST);
        }
    };

    info!(
        "Processing webhook notification: channel_id={}, resource_state={}, source_id={:?}",
        notification.channel_id, notification.resource_state, notification.source_id
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

    Ok(StatusCode::OK)
}

// ---------------------------------------------------------------------------
// /users/search/:source_id — admin directory user search
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize)]
pub struct UserSearchQuery {
    q: Option<String>,
    limit: Option<u32>,
    page_token: Option<String>,
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
    State(state): State<RoutesState>,
    Path(source_id): Path<String>,
    Query(params): Query<UserSearchQuery>,
) -> Result<Json<UserSearchResponse>, StatusCode> {
    info!("Searching users for source: {}", source_id);

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

    if creds.provider != ServiceProvider::Google {
        error!(
            "Expected Google credentials for source {}, found {:?}",
            source_id, creds.provider
        );
        return Err(StatusCode::BAD_REQUEST);
    }

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

    let admin_scopes = crate::auth::get_scopes_for_source_type(SourceType::GoogleDrive);
    let auth = match ServiceAccountAuth::new(service_account_key, admin_scopes) {
        Ok(auth) => auth,
        Err(e) => {
            error!("Failed to create auth for source {}: {}", source_id, e);
            return Err(StatusCode::INTERNAL_SERVER_ERROR);
        }
    };

    let token = match auth.get_access_token(&principal_email).await {
        Ok(token) => token,
        Err(e) => {
            error!("Failed to get access token for source {}: {}", source_id, e);
            return Err(StatusCode::UNAUTHORIZED);
        }
    };

    let limit = params.limit.unwrap_or(50).min(100);
    let query = params.q.as_deref();
    let page_token = params.page_token.as_deref();

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

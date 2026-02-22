use crate::connector_client::ConnectorClient;
use crate::models::{
    ActionRequest, ConnectorInfo, ExecuteActionRequest, ScheduleInfo, SyncProgress,
    TriggerSyncRequest, TriggerSyncResponse, TriggerType,
};
use crate::sync_manager::SyncError;
use crate::AppState;
use axum::{
    extract::{Path, State},
    http::StatusCode,
    response::{
        sse::{Event, KeepAlive, Sse},
        IntoResponse,
    },
    Json,
};
use futures::stream::Stream;
use serde_json::json;
use shared::db::repositories::SyncRunRepository;
use shared::models::{SourceType, SyncType};
use shared::queue::EventQueue;
use shared::utils;
use shared::{Repository, ServiceCredentialsRepo, SourceRepository};
use std::collections::HashMap;
use std::convert::Infallible;
use std::time::Duration;
use tracing::{debug, error, info};

pub async fn health_check() -> impl IntoResponse {
    Json(json!({ "status": "healthy" }))
}

pub async fn trigger_sync(
    State(state): State<AppState>,
    Json(request): Json<TriggerSyncRequest>,
) -> Result<Json<TriggerSyncResponse>, ApiError> {
    info!("Manual sync triggered for source {}", request.source_id);

    let sync_run_id = state
        .sync_manager
        .trigger_sync(
            &request.source_id,
            match request.sync_mode.as_deref() {
                // TODO: Use SyncType in TriggerSyncRequest
                Some("full") => SyncType::Full,
                _ => SyncType::Incremental,
            },
            TriggerType::Manual,
        )
        .await?;

    Ok(Json(TriggerSyncResponse {
        sync_run_id,
        status: "started".to_string(),
    }))
}

pub async fn trigger_sync_by_id(
    State(state): State<AppState>,
    Path(source_id): Path<String>,
) -> Result<Json<TriggerSyncResponse>, ApiError> {
    info!("Manual sync triggered for source {}", source_id);

    let sync_run_id = state
        .sync_manager
        .trigger_sync(&source_id, SyncType::Full, TriggerType::Manual)
        .await
        .map_err(|e| {
            error!("Failed to trigger sync for source {}: {:?}", source_id, e);
            ApiError::from(e)
        })?;

    Ok(Json(TriggerSyncResponse {
        sync_run_id,
        status: "started".to_string(),
    }))
}

pub async fn cancel_sync(
    State(state): State<AppState>,
    Path(sync_run_id): Path<String>,
) -> Result<Json<serde_json::Value>, ApiError> {
    info!("Cancel requested for sync {}", sync_run_id);

    state.sync_manager.cancel_sync(&sync_run_id).await?;

    Ok(Json(json!({ "status": "cancelled" })))
}

pub async fn get_sync_progress(
    State(state): State<AppState>,
    Path(sync_run_id): Path<String>,
) -> Result<Sse<impl Stream<Item = Result<Event, Infallible>>>, ApiError> {
    debug!("SSE connection for sync progress: {}", sync_run_id);

    let pool = state.db_pool.pool().clone();
    let sync_run_id_clone = sync_run_id.clone();

    let stream = async_stream::stream! {
        let mut interval = tokio::time::interval(Duration::from_secs(1));

        loop {
            interval.tick().await;

            let progress = match get_progress_from_db(&pool, &sync_run_id_clone).await {
                Ok(p) => p,
                Err(e) => {
                    error!("Failed to get progress: {}", e);
                    break;
                }
            };

            let event = Event::default()
                .json_data(&progress)
                .unwrap_or_else(|_| Event::default().data("error"));

            yield Ok(event);

            // Stop streaming if sync is complete
            if progress.status != "running" {
                break;
            }
        }
    };

    Ok(Sse::new(stream).keep_alive(KeepAlive::default()))
}

async fn get_progress_from_db(
    pool: &sqlx::PgPool,
    sync_run_id: &str,
) -> Result<SyncProgress, sqlx::Error> {
    let row: (
        String,
        String,
        String,
        i32,
        i32,
        i32,
        Option<String>,
        Option<time::OffsetDateTime>,
        Option<time::OffsetDateTime>,
    ) = sqlx::query_as(
        r#"
        SELECT id, source_id, status, documents_scanned, documents_processed, documents_updated,
               error_message, started_at, completed_at
        FROM sync_runs
        WHERE id = $1
        "#,
    )
    .bind(sync_run_id)
    .fetch_one(pool)
    .await?;

    Ok(SyncProgress {
        sync_run_id: row.0,
        source_id: row.1,
        status: row.2,
        documents_scanned: row.3,
        documents_processed: row.4,
        documents_updated: row.5,
        error_message: row.6,
        started_at: row.7.map(|t| t.to_string()),
        completed_at: row.8.map(|t| t.to_string()),
    })
}

pub async fn list_schedules(
    State(state): State<AppState>,
) -> Result<Json<Vec<ScheduleInfo>>, ApiError> {
    let source_repo = SourceRepository::new(state.db_pool.pool());
    let sync_run_repo = SyncRunRepository::new(state.db_pool.pool());

    let sources = source_repo
        .find_active_sources()
        .await
        .map_err(|e| ApiError::Internal(e.to_string()))?;

    let source_ids: Vec<String> = sources.iter().map(|s| s.id.clone()).collect();
    let latest_runs = sync_run_repo
        .find_latest_for_sources(&source_ids)
        .await
        .map_err(|e| ApiError::Internal(e.to_string()))?;

    let runs_by_source: HashMap<String, &shared::models::SyncRun> = latest_runs
        .iter()
        .map(|r| (r.source_id.clone(), r))
        .collect();

    let schedules: Vec<ScheduleInfo> = sources
        .into_iter()
        .map(|source| {
            let latest_run = runs_by_source.get(&source.id);
            let last_sync_at = latest_run.and_then(|r| r.completed_at);
            let next_sync_at = match (last_sync_at, source.sync_interval_seconds) {
                (Some(completed), Some(interval)) => {
                    Some(completed + time::Duration::seconds(interval as i64))
                }
                _ => None,
            };

            ScheduleInfo {
                source_id: source.id,
                source_name: source.name,
                source_type: serde_json::to_value(&source.source_type)
                    .ok()
                    .and_then(|v| v.as_str().map(String::from))
                    .unwrap_or_default(),
                sync_interval_seconds: source.sync_interval_seconds,
                next_sync_at: next_sync_at.map(|t| t.to_string()),
                last_sync_at: last_sync_at.map(|t| t.to_string()),
                sync_status: latest_run.map(|r| {
                    serde_json::to_value(&r.status)
                        .ok()
                        .and_then(|v| v.as_str().map(String::from))
                        .unwrap_or_default()
                }),
            }
        })
        .collect();

    Ok(Json(schedules))
}

pub async fn list_connectors(
    State(state): State<AppState>,
) -> Result<Json<Vec<ConnectorInfo>>, ApiError> {
    let client = ConnectorClient::new();
    let mut connectors = Vec::new();

    for (source_type, url) in &state.config.connector_urls {
        let healthy = client.health_check(url).await;
        let manifest = if healthy {
            client.get_manifest(url).await.ok()
        } else {
            None
        };

        connectors.push(ConnectorInfo {
            source_type: source_type.clone(),
            url: url.clone(),
            healthy,
            manifest,
        });
    }

    Ok(Json(connectors))
}

pub async fn execute_action(
    State(state): State<AppState>,
    Json(request): Json<ExecuteActionRequest>,
) -> Result<Json<serde_json::Value>, ApiError> {
    info!(
        "Executing action {} for source {}",
        request.action, request.source_id
    );

    // Get source to determine connector type
    let source: Option<(SourceType,)> =
        sqlx::query_as("SELECT source_type FROM sources WHERE id = $1")
            .bind(&request.source_id)
            .fetch_optional(state.db_pool.pool())
            .await
            .map_err(|e| ApiError::Internal(e.to_string()))?;

    let source_type = source
        .ok_or_else(|| ApiError::NotFound(format!("Source not found: {}", request.source_id)))?
        .0;

    let connector_url = state.config.get_connector_url(source_type).ok_or_else(|| {
        ApiError::NotFound(format!(
            "Connector not configured for type: {:?}",
            source_type
        ))
    })?;

    // Get credentials
    let creds_repo = ServiceCredentialsRepo::new(state.db_pool.pool().clone())
        .map_err(|e| ApiError::Internal(e.to_string()))?;
    let creds = creds_repo
        .get_by_source_id(&request.source_id)
        .await
        .map_err(|e| ApiError::Internal(e.to_string()))?
        .ok_or_else(|| {
            ApiError::NotFound(format!(
                "Credentials not found for source: {}",
                request.source_id
            ))
        })?;

    let client = ConnectorClient::new();
    let action_request = ActionRequest {
        action: request.action,
        params: request.params,
        credentials: json!({
            "credentials": creds.credentials,
            "config": creds.config,
            "principal_email": creds.principal_email,
        }),
    };

    let response = client
        .execute_action(connector_url, &action_request)
        .await
        .map_err(|e| ApiError::Internal(e.to_string()))?;

    Ok(Json(json!({
        "status": response.status,
        "result": response.result,
        "error": response.error
    })))
}

pub async fn list_actions(
    State(state): State<AppState>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let client = ConnectorClient::new();
    let mut all_actions = Vec::new();

    for (source_type, url) in &state.config.connector_urls {
        if let Ok(manifest) = client.get_manifest(url).await {
            for action in manifest.actions {
                all_actions.push(json!({
                    "source_type": source_type,
                    "name": action.name,
                    "description": action.description,
                    "parameters": action.parameters
                }));
            }
        }
    }

    Ok(Json(json!({ "actions": all_actions })))
}

#[derive(Debug, thiserror::Error)]
pub enum ApiError {
    #[error("Not found: {0}")]
    NotFound(String),

    #[error("Bad request: {0}")]
    BadRequest(String),

    #[error("Conflict: {0}")]
    Conflict(String),

    #[error("Internal error: {0}")]
    Internal(String),
}

impl From<SyncError> for ApiError {
    fn from(err: SyncError) -> Self {
        match err {
            SyncError::SourceNotFound(id) => {
                ApiError::NotFound(format!("Source not found: {}", id))
            }
            SyncError::SyncRunNotFound(id) => {
                ApiError::NotFound(format!("Sync run not found: {}", id))
            }
            SyncError::ConnectorNotConfigured(t) => {
                ApiError::NotFound(format!("Connector not configured for type: {}", t))
            }
            SyncError::SourceInactive(id) => {
                ApiError::BadRequest(format!("Source is inactive: {}", id))
            }
            SyncError::SyncAlreadyRunning(id) => {
                ApiError::Conflict(format!("Sync already running for source: {}", id))
            }
            SyncError::SyncNotRunning(id) => {
                ApiError::BadRequest(format!("Sync is not running: {}", id))
            }
            SyncError::ConcurrencyLimitReached => {
                ApiError::Conflict("Concurrency limit reached, try again later".to_string())
            }
            SyncError::DatabaseError(e) => ApiError::Internal(e),
            SyncError::ConnectorError(e) => ApiError::Internal(e.to_string()),
        }
    }
}

impl IntoResponse for ApiError {
    fn into_response(self) -> axum::response::Response {
        let (status, message) = match &self {
            ApiError::NotFound(msg) => (StatusCode::NOT_FOUND, msg.clone()),
            ApiError::BadRequest(msg) => (StatusCode::BAD_REQUEST, msg.clone()),
            ApiError::Conflict(msg) => (StatusCode::CONFLICT, msg.clone()),
            ApiError::Internal(msg) => (StatusCode::INTERNAL_SERVER_ERROR, msg.clone()),
        };

        let body = json!({ "error": message });
        (status, Json(body)).into_response()
    }
}

// ============================================================================
// SDK Handlers - Called by connectors
// ============================================================================

use crate::models::{
    SdkCancelSyncRequest, SdkCancelSyncResponse, SdkCompleteRequest, SdkCreateSyncRequest,
    SdkCreateSyncResponse, SdkEmitEventRequest, SdkFailRequest, SdkIncrementScannedRequest,
    SdkSourceSyncConfigResponse, SdkStatusResponse, SdkStoreContentRequest,
    SdkStoreContentResponse, SdkUserEmailResponse, SdkWebhookNotification, SdkWebhookResponse,
};

pub async fn sdk_emit_event(
    State(state): State<AppState>,
    Json(request): Json<SdkEmitEventRequest>,
) -> Result<Json<SdkStatusResponse>, ApiError> {
    debug!(
        "SDK: Emitting event for sync_run={}, source={}",
        request.sync_run_id, request.source_id
    );

    let event_queue = EventQueue::new(state.db_pool.pool().clone());

    // Enqueue the event
    event_queue
        .enqueue(&request.source_id, &request.event)
        .await
        .map_err(|e| ApiError::Internal(format!("Failed to enqueue event: {}", e)))?;

    // Update heartbeat
    let sync_run_repo = SyncRunRepository::new(state.db_pool.pool());
    sync_run_repo
        .update_activity(&request.sync_run_id)
        .await
        .map_err(|e| ApiError::Internal(format!("Failed to update activity: {}", e)))?;

    Ok(Json(SdkStatusResponse {
        status: "ok".to_string(),
    }))
}

pub async fn sdk_store_content(
    State(state): State<AppState>,
    Json(request): Json<SdkStoreContentRequest>,
) -> Result<Json<SdkStoreContentResponse>, ApiError> {
    debug!("SDK: Storing content for sync_run={}", request.sync_run_id);

    let content_storage = state.content_storage.clone();

    // Generate storage prefix from sync_run_id
    let today = time::OffsetDateTime::now_utc();
    let prefix = format!(
        "{:04}-{:02}-{:02}/{}",
        today.year(),
        today.month() as u8,
        today.day(),
        request.sync_run_id
    );

    let content = utils::normalize_whitespace(&request.content);
    let content_id = content_storage
        .store_text(&content, Some(&prefix))
        .await
        .map_err(|e| ApiError::Internal(format!("Failed to store content: {}", e)))?;

    // Update heartbeat
    let sync_run_repo = SyncRunRepository::new(state.db_pool.pool());
    sync_run_repo
        .update_activity(&request.sync_run_id)
        .await
        .map_err(|e| ApiError::Internal(format!("Failed to update activity: {}", e)))?;

    Ok(Json(SdkStoreContentResponse { content_id }))
}

pub async fn sdk_heartbeat(
    State(state): State<AppState>,
    Path(sync_run_id): Path<String>,
) -> Result<Json<SdkStatusResponse>, ApiError> {
    debug!("SDK: Heartbeat for sync_run={}", sync_run_id);

    let sync_run_repo = SyncRunRepository::new(state.db_pool.pool());
    sync_run_repo
        .update_activity(&sync_run_id)
        .await
        .map_err(|e| ApiError::Internal(format!("Failed to update activity: {}", e)))?;

    Ok(Json(SdkStatusResponse {
        status: "ok".to_string(),
    }))
}

pub async fn sdk_complete(
    State(state): State<AppState>,
    Path(sync_run_id): Path<String>,
    Json(request): Json<SdkCompleteRequest>,
) -> Result<Json<SdkStatusResponse>, ApiError> {
    info!("SDK: Completing sync_run={}", sync_run_id);

    let sync_run_repo = SyncRunRepository::new(state.db_pool.pool());

    // Mark sync as completed
    sync_run_repo
        .mark_completed(
            &sync_run_id,
            request.documents_scanned.unwrap_or(0),
            request.documents_updated.unwrap_or(0),
        )
        .await
        .map_err(|e| ApiError::Internal(format!("Failed to mark completed: {}", e)))?;

    // Store connector state if provided
    if let Some(new_state) = request.new_state {
        if let Ok(Some(sync_run)) = sync_run_repo.find_by_id(&sync_run_id).await {
            let source_repo = SourceRepository::new(state.db_pool.pool());
            let _ = source_repo
                .update_connector_state(&sync_run.source_id, new_state)
                .await;
        }
    }

    Ok(Json(SdkStatusResponse {
        status: "ok".to_string(),
    }))
}

pub async fn sdk_fail(
    State(state): State<AppState>,
    Path(sync_run_id): Path<String>,
    Json(request): Json<SdkFailRequest>,
) -> Result<Json<SdkStatusResponse>, ApiError> {
    info!("SDK: Failing sync_run={}: {}", sync_run_id, request.error);

    let sync_run_repo = SyncRunRepository::new(state.db_pool.pool());

    // Mark sync as failed
    sync_run_repo
        .mark_failed(&sync_run_id, &request.error)
        .await
        .map_err(|e| ApiError::Internal(format!("Failed to mark failed: {}", e)))?;

    Ok(Json(SdkStatusResponse {
        status: "ok".to_string(),
    }))
}

pub async fn sdk_increment_scanned(
    State(state): State<AppState>,
    Path(sync_run_id): Path<String>,
    Json(request): Json<SdkIncrementScannedRequest>,
) -> Result<Json<SdkStatusResponse>, ApiError> {
    debug!(
        "SDK: Incrementing scanned for sync_run={} by {}",
        sync_run_id, request.count
    );

    let sync_run_repo = SyncRunRepository::new(state.db_pool.pool());
    sync_run_repo
        .increment_scanned(&sync_run_id, request.count)
        .await
        .map_err(|e| ApiError::Internal(format!("Failed to increment scanned: {}", e)))?;

    Ok(Json(SdkStatusResponse {
        status: "ok".to_string(),
    }))
}

pub async fn sdk_get_source(
    State(state): State<AppState>,
    Path(source_id): Path<String>,
) -> Result<Json<shared::models::Source>, ApiError> {
    debug!("SDK: Getting source config for source_id={}", source_id);

    let source_repo = SourceRepository::new(state.db_pool.pool());
    let source = source_repo
        .find_by_id(source_id.clone())
        .await
        .map_err(|e| ApiError::Internal(format!("Database error: {}", e)))?
        .ok_or_else(|| ApiError::NotFound(format!("Source not found: {}", source_id)))?;

    Ok(Json(source))
}

pub async fn sdk_get_credentials(
    State(state): State<AppState>,
    Path(source_id): Path<String>,
) -> Result<Json<shared::models::ServiceCredentials>, ApiError> {
    debug!("SDK: Getting credentials for source_id={}", source_id);

    let creds_repo = ServiceCredentialsRepo::new(state.db_pool.pool().clone())
        .map_err(|e| ApiError::Internal(format!("Failed to create credentials repo: {}", e)))?;

    let creds = creds_repo
        .get_by_source_id(&source_id)
        .await
        .map_err(|e| ApiError::Internal(format!("Database error: {}", e)))?
        .ok_or_else(|| {
            ApiError::NotFound(format!("Credentials not found for source: {}", source_id))
        })?;

    Ok(Json(creds))
}

pub async fn sdk_get_source_sync_config(
    State(state): State<AppState>,
    Path(source_id): Path<String>,
) -> Result<Json<SdkSourceSyncConfigResponse>, ApiError> {
    debug!(
        "SDK: Getting source sync config for source_id={}",
        source_id
    );

    let source_repo = SourceRepository::new(state.db_pool.pool());
    let source = source_repo
        .find_by_id(source_id.clone())
        .await
        .map_err(|e| ApiError::Internal(format!("Database error: {}", e)))?
        .ok_or_else(|| ApiError::NotFound(format!("Source not found: {}", source_id)))?;

    let creds_repo = ServiceCredentialsRepo::new(state.db_pool.pool().clone())
        .map_err(|e| ApiError::Internal(format!("Failed to create credentials repo: {}", e)))?;

    let credentials = creds_repo
        .get_by_source_id(&source_id)
        .await
        .map_err(|e| ApiError::Internal(format!("Database error: {}", e)))?
        .map(|c| c.credentials)
        .unwrap_or_else(|| serde_json::json!({}));

    Ok(Json(SdkSourceSyncConfigResponse {
        config: source.config,
        credentials,
        connector_state: source.connector_state,
        source_type: source.source_type,
    }))
}

pub async fn sdk_create_sync(
    State(state): State<AppState>,
    Json(request): Json<SdkCreateSyncRequest>,
) -> Result<Json<SdkCreateSyncResponse>, ApiError> {
    info!(
        "SDK: Creating sync run for source={}, type={:?}",
        request.source_id, request.sync_type
    );

    let sync_run_repo = SyncRunRepository::new(state.db_pool.pool());
    let sync_run = sync_run_repo
        .create(&request.source_id, request.sync_type, "manual")
        .await
        .map_err(|e| ApiError::Internal(format!("Failed to create sync run: {}", e)))?;

    Ok(Json(SdkCreateSyncResponse {
        sync_run_id: sync_run.id,
    }))
}

pub async fn sdk_cancel_sync(
    State(state): State<AppState>,
    Json(request): Json<SdkCancelSyncRequest>,
) -> Result<Json<SdkCancelSyncResponse>, ApiError> {
    info!("SDK: Cancelling sync_run={}", request.sync_run_id);

    let sync_run_repo = SyncRunRepository::new(state.db_pool.pool());
    sync_run_repo
        .mark_cancelled(&request.sync_run_id)
        .await
        .map_err(|e| ApiError::Internal(format!("Failed to cancel sync: {}", e)))?;

    Ok(Json(SdkCancelSyncResponse { success: true }))
}

pub async fn sdk_get_user_email(
    State(state): State<AppState>,
    Path(source_id): Path<String>,
) -> Result<Json<SdkUserEmailResponse>, ApiError> {
    debug!("SDK: Getting user email for source_id={}", source_id);

    let email = sqlx::query_scalar::<_, String>(
        "SELECT u.email FROM sources s JOIN users u ON s.created_by = u.id WHERE s.id = $1",
    )
    .bind(&source_id)
    .fetch_one(state.db_pool.pool())
    .await
    .map_err(|e| ApiError::Internal(format!("Failed to get user email: {}", e)))?;

    Ok(Json(SdkUserEmailResponse { email }))
}

pub async fn sdk_notify_webhook(
    State(state): State<AppState>,
    Json(request): Json<SdkWebhookNotification>,
) -> Result<Json<SdkWebhookResponse>, ApiError> {
    info!(
        "SDK: Webhook notification for source={}, event_type={}",
        request.source_id, request.event_type
    );

    // Trigger a sync for this source (connector-manager handles sync run creation)
    let sync_run_id = state
        .sync_manager
        .trigger_sync(
            &request.source_id,
            SyncType::Incremental,
            TriggerType::Webhook,
        )
        .await
        .map_err(|e| ApiError::Internal(format!("Failed to trigger sync: {}", e)))?;

    Ok(Json(SdkWebhookResponse { sync_run_id }))
}

// ============================================================================
// SDK Connector State Management
// ============================================================================

pub async fn sdk_update_connector_state(
    State(state): State<AppState>,
    Path(source_id): Path<String>,
    Json(new_state): Json<serde_json::Value>,
) -> Result<Json<SdkStatusResponse>, ApiError> {
    debug!("SDK: Updating connector state for source_id={}", source_id);

    let source_repo = SourceRepository::new(state.db_pool.pool());
    source_repo
        .update_connector_state(&source_id, new_state)
        .await
        .map_err(|e| ApiError::Internal(format!("Failed to update connector state: {}", e)))?;

    Ok(Json(SdkStatusResponse {
        status: "ok".to_string(),
    }))
}

// ============================================================================
// SDK Sources by Type
// ============================================================================

pub async fn sdk_get_sources_by_type(
    State(state): State<AppState>,
    Path(source_type): Path<String>,
) -> Result<Json<Vec<shared::models::Source>>, ApiError> {
    debug!("SDK: Getting sources by type={}", source_type);

    let source_repo = SourceRepository::new(state.db_pool.pool());
    let sources = source_repo
        .find_by_type(&source_type)
        .await
        .map_err(|e| ApiError::Internal(format!("Failed to get sources by type: {}", e)))?;

    let active_sources: Vec<_> = sources.into_iter().filter(|s| s.is_active).collect();

    Ok(Json(active_sources))
}

// ============================================================================
// SDK Connector Config
// ============================================================================

pub async fn sdk_get_connector_config(
    State(state): State<AppState>,
    Path(provider): Path<String>,
) -> Result<Json<serde_json::Value>, ApiError> {
    debug!("SDK: Getting connector config for provider={}", provider);

    let repo = shared::ConnectorConfigRepository::new(state.db_pool.pool().clone());
    let config = repo
        .get_by_provider(&provider)
        .await
        .map_err(|e| ApiError::Internal(format!("Failed to get connector config: {}", e)))?
        .ok_or_else(|| {
            ApiError::NotFound(format!(
                "Connector config not found for provider: {}",
                provider
            ))
        })?;

    Ok(Json(config.config))
}

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
        .trigger_sync(&request.source_id, request.sync_mode, TriggerType::Manual)
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
        .trigger_sync(&source_id, None, TriggerType::Manual)
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
    let schedules: Vec<ScheduleInfo> = sqlx::query_as::<_, ScheduleRow>(
        r#"
        SELECT id, name, source_type::text as source_type, sync_interval_seconds,
               next_sync_at, last_sync_at, sync_status
        FROM sources
        WHERE is_active = true AND is_deleted = false
        ORDER BY next_sync_at ASC NULLS LAST
        "#,
    )
    .fetch_all(state.db_pool.pool())
    .await
    .map_err(|e| ApiError::Internal(e.to_string()))?
    .into_iter()
    .map(|row| ScheduleInfo {
        source_id: row.id,
        source_name: row.name,
        source_type: row.source_type,
        sync_interval_seconds: row.sync_interval_seconds,
        next_sync_at: row.next_sync_at.map(|t| t.to_string()),
        last_sync_at: row.last_sync_at.map(|t| t.to_string()),
        sync_status: row.sync_status,
    })
    .collect();

    Ok(Json(schedules))
}

#[derive(Debug, sqlx::FromRow)]
struct ScheduleRow {
    id: String,
    name: String,
    source_type: String,
    sync_interval_seconds: Option<i32>,
    next_sync_at: Option<time::OffsetDateTime>,
    last_sync_at: Option<time::OffsetDateTime>,
    sync_status: Option<String>,
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
    let source: Option<(String,)> =
        sqlx::query_as("SELECT source_type::text FROM sources WHERE id = $1")
            .bind(&request.source_id)
            .fetch_optional(state.db_pool.pool())
            .await
            .map_err(|e| ApiError::Internal(e.to_string()))?;

    let source_type = source
        .ok_or_else(|| ApiError::NotFound(format!("Source not found: {}", request.source_id)))?
        .0;

    let connector_url = state
        .config
        .get_connector_url(&source_type)
        .ok_or_else(|| {
            ApiError::NotFound(format!(
                "Connector not configured for type: {}",
                source_type
            ))
        })?;

    // Get credentials
    let creds_repo =
        shared::db::repositories::ServiceCredentialsRepo::new(state.db_pool.pool().clone())
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
        credentials: creds.credentials,
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
    SdkCompleteRequest, SdkEmitEventRequest, SdkFailRequest, SdkStatusResponse,
    SdkStoreContentRequest, SdkStoreContentResponse,
};
use shared::db::repositories::SyncRunRepository;
use shared::queue::EventQueue;

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

    let content_id = content_storage
        .store_text(&request.content, Some(&prefix))
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

    // Update source status
    if let Ok(Some(sync_run)) = sync_run_repo.find_by_id(&sync_run_id).await {
        let source_repo = shared::SourceRepository::new(state.db_pool.pool());
        let _ = source_repo
            .update_sync_status(
                &sync_run.source_id,
                "completed",
                Some(chrono::Utc::now()),
                None,
            )
            .await;

        // Store connector state if provided
        if let Some(new_state) = request.new_state {
            let _ = sqlx::query(
                "UPDATE sources SET connector_state = $1, updated_at = CURRENT_TIMESTAMP WHERE id = $2",
            )
            .bind(&new_state)
            .bind(&sync_run.source_id)
            .execute(state.db_pool.pool())
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

    // Update source status
    if let Ok(Some(sync_run)) = sync_run_repo.find_by_id(&sync_run_id).await {
        let source_repo = shared::SourceRepository::new(state.db_pool.pool());
        let _ = source_repo
            .update_sync_status(
                &sync_run.source_id,
                "failed",
                None,
                Some(request.error.clone()),
            )
            .await;
    }

    Ok(Json(SdkStatusResponse {
        status: "ok".to_string(),
    }))
}

pub async fn sdk_increment_scanned(
    State(state): State<AppState>,
    Path(sync_run_id): Path<String>,
) -> Result<Json<SdkStatusResponse>, ApiError> {
    debug!("SDK: Incrementing scanned for sync_run={}", sync_run_id);

    let sync_run_repo = SyncRunRepository::new(state.db_pool.pool());
    sync_run_repo
        .increment_scanned_with_activity(&sync_run_id)
        .await
        .map_err(|e| ApiError::Internal(format!("Failed to increment scanned: {}", e)))?;

    Ok(Json(SdkStatusResponse {
        status: "ok".to_string(),
    }))
}

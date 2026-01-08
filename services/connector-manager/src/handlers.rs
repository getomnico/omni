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
            SyncError::CredentialsNotFound(id) => {
                ApiError::NotFound(format!("Credentials not found for source: {}", id))
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

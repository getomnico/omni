use crate::connector_client::ConnectorClient;
use crate::models::{
    ActionRequest, ConnectorInfo, ExecuteActionRequest, ExecutePromptRequest,
    ExecuteResourceRequest, PromptRequest, ResourceRequest, ScheduleInfo, SyncProgress,
    TriggerSyncRequest, TriggerSyncResponse, TriggerType,
};
use crate::sync_manager::SyncError;
use crate::AppState;
use axum::{
    extract::{Path, Query, State},
    http::{header, HeaderValue, StatusCode},
    response::{
        sse::{Event, KeepAlive, Sse},
        IntoResponse,
    },
    Json,
};
use futures::stream::Stream;
use redis::AsyncCommands;
use serde_json::json;
use shared::clients::docling::{DoclingClient, DoclingError};
use shared::constants::REDIS_SYSTEM_SETTINGS_KEY;
use shared::db::repositories::SyncRunRepository;
use shared::models::{ConnectorManifest, SearchOperator, SourceType, SyncType};
use shared::queue::EventQueue;
use shared::utils;
use shared::{DocumentRepository, Repository, ServiceCredentialsRepo, SourceRepository};
use std::collections::HashMap;
use std::convert::Infallible;
use std::time::Duration;
use tracing::{debug, error, info, warn};

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
            request.sync_mode.unwrap_or(SyncType::Incremental),
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

pub async fn list_sources(
    State(state): State<AppState>,
) -> Result<Json<Vec<shared::models::Source>>, ApiError> {
    let source_repo = SourceRepository::new(state.db_pool.pool());
    let sources = source_repo
        .find_all_sources()
        .await
        .map_err(|e| ApiError::Internal(e.to_string()))?;
    Ok(Json(sources))
}

pub async fn list_connectors(
    State(state): State<AppState>,
) -> Result<Json<Vec<ConnectorInfo>>, ApiError> {
    let manifests = get_registered_manifests(&state.redis_client).await;
    let client = ConnectorClient::new();
    let mut connectors = Vec::new();

    for manifest in manifests {
        let url = manifest.connector_url.clone();
        let healthy = if !url.is_empty() {
            client.health_check(&url).await
        } else {
            false
        };

        for source_type in &manifest.source_types {
            connectors.push(ConnectorInfo {
                source_type: source_type.clone(),
                url: url.clone(),
                healthy,
                manifest: Some(manifest.clone()),
            });
        }
    }

    Ok(Json(connectors))
}

pub async fn execute_action(
    State(state): State<AppState>,
    Json(request): Json<ExecuteActionRequest>,
) -> Result<axum::response::Response, ApiError> {
    info!(
        "Executing action {} for source {}",
        request.action, request.source_id
    );

    // Get source to determine connector type and config
    let source: Option<(SourceType, serde_json::Value)> =
        sqlx::query_as("SELECT source_type, config FROM sources WHERE id = $1")
            .bind(&request.source_id)
            .fetch_optional(state.db_pool.pool())
            .await
            .map_err(|e| ApiError::Internal(e.to_string()))?;

    let (source_type, source_config) = source
        .ok_or_else(|| ApiError::NotFound(format!("Source not found: {}", request.source_id)))?;

    // Look up the connector manifest to get connector_url and read_only flag
    let manifests = get_registered_manifests(&state.redis_client).await;
    let manifest = manifests
        .iter()
        .find(|m| m.source_types.contains(&source_type));

    let connector_url = manifest.map(|m| m.connector_url.clone()).ok_or_else(|| {
        ApiError::NotFound(format!(
            "Connector not registered for type: {:?}",
            source_type
        ))
    })?;

    // Enforce read_only: block write-mode actions if connector or source is read-only
    let source_read_only = source_config
        .get("read_only")
        .and_then(|v| v.as_bool())
        .unwrap_or(false);

    if let Some(m) = manifest {
        if m.read_only || source_read_only {
            let action_mode = m
                .actions
                .iter()
                .find(|a| a.name == request.action)
                .map(|a| a.mode.as_str())
                .unwrap_or("write");

            if action_mode == "write" {
                return Err(ApiError::BadRequest(format!(
                    "Action '{}' is not allowed: source is read-only",
                    request.action
                )));
            }
        }
    }

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

    // Resolve Omni document ID -> source external_id.
    // TODO: replace hard-coded param names with a connector-declared resolve_params list.
    let mut params = request.params.clone();
    let doc_id = params
        .get("document_id")
        .or_else(|| params.get("file_id"))
        .and_then(|v| v.as_str())
        .map(|s| s.to_string());
    if let Some(doc_id) = doc_id {
        let doc_repo = DocumentRepository::new(state.db_pool.pool());
        if let Ok(Some(doc)) = doc_repo.find_by_id(&doc_id).await {
            info!(
                "Resolved document/file ID {} -> external_id {}",
                doc_id, doc.external_id
            );
            if let Some(obj) = params.as_object_mut() {
                obj.remove("document_id");
                obj.remove("file_id");
                obj.insert(
                    "file_id".to_string(),
                    serde_json::Value::String(doc.external_id),
                );
            }
        }
        // If not found, assume the ID is already a source-native ID and pass through
    }

    let client = ConnectorClient::new();
    let action_request = ActionRequest {
        action: request.action,
        params,
        credentials: json!({
            "credentials": creds.credentials,
            "config": creds.config,
            "principal_email": creds.principal_email,
        }),
    };

    // Use raw response to support binary passthrough
    let response = client
        .execute_action_raw(&connector_url, &action_request)
        .await
        .map_err(|e| ApiError::Internal(e.to_string()))?;

    let content_type = response
        .headers()
        .get("content-type")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("application/json")
        .to_string();

    // If JSON response, parse and return as before
    if content_type.contains("application/json") {
        let body = response
            .text()
            .await
            .map_err(|e| ApiError::Internal(e.to_string()))?;

        let action_response: crate::models::ActionResponse =
            serde_json::from_str(&body).map_err(|e| ApiError::Internal(e.to_string()))?;

        let json_body = json!({
            "status": action_response.status,
            "result": action_response.result,
            "error": action_response.error
        });

        Ok(axum::Json(json_body).into_response())
    } else {
        // Binary passthrough: proxy the response body and headers
        let mut builder = axum::response::Response::builder()
            .status(axum::http::StatusCode::OK)
            .header("Content-Type", &content_type);

        // Forward X-File-Name header if present
        if let Some(file_name) = response.headers().get("x-file-name") {
            builder = builder.header("X-File-Name", file_name);
        }
        // Forward Content-Length if present
        if let Some(content_length) = response.headers().get("content-length") {
            builder = builder.header("Content-Length", content_length);
        }

        let bytes = response
            .bytes()
            .await
            .map_err(|e| ApiError::Internal(e.to_string()))?;

        Ok(builder.body(axum::body::Body::from(bytes)).unwrap())
    }
}

pub async fn list_actions(
    State(state): State<AppState>,
    Query(params): Query<HashMap<String, String>>,
) -> Result<Json<serde_json::Value>, ApiError> {
    // If source_id is provided, check source-level read_only
    let source_read_only = if let Some(source_id) = params.get("source_id") {
        let row: Option<(serde_json::Value,)> =
            sqlx::query_as("SELECT config FROM sources WHERE id = $1")
                .bind(source_id)
                .fetch_optional(state.db_pool.pool())
                .await
                .map_err(|e| ApiError::Internal(e.to_string()))?;
        row.and_then(|(config,)| config.get("read_only").and_then(|v| v.as_bool()))
            .unwrap_or(false)
    } else {
        false
    };

    let manifests = get_registered_manifests(&state.redis_client).await;
    let mut all_actions = Vec::new();

    for manifest in manifests {
        for source_type in &manifest.source_types {
            for action in &manifest.actions {
                if (manifest.read_only || source_read_only) && action.mode == "write" {
                    continue;
                }
                all_actions.push(json!({
                    "source_type": source_type,
                    "name": action.name,
                    "description": action.description,
                    "input_schema": action.input_schema,
                    "mode": action.mode
                }));
            }
        }
    }

    Ok(Json(json!({ "actions": all_actions })))
}

pub async fn list_resources(
    State(state): State<AppState>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let manifests = get_registered_manifests(&state.redis_client).await;
    let mut all_resources = Vec::new();

    for manifest in manifests {
        if !manifest.mcp_enabled {
            continue;
        }
        for source_type in &manifest.source_types {
            for resource in &manifest.resources {
                all_resources.push(json!({
                    "source_type": source_type,
                    "uri_template": resource.uri_template,
                    "name": resource.name,
                    "description": resource.description,
                    "mime_type": resource.mime_type,
                }));
            }
        }
    }

    Ok(Json(json!({ "resources": all_resources })))
}

pub async fn list_prompts(
    State(state): State<AppState>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let manifests = get_registered_manifests(&state.redis_client).await;
    let mut all_prompts = Vec::new();

    for manifest in manifests {
        if !manifest.mcp_enabled {
            continue;
        }
        for source_type in &manifest.source_types {
            for prompt in &manifest.prompts {
                all_prompts.push(json!({
                    "source_type": source_type,
                    "name": prompt.name,
                    "description": prompt.description,
                    "arguments": prompt.arguments,
                }));
            }
        }
    }

    Ok(Json(json!({ "prompts": all_prompts })))
}

pub async fn read_resource(
    State(state): State<AppState>,
    Json(request): Json<ExecuteResourceRequest>,
) -> Result<Json<serde_json::Value>, ApiError> {
    info!(
        "Reading resource {} for source {}",
        request.uri, request.source_id
    );

    let source: Option<(SourceType,)> =
        sqlx::query_as("SELECT source_type FROM sources WHERE id = $1")
            .bind(&request.source_id)
            .fetch_optional(state.db_pool.pool())
            .await
            .map_err(|e| ApiError::Internal(e.to_string()))?;

    let source_type = source
        .ok_or_else(|| ApiError::NotFound(format!("Source not found: {}", request.source_id)))?
        .0;

    let connector_url = get_connector_url_for_source(&state.redis_client, source_type)
        .await
        .ok_or_else(|| {
            ApiError::NotFound(format!(
                "Connector not registered for type: {:?}",
                source_type
            ))
        })?;

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
    let resource_request = ResourceRequest {
        uri: request.uri,
        credentials: json!({
            "credentials": creds.credentials,
            "config": creds.config,
            "principal_email": creds.principal_email,
        }),
    };

    let result = client
        .read_resource(&connector_url, &resource_request)
        .await
        .map_err(|e| ApiError::Internal(e.to_string()))?;

    Ok(Json(result))
}

pub async fn get_prompt(
    State(state): State<AppState>,
    Json(request): Json<ExecutePromptRequest>,
) -> Result<Json<serde_json::Value>, ApiError> {
    info!(
        "Getting prompt {} for source {}",
        request.name, request.source_id
    );

    let source: Option<(SourceType,)> =
        sqlx::query_as("SELECT source_type FROM sources WHERE id = $1")
            .bind(&request.source_id)
            .fetch_optional(state.db_pool.pool())
            .await
            .map_err(|e| ApiError::Internal(e.to_string()))?;

    let source_type = source
        .ok_or_else(|| ApiError::NotFound(format!("Source not found: {}", request.source_id)))?
        .0;

    let connector_url = get_connector_url_for_source(&state.redis_client, source_type)
        .await
        .ok_or_else(|| {
            ApiError::NotFound(format!(
                "Connector not registered for type: {:?}",
                source_type
            ))
        })?;

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
    let prompt_request = PromptRequest {
        name: request.name,
        arguments: request.arguments,
        credentials: json!({
            "credentials": creds.credentials,
            "config": creds.config,
            "principal_email": creds.principal_email,
        }),
    };

    let result = client
        .get_prompt(&connector_url, &prompt_request)
        .await
        .map_err(|e| ApiError::Internal(e.to_string()))?;

    Ok(Json(result))
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

    #[error("Too many requests: {message} (retry after {retry_after_secs}s)")]
    TooManyRequests {
        message: String,
        retry_after_secs: u64,
    },
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
        if let ApiError::TooManyRequests {
            message,
            retry_after_secs,
        } = &self
        {
            let body = json!({ "error": message });
            let mut resp = (StatusCode::TOO_MANY_REQUESTS, Json(body)).into_response();
            if let Ok(v) = HeaderValue::from_str(&retry_after_secs.to_string()) {
                resp.headers_mut().insert(header::RETRY_AFTER, v);
            }
            return resp;
        }

        let (status, message) = match &self {
            ApiError::NotFound(msg) => (StatusCode::NOT_FOUND, msg.clone()),
            ApiError::BadRequest(msg) => (StatusCode::BAD_REQUEST, msg.clone()),
            ApiError::Conflict(msg) => (StatusCode::CONFLICT, msg.clone()),
            ApiError::Internal(msg) => (StatusCode::INTERNAL_SERVER_ERROR, msg.clone()),
            ApiError::TooManyRequests { .. } => unreachable!(),
        };

        let body = json!({ "error": message });
        (status, Json(body)).into_response()
    }
}

// ============================================================================
// Connector Registration
// ============================================================================

const REGISTRATION_TTL_SECONDS: u64 = 90;

pub async fn sdk_register(
    State(state): State<AppState>,
    Json(manifest): Json<ConnectorManifest>,
) -> Result<Json<SdkStatusResponse>, ApiError> {
    if manifest.connector_id.is_empty() {
        return Err(ApiError::BadRequest(
            "connector_id is required for registration".to_string(),
        ));
    }
    if manifest.connector_url.is_empty() {
        return Err(ApiError::BadRequest(
            "connector_url is required for registration".to_string(),
        ));
    }

    // Validate the connector is reachable before accepting registration
    let client = ConnectorClient::new();
    if !client.health_check(&manifest.connector_url).await {
        return Err(ApiError::BadRequest(format!(
            "Connector health check failed at {}. Registration rejected.",
            manifest.connector_url
        )));
    }

    let connector_id = &manifest.connector_id;

    info!(
        "SDK: Registered connector '{}' (source_types: {:?}, url: {})",
        connector_id, manifest.source_types, manifest.connector_url
    );

    let manifest_json = serde_json::to_string(&manifest)
        .map_err(|e| ApiError::Internal(format!("Failed to serialize manifest: {}", e)))?;

    let key = format!("connector:manifest:{}", connector_id);

    let mut conn = state
        .redis_client
        .get_multiplexed_async_connection()
        .await
        .map_err(|e| ApiError::Internal(format!("Redis connection error: {}", e)))?;

    let _: () = conn
        .set_ex(&key, &manifest_json, REGISTRATION_TTL_SECONDS)
        .await
        .map_err(|e| ApiError::Internal(format!("Failed to store registration: {}", e)))?;

    // Aggregate search operators from all registered connectors
    let keys: Vec<String> = redis::cmd("KEYS")
        .arg("connector:manifest:*")
        .query_async(&mut conn)
        .await
        .unwrap_or_default();

    let mut all_operators: Vec<SearchOperator> = Vec::new();
    for k in &keys {
        if let Ok(val) = conn.get::<_, String>(k).await {
            if let Ok(m) = serde_json::from_str::<ConnectorManifest>(&val) {
                all_operators.extend(m.search_operators);
            }
        }
    }

    if let Ok(json) = serde_json::to_string(&all_operators) {
        let _: Result<(), _> = conn.set("search:operators", json).await;
    }

    Ok(Json(SdkStatusResponse {
        status: "ok".to_string(),
    }))
}

/// Scan Redis for all registered connector manifests.
pub async fn get_registered_manifests(redis_client: &redis::Client) -> Vec<ConnectorManifest> {
    let mut conn = match redis_client.get_multiplexed_async_connection().await {
        Ok(c) => c,
        Err(e) => {
            error!("Redis connection error: {}", e);
            return Vec::new();
        }
    };

    let keys: Vec<String> = redis::cmd("KEYS")
        .arg("connector:manifest:*")
        .query_async(&mut conn)
        .await
        .unwrap_or_default();

    let mut manifests = Vec::new();
    for key in &keys {
        if let Ok(val) = conn.get::<_, String>(key).await {
            if let Ok(m) = serde_json::from_str::<ConnectorManifest>(&val) {
                manifests.push(m);
            }
        }
    }
    manifests
}

/// Look up the connector URL for a given source type from the Redis registry.
pub async fn get_connector_url_for_source(
    redis_client: &redis::Client,
    source_type: SourceType,
) -> Option<String> {
    let manifests = get_registered_manifests(redis_client).await;
    for manifest in manifests {
        if manifest.source_types.contains(&source_type) {
            return Some(manifest.connector_url);
        }
    }
    None
}

/// Look up the sync modes the connector declared for a given source type.
/// Returns an empty vec when no connector is registered for the source_type.
pub async fn get_sync_modes_for_source(
    redis_client: &redis::Client,
    source_type: SourceType,
) -> Vec<SyncType> {
    for manifest in get_registered_manifests(redis_client).await {
        if manifest.source_types.contains(&source_type) {
            return manifest.sync_modes;
        }
    }
    Vec::new()
}

const VALID_DOCLING_PRESETS: &[&str] = &["fast", "balanced", "quality"];
const DEFAULT_DOCLING_PRESET: &str = "balanced";

/// Read both the Docling `enabled` flag and quality preset from Redis in a
/// single multiplexed connection. Invalid/unknown preset values fall back to
/// `balanced` with a warning so a corrupted setting can't silently break
/// conversion (it would otherwise surface as a Docling 400 → built-in
/// fallback, invisible to the admin).
async fn get_docling_settings(redis_client: &redis::Client) -> (bool, String) {
    let mut conn = match redis_client.get_multiplexed_async_connection().await {
        Ok(c) => c,
        Err(e) => {
            warn!("Failed to connect to Redis for docling settings: {}", e);
            return (false, DEFAULT_DOCLING_PRESET.to_string());
        }
    };

    let values: Vec<Option<String>> = conn
        .hget(
            REDIS_SYSTEM_SETTINGS_KEY,
            &["docling_enabled", "docling_quality_preset"],
        )
        .await
        .unwrap_or_else(|e| {
            warn!("Failed to read docling settings from Redis: {}", e);
            vec![None, None]
        });

    let enabled = values
        .first()
        .and_then(|v| v.as_deref())
        .map(|s| s == "true")
        .unwrap_or(false);

    let preset = match values.get(1).and_then(|v| v.as_deref()) {
        Some(p) if VALID_DOCLING_PRESETS.contains(&p) => p.to_string(),
        Some(p) => {
            warn!(
                "Invalid docling preset '{}' in Redis; falling back to '{}'.",
                p, DEFAULT_DOCLING_PRESET
            );
            DEFAULT_DOCLING_PRESET.to_string()
        }
        None => DEFAULT_DOCLING_PRESET.to_string(),
    };

    (enabled, preset)
}

/// MIME types that Docling can process.
/// See <https://docling-project.github.io/docling/usage/supported_formats/>
///
/// Includes standard MIME types plus common non-standard alternatives.
/// Audio/video formats are omitted because our Docling service does not
/// include the `asr` extra required by Docling for transcription.
fn is_docling_supported_mime(mime_type: &str) -> bool {
    matches!(
        mime_type,
        // PDF
        "application/pdf" | "application/x-pdf"
        // MS Office Open XML (DOCX, XLSX, PPTX)
        | "application/vnd.openxmlformats-officedocument.wordprocessingml.document"
        | "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet"
        | "application/vnd.openxmlformats-officedocument.presentationml.presentation"
        // Legacy MS Office (DOC, XLS, PPT)
        | "application/msword"
        | "application/vnd.ms-excel"
        | "application/vnd.ms-powerpoint"
        // HTML / XHTML
        | "text/html"
        | "application/xhtml+xml"
        // Markdown
        | "text/markdown"
        | "text/x-markdown"
        // AsciiDoc
        | "text/asciidoc"
        // LaTeX
        | "application/x-latex"
        | "text/x-latex"
        // CSV
        | "text/csv"
        // Images
        | "image/png"
        | "image/jpeg"
        | "image/jpg"
        | "image/tiff"
        | "image/bmp"
        | "image/webp"
    )
}

/// Check if a filename extension corresponds to a Docling-supported format.
/// Used as fallback when the MIME type is generic (`application/octet-stream`)
/// or missing.
fn is_docling_supported_extension(filename: Option<&str>) -> bool {
    let ext = match filename.and_then(|f| f.rsplit_once('.')) {
        Some((_, e)) => e.to_ascii_lowercase(),
        None => return false,
    };
    matches!(
        ext.as_str(),
        "pdf"
            | "docx"
            | "xlsx"
            | "pptx"
            | "doc"
            | "xls"
            | "ppt"
            | "html"
            | "htm"
            | "xhtml"
            | "md"
            | "markdown"
            | "adoc"
            | "asciidoc"
            | "tex"
            | "latex"
            | "csv"
            | "png"
            | "jpg"
            | "jpeg"
            | "tiff"
            | "tif"
            | "bmp"
            | "webp"
    )
}

// ============================================================================
// SDK Handlers - Called by connectors
// ============================================================================

use crate::models::{
    SdkCancelSyncRequest, SdkCancelSyncResponse, SdkCompleteRequest, SdkCreateSyncRequest,
    SdkCreateSyncResponse, SdkEmitBatchRequest, SdkEmitEventRequest, SdkExtractContentResponse,
    SdkExtractTextResponse, SdkFailRequest, SdkIncrementScannedRequest,
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

pub async fn sdk_emit_batch(
    State(state): State<AppState>,
    Json(request): Json<SdkEmitBatchRequest>,
) -> Result<Json<SdkStatusResponse>, ApiError> {
    debug!(
        "SDK: Emitting batch of {} events for sync_run={}, source={}",
        request.events.len(),
        request.sync_run_id,
        request.source_id
    );

    let event_queue = EventQueue::new(state.db_pool.pool().clone());

    event_queue
        .enqueue_batch(&request.source_id, &request.events)
        .await
        .map_err(|e| ApiError::Internal(format!("Failed to enqueue event batch: {}", e)))?;

    let sync_run_repo = SyncRunRepository::new(state.db_pool.pool());
    sync_run_repo
        .update_activity(&request.sync_run_id)
        .await
        .map_err(|e| ApiError::Internal(format!("Failed to update activity: {}", e)))?;

    Ok(Json(SdkStatusResponse {
        status: "ok".to_string(),
    }))
}

// TODO: Merge this with sdk_store_content into a single unified content API
// that accepts both text and binary, deciding extraction based on mime type.
/// Parsed fields from a multipart extraction request.
struct ExtractMultipartFields {
    sync_run_id: String,
    mime_type: String,
    filename: Option<String>,
    data: Vec<u8>,
}

/// Parse common multipart fields used by both extract-content and extract-text.
async fn parse_extract_multipart(
    mut multipart: axum::extract::Multipart,
) -> Result<ExtractMultipartFields, ApiError> {
    let mut sync_run_id: Option<String> = None;
    let mut mime_type: Option<String> = None;
    let mut filename: Option<String> = None;
    let mut data: Option<Vec<u8>> = None;

    while let Some(field) = multipart
        .next_field()
        .await
        .map_err(|e| ApiError::BadRequest(format!("Failed to read multipart field: {}", e)))?
    {
        match field.name() {
            Some("sync_run_id") => {
                sync_run_id =
                    Some(field.text().await.map_err(|e| {
                        ApiError::BadRequest(format!("Invalid sync_run_id: {}", e))
                    })?);
            }
            Some("mime_type") => {
                mime_type = Some(
                    field
                        .text()
                        .await
                        .map_err(|e| ApiError::BadRequest(format!("Invalid mime_type: {}", e)))?,
                );
            }
            Some("filename") => {
                filename = Some(
                    field
                        .text()
                        .await
                        .map_err(|e| ApiError::BadRequest(format!("Invalid filename: {}", e)))?,
                );
            }
            Some("data") => {
                data = Some(
                    field
                        .bytes()
                        .await
                        .map_err(|e| ApiError::BadRequest(format!("Failed to read data: {}", e)))?
                        .to_vec(),
                );
            }
            _ => {}
        }
    }

    Ok(ExtractMultipartFields {
        sync_run_id: sync_run_id
            .ok_or_else(|| ApiError::BadRequest("Missing sync_run_id".to_string()))?,
        mime_type: mime_type
            .ok_or_else(|| ApiError::BadRequest("Missing mime_type".to_string()))?,
        filename,
        data: data.ok_or_else(|| ApiError::BadRequest("Missing data".to_string()))?,
    })
}

/// Extract text from binary data using Docling (if enabled) or the built-in extractor.
async fn do_extract_text(
    redis_client: &redis::Client,
    mime_type: &str,
    filename: Option<&str>,
    data: &[u8],
) -> Result<String, ApiError> {
    let docling_candidate = is_docling_supported_mime(mime_type)
        || (mime_type == "application/octet-stream" && is_docling_supported_extension(filename));
    let (docling_enabled, preset) = if docling_candidate {
        get_docling_settings(redis_client).await
    } else {
        (false, DEFAULT_DOCLING_PRESET.to_string())
    };
    if docling_candidate && docling_enabled {
        let docling_result = if let Some(client) = DoclingClient::from_env() {
            let file_name = filename.unwrap_or("document");
            debug!(
                "Using docling-based document content extraction for file '{}' (preset={})",
                file_name, preset
            );
            match client.convert(data, file_name, &preset).await {
                Ok(markdown) => {
                    debug!("Docling extraction succeeded: {} chars", markdown.len());
                    Some(markdown)
                }
                Err(DoclingError::ServiceOverloaded { retry_after_secs }) => {
                    warn!(
                        "Docling overloaded, propagating 429 (retry after {}s)",
                        retry_after_secs
                    );
                    return Err(ApiError::TooManyRequests {
                        message: "Document conversion service is overloaded. Try again later."
                            .to_string(),
                        retry_after_secs,
                    });
                }
                Err(e) => {
                    warn!("Docling extraction failed, falling back to built-in: {}", e);
                    None
                }
            }
        } else {
            warn!("Docling enabled but DOCLING_URL not set, falling back to built-in extraction");
            None
        };

        Ok(docling_result.unwrap_or_else(|| {
            debug!(
                "Using built-in document content extraction for file {:?}",
                filename
            );
            shared::content_extractor::extract_content(data, mime_type, filename).unwrap_or_else(
                |e| {
                    warn!("Built-in content extraction failed: {}", e);
                    String::new()
                },
            )
        }))
    } else {
        debug!("Using built-in document content extraction for file {:?} (docling_enabled={}, docling_candidate={})", filename, docling_enabled, docling_candidate);
        Ok(
            shared::content_extractor::extract_content(data, mime_type, filename).unwrap_or_else(
                |e| {
                    warn!("Content extraction failed: {}", e);
                    String::new()
                },
            ),
        )
    }
}

pub async fn sdk_extract_content(
    State(state): State<AppState>,
    multipart: axum::extract::Multipart,
) -> Result<Json<SdkExtractContentResponse>, ApiError> {
    let fields = parse_extract_multipart(multipart).await?;

    debug!(
        "SDK: Extracting content for sync_run={}, mime={}, filename={:?}, size={}",
        fields.sync_run_id,
        fields.mime_type,
        fields.filename,
        fields.data.len()
    );

    let extracted_text = do_extract_text(
        &state.redis_client,
        &fields.mime_type,
        fields.filename.as_deref(),
        &fields.data,
    )
    .await?;

    let today = time::OffsetDateTime::now_utc();
    let prefix = format!(
        "{:04}-{:02}-{:02}/{}",
        today.year(),
        today.month() as u8,
        today.day(),
        fields.sync_run_id
    );

    let content = utils::normalize_whitespace(&extracted_text);
    let content_id = state
        .content_storage
        .store_text(&content, Some(&prefix))
        .await
        .map_err(|e| ApiError::Internal(format!("Failed to store content: {}", e)))?;

    // Update heartbeat
    let sync_run_repo = SyncRunRepository::new(state.db_pool.pool());
    sync_run_repo
        .update_activity(&fields.sync_run_id)
        .await
        .map_err(|e| ApiError::Internal(format!("Failed to update activity: {}", e)))?;

    Ok(Json(SdkExtractContentResponse { content_id }))
}

pub async fn sdk_extract_text(
    State(state): State<AppState>,
    multipart: axum::extract::Multipart,
) -> Result<Json<SdkExtractTextResponse>, ApiError> {
    let fields = parse_extract_multipart(multipart).await?;

    debug!(
        "SDK: Extracting text for sync_run={}, mime={}, filename={:?}, size={}",
        fields.sync_run_id,
        fields.mime_type,
        fields.filename,
        fields.data.len()
    );

    let extracted_text = do_extract_text(
        &state.redis_client,
        &fields.mime_type,
        fields.filename.as_deref(),
        &fields.data,
    )
    .await?;

    let text = utils::normalize_whitespace(&extracted_text);

    // Update heartbeat
    let sync_run_repo = SyncRunRepository::new(state.db_pool.pool());
    sync_run_repo
        .update_activity(&fields.sync_run_id)
        .await
        .map_err(|e| ApiError::Internal(format!("Failed to update activity: {}", e)))?;

    Ok(Json(SdkExtractTextResponse { text }))
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
        user_filter_mode: source.user_filter_mode,
        user_whitelist: source.user_whitelist,
        user_blacklist: source.user_blacklist,
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_is_docling_supported_mime() {
        // PDF
        assert!(is_docling_supported_mime("application/pdf"));
        assert!(is_docling_supported_mime("application/x-pdf"));

        // Office Open XML
        assert!(is_docling_supported_mime(
            "application/vnd.openxmlformats-officedocument.wordprocessingml.document"
        ));
        assert!(is_docling_supported_mime(
            "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet"
        ));
        assert!(is_docling_supported_mime(
            "application/vnd.openxmlformats-officedocument.presentationml.presentation"
        ));

        // Legacy Office
        assert!(is_docling_supported_mime("application/msword"));
        assert!(is_docling_supported_mime("application/vnd.ms-excel"));
        assert!(is_docling_supported_mime("application/vnd.ms-powerpoint"));

        // HTML / XHTML
        assert!(is_docling_supported_mime("text/html"));
        assert!(is_docling_supported_mime("application/xhtml+xml"));

        // Markdown (standard + non-standard)
        assert!(is_docling_supported_mime("text/markdown"));
        assert!(is_docling_supported_mime("text/x-markdown"));

        // AsciiDoc
        assert!(is_docling_supported_mime("text/asciidoc"));

        // LaTeX
        assert!(is_docling_supported_mime("application/x-latex"));
        assert!(is_docling_supported_mime("text/x-latex"));

        // CSV
        assert!(is_docling_supported_mime("text/csv"));

        // Images
        assert!(is_docling_supported_mime("image/png"));
        assert!(is_docling_supported_mime("image/jpeg"));
        assert!(is_docling_supported_mime("image/jpg"));
        assert!(is_docling_supported_mime("image/tiff"));
        assert!(is_docling_supported_mime("image/bmp"));
        assert!(is_docling_supported_mime("image/webp"));

        // Unsupported types
        assert!(!is_docling_supported_mime("text/plain"));
        assert!(!is_docling_supported_mime("application/json"));
        assert!(!is_docling_supported_mime("image/svg+xml"));
        assert!(!is_docling_supported_mime("application/zip"));
        assert!(!is_docling_supported_mime("application/octet-stream"));
        assert!(!is_docling_supported_mime(""));
    }

    #[test]
    fn test_is_docling_supported_extension() {
        // Supported extensions
        assert!(is_docling_supported_extension(Some("report.pdf")));
        assert!(is_docling_supported_extension(Some("doc.docx")));
        assert!(is_docling_supported_extension(Some("sheet.xlsx")));
        assert!(is_docling_supported_extension(Some("slides.pptx")));
        assert!(is_docling_supported_extension(Some("old.doc")));
        assert!(is_docling_supported_extension(Some("old.xls")));
        assert!(is_docling_supported_extension(Some("old.ppt")));
        assert!(is_docling_supported_extension(Some("page.html")));
        assert!(is_docling_supported_extension(Some("page.htm")));
        assert!(is_docling_supported_extension(Some("page.xhtml")));
        assert!(is_docling_supported_extension(Some("readme.md")));
        assert!(is_docling_supported_extension(Some("readme.markdown")));
        assert!(is_docling_supported_extension(Some("guide.adoc")));
        assert!(is_docling_supported_extension(Some("guide.asciidoc")));
        assert!(is_docling_supported_extension(Some("paper.tex")));
        assert!(is_docling_supported_extension(Some("paper.latex")));
        assert!(is_docling_supported_extension(Some("data.csv")));
        assert!(is_docling_supported_extension(Some("photo.png")));
        assert!(is_docling_supported_extension(Some("photo.jpg")));
        assert!(is_docling_supported_extension(Some("photo.jpeg")));
        assert!(is_docling_supported_extension(Some("scan.tiff")));
        assert!(is_docling_supported_extension(Some("scan.tif")));
        assert!(is_docling_supported_extension(Some("image.bmp")));
        assert!(is_docling_supported_extension(Some("image.webp")));

        // Case insensitive via filename
        assert!(is_docling_supported_extension(Some("REPORT.PDF")));
        assert!(is_docling_supported_extension(Some("Doc.DOCX")));

        // Unsupported
        assert!(!is_docling_supported_extension(Some("file.txt")));
        assert!(!is_docling_supported_extension(Some("data.json")));
        assert!(!is_docling_supported_extension(Some("archive.zip")));
        assert!(!is_docling_supported_extension(Some("noext")));
        assert!(!is_docling_supported_extension(Some("pdf"))); // no dot — not an extension
        assert!(!is_docling_supported_extension(None));
    }
}

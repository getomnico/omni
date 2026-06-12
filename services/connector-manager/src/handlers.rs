use crate::connector_client::ConnectorClient;
use crate::models::{
    ActionRequest, ConnectorInfo, ExecuteActionRequest, ExecutePromptRequest,
    ExecuteResourceRequest, PromptRequest, ResourceRequest, ScheduleInfo, SourceHealth,
    SourceSyncOverview, SyncProgress, TriggerSyncRequest, TriggerSyncResponse, TriggerType,
};
use crate::sync_circuit_breaker::has_failure_streak;
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
use shared::db::repositories::SyncRunRepository;
use shared::models::{
    ActionMode, ConnectorManifest, SearchOperator, ServiceProvider, Source, SourceType, SyncRun,
    SyncType,
};
use shared::queue::EventQueue;
use shared::utils;
use shared::{DocumentRepository, Repository, ServiceCredentialsRepo, SourceRepository};
use std::collections::HashMap;
use std::convert::Infallible;
use std::time::Duration;
use tokio::sync::OwnedSemaphorePermit;
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
) -> Result<Json<Vec<SourceSyncOverview>>, ApiError> {
    let source_repo = SourceRepository::new(state.db_pool.pool());
    let sources = source_repo
        .find_all_sources()
        .await
        .map_err(|e| ApiError::Internal(e.to_string()))?;

    Ok(Json(build_source_sync_overviews(&state, sources).await?))
}

pub async fn get_source(
    State(state): State<AppState>,
    Path(source_id): Path<String>,
) -> Result<Json<SourceSyncOverview>, ApiError> {
    let source_repo = SourceRepository::new(state.db_pool.pool());
    let source = source_repo
        .find_by_id(source_id.clone())
        .await
        .map_err(|e| ApiError::Internal(e.to_string()))?
        .filter(|source| !source.is_deleted)
        .ok_or_else(|| ApiError::NotFound(format!("Source not found: {}", source_id)))?;

    let mut overviews = build_source_sync_overviews(&state, vec![source]).await?;
    let overview = overviews
        .pop()
        .ok_or_else(|| ApiError::NotFound(format!("Source not found: {}", source_id)))?;

    Ok(Json(overview))
}

async fn build_source_sync_overviews(
    state: &AppState,
    sources: Vec<Source>,
) -> Result<Vec<SourceSyncOverview>, ApiError> {
    let sync_run_repo = SyncRunRepository::new(state.db_pool.pool());
    let source_ids: Vec<String> = sources.iter().map(|s| s.id.clone()).collect();
    // Fetch more than the 10 runs we return to the UI so health evaluation can
    // ignore manual failures while still finding enough scheduled failures to
    // make a circuit-breaker decision.
    let sync_run_limit = state
        .config
        .sync_max_consecutive_failures
        .max(10)
        .saturating_mul(3);
    let sync_runs = sync_run_repo
        .list_runs_for_sync_types(
            &source_ids,
            &[SyncType::Full, SyncType::Incremental],
            i64::from(sync_run_limit),
        )
        .await
        .map_err(|e| ApiError::Internal(e.to_string()))?;

    let mut runs_by_source: HashMap<String, Vec<SyncRun>> = HashMap::new();
    for run in sync_runs {
        runs_by_source
            .entry(run.source_id.clone())
            .or_default()
            .push(run);
    }

    Ok(sources
        .into_iter()
        .map(|source| {
            let sync_runs = runs_by_source.remove(&source.id).unwrap_or_default();
            let health =
                if has_failure_streak(&sync_runs, state.config.sync_max_consecutive_failures) {
                    SourceHealth::Unhealthy
                } else {
                    SourceHealth::Healthy
                };
            let sync_runs = sync_runs.into_iter().take(10).collect();

            SourceSyncOverview {
                sync_runs,
                source,
                health,
            }
        })
        .collect())
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
        "Executing action '{}' for source {} (user {:?}, params keys: {:?})",
        request.action,
        request.source_id,
        request.user_id,
        request
            .params
            .as_object()
            .map(|m| m.keys().cloned().collect::<Vec<_>>())
            .unwrap_or_default()
    );

    let source_repo = SourceRepository::new(state.db_pool.pool());
    let source = source_repo
        .find_by_id(request.source_id.clone())
        .await
        .map_err(|e| ApiError::Internal(e.to_string()))?
        .ok_or_else(|| ApiError::NotFound(format!("Source not found: {}", request.source_id)))?;

    // Look up the connector manifest to get connector_url and read_only flag
    let manifests = get_registered_manifests(&state.redis_client).await;
    let manifest = manifests
        .iter()
        .find(|m| m.source_types.contains(&source.source_type));

    let connector_url = manifest.map(|m| m.connector_url.clone()).ok_or_else(|| {
        ApiError::NotFound(format!(
            "Connector not registered for type: {:?}",
            source.source_type
        ))
    })?;

    let action_def = manifest.and_then(|m| m.actions.iter().find(|a| a.name == request.action));
    let action_mode = action_def.map(|a| a.mode).unwrap_or_default();
    let action_admin_only = action_def.map(|a| a.admin_only).unwrap_or(false);

    // TODO: replace this opaque-blob `read_only` lookup with a strongly-typed
    // SourceConfig. Today every connector pokes its own keys into Source.config
    // unchecked — `read_only` is the only key the manager itself reads.
    let source_read_only = source
        .config
        .get("read_only")
        .and_then(|v| v.as_bool())
        .unwrap_or(false);

    if let Some(m) = manifest {
        if (m.read_only || source_read_only) && action_mode == ActionMode::Write {
            return Err(ApiError::BadRequest(format!(
                "Action '{}' is not allowed: source is read-only",
                request.action
            )));
        }
    }

    let creds_repo = ServiceCredentialsRepo::new(state.db_pool.pool().clone())
        .map_err(|e| ApiError::Internal(e.to_string()))?;
    let creds = match resolve_credentials(
        &creds_repo,
        &request.source_id,
        request.user_id.as_deref(),
        action_admin_only,
    )
    .await?
    {
        CredentialResolution::Resolved(c) => c,
        CredentialResolution::NeedsUserAuth { provider } => {
            return Ok(needs_user_auth_response(
                &request.source_id,
                source.source_type,
                provider,
            )?);
        }
        CredentialResolution::NoCredentials => {
            return Err(ApiError::NotFound(format!(
                "Credentials not found for source: {}",
                request.source_id
            )));
        }
    };

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

    // Merge source config into params so connectors can access source-level
    // settings during action execution (e.g., server_url for Nextcloud
    // fetch_file). Caller-provided param values always take precedence.
    // Null/missing params is promoted to an empty object so the merge runs.
    if params.is_null() {
        params = serde_json::Value::Object(serde_json::Map::new());
    }
    if let (Some(src_obj), Some(params_obj)) = (source.config.as_object(), params.as_object_mut()) {
        for (k, v) in src_obj {
            params_obj.entry(k.clone()).or_insert_with(|| v.clone());
        }
    }

    info!(
        "Dispatching action '{}' to connector {} with credential {} (provider={:?}, auth_type={:?}, principal={:?})",
        request.action,
        connector_url,
        creds.id,
        creds.provider,
        creds.auth_type,
        creds.principal_email,
    );

    let client = ConnectorClient::new();
    let action_request = ActionRequest {
        action: request.action,
        params,
        credentials: Some(creds),
    };

    // Proxy the connector's full HTTP response (status, headers, body) verbatim.
    let response = client
        .execute_action_raw(&connector_url, &action_request)
        .await
        .map_err(|e| ApiError::Internal(e.to_string()))?;

    let status = response.status();
    let mut builder = axum::response::Response::builder().status(status);

    // Forward all headers except hop-by-hop connection headers.
    let hop_by_hop = [
        "connection",
        "keep-alive",
        "transfer-encoding",
        "te",
        "trailer",
        "upgrade",
    ];
    for (key, value) in response.headers() {
        let key_str = key.as_str();
        if !hop_by_hop.contains(&key_str) {
            builder = builder.header(key, value);
        }
    }

    let bytes = response
        .bytes()
        .await
        .map_err(|e| ApiError::Internal(e.to_string()))?;

    Ok(builder.body(axum::body::Body::from(bytes)).unwrap())
}

/// Outcome of resolving credentials for a tool/action invocation.
enum CredentialResolution {
    Resolved(shared::models::ServiceCredential),
    NeedsUserAuth { provider: ServiceProvider },
    NoCredentials,
}

/// Resolve which credential to use for a tool/action invocation.
///
/// * `admin_only` action → org row regardless of user_id. These actions
///   (e.g. Google Admin directory ops) require the service-account credential
///   the admin set up org-wide; per-user OAuth scopes don't cover them.
/// * `Some(user_id)` (chat tool, user-scoped agent) → per-user row required.
///   No fallback to org credentials — if the user hasn't connected, return
///   `NeedsUserAuth` so the UI can prompt. Personal sources satisfy this
///   because their cred row is keyed on the owner's user_id (see migration
///   087).
/// * `None` (sync, org-level agent) → org row.
async fn resolve_credentials(
    creds_repo: &ServiceCredentialsRepo,
    source_id: &str,
    user_id: Option<&str>,
    admin_only: bool,
) -> Result<CredentialResolution, ApiError> {
    let internal = |e: anyhow::Error| ApiError::Internal(e.to_string());

    if admin_only {
        let resolved = creds_repo
            .find_org_credential(source_id)
            .await
            .map_err(internal)?;
        match &resolved {
            Some(c) => info!(
                "resolve_credentials(source={}, user={:?}): admin_only → org cred {}",
                source_id, user_id, c.id
            ),
            None => warn!(
                "resolve_credentials(source={}, user={:?}): admin_only → no org cred found",
                source_id, user_id
            ),
        }
        return Ok(resolved
            .map(CredentialResolution::Resolved)
            .unwrap_or(CredentialResolution::NoCredentials));
    }

    match user_id {
        Some(uid) => {
            if let Some(c) = creds_repo
                .find_user_credential(source_id, uid)
                .await
                .map_err(internal)?
            {
                info!(
                    "resolve_credentials(source={}, user={}): per-user cred {}",
                    source_id, uid, c.id
                );
                return Ok(CredentialResolution::Resolved(c));
            }
            // No per-user row — surface a NeedsUserAuth response so the UI
            // can prompt. Provider hint comes from the org row when present;
            // if neither row exists the source is misconfigured.
            match creds_repo
                .find_org_credential(source_id)
                .await
                .map_err(internal)?
            {
                Some(org) => {
                    info!(
                        "resolve_credentials(source={}, user={}): no per-user cred, org row exists → NeedsUserAuth({:?})",
                        source_id, uid, org.provider
                    );
                    Ok(CredentialResolution::NeedsUserAuth {
                        provider: org.provider,
                    })
                }
                None => {
                    warn!(
                        "resolve_credentials(source={}, user={}): no per-user cred and no org cred",
                        source_id, uid
                    );
                    Ok(CredentialResolution::NoCredentials)
                }
            }
        }
        None => {
            let resolved = creds_repo
                .find_org_credential(source_id)
                .await
                .map_err(internal)?;
            match &resolved {
                Some(c) => info!(
                    "resolve_credentials(source={}, no user): org cred {}",
                    source_id, c.id
                ),
                None => warn!(
                    "resolve_credentials(source={}, no user): no org cred found",
                    source_id
                ),
            }
            Ok(resolved
                .map(CredentialResolution::Resolved)
                .unwrap_or(CredentialResolution::NoCredentials))
        }
    }
}

/// Wire shape for the 412 "needs user auth" response. Stable contract used by
/// the web layer and AI service to drive the "Connect <provider>" CTA.
#[derive(Debug, serde::Serialize)]
struct NeedsUserAuthResponse {
    error: &'static str,
    source_id: String,
    source_type: SourceType,
    provider: ServiceProvider,
    oauth_start_url: String,
}

fn needs_user_auth_response(
    source_id: &str,
    source_type: SourceType,
    provider: ServiceProvider,
) -> Result<axum::response::Response, ApiError> {
    let body = NeedsUserAuthResponse {
        error: "needs_user_auth",
        source_id: source_id.to_string(),
        source_type,
        provider,
        oauth_start_url: format!("/api/oauth/start?source_id={}", source_id),
    };
    let body_json = serde_json::to_string(&body).map_err(|e| ApiError::Internal(e.to_string()))?;
    axum::response::Response::builder()
        .status(StatusCode::PRECONDITION_FAILED)
        .header(
            header::CONTENT_TYPE,
            HeaderValue::from_static("application/json"),
        )
        .body(axum::body::Body::from(body_json))
        .map_err(|e| ApiError::Internal(e.to_string()))
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
                if (manifest.read_only || source_read_only) && action.mode == ActionMode::Write {
                    continue;
                }
                if !action.source_types.is_empty() && !action.source_types.contains(source_type) {
                    continue;
                }
                all_actions.push(json!({
                    "source_type": source_type,
                    "name": action.name,
                    "description": action.description,
                    "input_schema": action.input_schema,
                    "mode": action.mode,
                    "admin_only": action.admin_only,
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

    let source_repo = SourceRepository::new(state.db_pool.pool());
    let source = source_repo
        .find_by_id(request.source_id.clone())
        .await
        .map_err(|e| ApiError::Internal(e.to_string()))?
        .ok_or_else(|| ApiError::NotFound(format!("Source not found: {}", request.source_id)))?;

    let connector_url = get_connector_url_for_source(&state.redis_client, source.source_type)
        .await
        .ok_or_else(|| {
            ApiError::NotFound(format!(
                "Connector not registered for type: {:?}",
                source.source_type
            ))
        })?;

    let creds_repo = ServiceCredentialsRepo::new(state.db_pool.pool().clone())
        .map_err(|e| ApiError::Internal(e.to_string()))?;
    let creds = creds_repo
        .find_owner_credential(&source)
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

    let source_repo = SourceRepository::new(state.db_pool.pool());
    let source = source_repo
        .find_by_id(request.source_id.clone())
        .await
        .map_err(|e| ApiError::Internal(e.to_string()))?
        .ok_or_else(|| ApiError::NotFound(format!("Source not found: {}", request.source_id)))?;

    let connector_url = get_connector_url_for_source(&state.redis_client, source.source_type)
        .await
        .ok_or_else(|| {
            ApiError::NotFound(format!(
                "Connector not registered for type: {:?}",
                source.source_type
            ))
        })?;

    let creds_repo = ServiceCredentialsRepo::new(state.db_pool.pool().clone())
        .map_err(|e| ApiError::Internal(e.to_string()))?;
    let creds = creds_repo
        .find_owner_credential(&source)
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

    #[error("Payload too large: {0}")]
    PayloadTooLarge(String),

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
            SyncError::SyncModeUnavailable {
                source_id,
                sync_type,
            } => ApiError::BadRequest(format!(
                "{} sync is not available for source: {}",
                sync_type, source_id
            )),
            SyncError::ConcurrencyLimitReachedForSlot(slot_class) => ApiError::Conflict(format!(
                "Concurrency limit reached for {} syncs, try again later",
                slot_class
            )),
            SyncError::DatabaseError(e) => ApiError::Internal(e),
            SyncError::ConnectorError(e) => ApiError::Internal(e.to_string()),
            e @ SyncError::ConnectorTriggerTimedOut { .. } => ApiError::Internal(e.to_string()),
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
            ApiError::PayloadTooLarge(msg) => (StatusCode::PAYLOAD_TOO_LARGE, msg.clone()),
            ApiError::TooManyRequests { .. } => unreachable!(),
        };

        let body = json!({ "error": message });
        (status, Json(body)).into_response()
    }
}

// ============================================================================
// Connector Registration
// ============================================================================

const REGISTRATION_TTL_SECONDS: u64 = 300;
const UNSUPPORTED_TOP_LEVEL_ACTION_SCHEMA_KEYWORDS: &[&str] = &["anyOf", "oneOf", "allOf"];
const UNSUPPORTED_ACTION_SCHEMA_KEYWORDS: &[&str] = &[
    "$ref",
    "$defs",
    "definitions",
    "dependentRequired",
    "dependentSchemas",
    "if",
    "then",
    "else",
    "not",
];

fn validate_action_input_schema(
    connector_name: &str,
    action_name: &str,
    schema: &serde_json::Value,
) -> Result<(), String> {
    let Some(obj) = schema.as_object() else {
        return Err(format!(
            "{}.{} input_schema must be a JSON object",
            connector_name, action_name
        ));
    };

    for keyword in UNSUPPORTED_TOP_LEVEL_ACTION_SCHEMA_KEYWORDS {
        if obj.contains_key(*keyword) {
            return Err(format!(
                "{}.{} input_schema.{} is not supported at the top level",
                connector_name, action_name, keyword
            ));
        }
    }

    if let Some(schema_type) = obj.get("type") {
        let is_object_schema = schema_type.as_str().map(|t| t == "object").unwrap_or(false);
        if !is_object_schema {
            return Err(format!(
                "{}.{} input_schema.type must be \"object\"",
                connector_name, action_name
            ));
        }
    }

    validate_action_schema_keywords(connector_name, action_name, "$", schema)
}

fn validate_action_schema_keywords(
    connector_name: &str,
    action_name: &str,
    path: &str,
    value: &serde_json::Value,
) -> Result<(), String> {
    match value {
        serde_json::Value::Object(obj) => {
            for keyword in UNSUPPORTED_ACTION_SCHEMA_KEYWORDS {
                if obj.contains_key(*keyword) {
                    let schema_path = if path == "$" {
                        format!(".{}", keyword)
                    } else {
                        format!("{}.{}", path, keyword)
                    };
                    return Err(format!(
                        "{}.{} input_schema{} is not supported",
                        connector_name, action_name, schema_path
                    ));
                }
            }

            for (key, child) in obj {
                let child_path = if path == "$" {
                    format!(".{}", key)
                } else {
                    format!("{}.{}", path, key)
                };
                validate_action_schema_keywords(connector_name, action_name, &child_path, child)?;
            }
            Ok(())
        }
        serde_json::Value::Array(items) => {
            for (idx, child) in items.iter().enumerate() {
                let child_path = format!("{}[{}]", path, idx);
                validate_action_schema_keywords(connector_name, action_name, &child_path, child)?;
            }
            Ok(())
        }
        _ => Ok(()),
    }
}

fn validate_connector_manifest_action_schemas(manifest: &ConnectorManifest) -> Result<(), String> {
    for action in &manifest.actions {
        validate_action_input_schema(&manifest.name, &action.name, &action.input_schema)?;
    }
    Ok(())
}

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
    validate_connector_manifest_action_schemas(&manifest).map_err(ApiError::BadRequest)?;

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

const DOCLING_ENABLED_KEY: &str = "docling_enabled";
const DOCLING_QUALITY_PRESET_KEY: &str = "docling_quality_preset";
const DEFAULT_DOCLING_PRESET: &str = "balanced";

#[derive(Debug, Clone, serde::Deserialize)]
struct DoclingEnabledConfiguration {
    enabled: bool,
}

impl Default for DoclingEnabledConfiguration {
    fn default() -> Self {
        Self { enabled: false }
    }
}

#[derive(Debug, Clone, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
enum DoclingQualityPreset {
    Fast,
    Balanced,
    Quality,
}

impl DoclingQualityPreset {
    fn as_str(&self) -> &'static str {
        match self {
            Self::Fast => "fast",
            Self::Balanced => "balanced",
            Self::Quality => "quality",
        }
    }
}

#[derive(Debug, Clone, serde::Deserialize)]
struct DoclingQualityPresetConfiguration {
    preset: DoclingQualityPreset,
}

impl Default for DoclingQualityPresetConfiguration {
    fn default() -> Self {
        Self {
            preset: DoclingQualityPreset::Balanced,
        }
    }
}

#[derive(Debug, Clone)]
struct GlobalScopedConfiguration {
    docling_enabled: DoclingEnabledConfiguration,
    docling_quality_preset: DoclingQualityPresetConfiguration,
}

#[allow(dead_code)]
#[derive(Debug, Clone, Default)]
struct UserScopedConfiguration {}

async fn get_typed_global_configuration<T>(
    pool: &sqlx::PgPool,
    key: &str,
) -> Result<Option<T>, ApiError>
where
    T: serde::de::DeserializeOwned,
{
    let value = sqlx::query_scalar::<_, serde_json::Value>(
        "SELECT value FROM configuration WHERE scope = 'global' AND key = $1 LIMIT 1",
    )
    .bind(key)
    .fetch_optional(pool)
    .await
    .map_err(|e| {
        ApiError::Internal(format!(
            "Failed to read global configuration key '{}': {}",
            key, e
        ))
    })?;

    value
        .map(|value| {
            serde_json::from_value::<T>(value).map_err(|e| {
                ApiError::Internal(format!(
                    "Invalid global configuration value for '{}': {}",
                    key, e
                ))
            })
        })
        .transpose()
}

async fn get_global_scoped_configuration(
    pool: &sqlx::PgPool,
) -> Result<GlobalScopedConfiguration, ApiError> {
    Ok(GlobalScopedConfiguration {
        docling_enabled: get_typed_global_configuration(pool, DOCLING_ENABLED_KEY)
            .await?
            .unwrap_or_default(),
        docling_quality_preset: get_typed_global_configuration(pool, DOCLING_QUALITY_PRESET_KEY)
            .await?
            .unwrap_or_default(),
    })
}

async fn get_docling_settings(pool: &sqlx::PgPool) -> Result<(bool, String), ApiError> {
    let configuration = get_global_scoped_configuration(pool).await?;
    Ok((
        configuration.docling_enabled.enabled,
        configuration
            .docling_quality_preset
            .preset
            .as_str()
            .to_string(),
    ))
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

fn has_extension(filename: Option<&str>, expected_ext: &str) -> bool {
    filename
        .and_then(|f| f.rsplit_once('.'))
        .map(|(_, ext)| ext.eq_ignore_ascii_case(expected_ext))
        .unwrap_or(false)
}

const DEFAULT_SPREADSHEET_MAX_INDEXED_ROWS: usize = 1000;
const DEFAULT_MAX_EXTRACT_INPUT_BYTES: usize = 50 * 1024 * 1024;
const DEFAULT_MAX_EXTRACTED_TEXT_BYTES: usize = 5 * 1024 * 1024;

fn env_usize_or(key: &str, default: usize) -> usize {
    std::env::var(key)
        .ok()
        .and_then(|value| value.parse::<usize>().ok())
        .filter(|value| *value > 0)
        .unwrap_or(default)
}

fn spreadsheet_max_indexed_rows() -> usize {
    env_usize_or(
        "CONNECTOR_MANAGER_SPREADSHEET_MAX_INDEXED_ROWS",
        DEFAULT_SPREADSHEET_MAX_INDEXED_ROWS,
    )
}

fn max_extract_input_bytes() -> usize {
    env_usize_or(
        "CONNECTOR_MANAGER_MAX_EXTRACT_INPUT_BYTES",
        DEFAULT_MAX_EXTRACT_INPUT_BYTES,
    )
}

fn max_extracted_text_bytes() -> usize {
    env_usize_or(
        "CONNECTOR_MANAGER_MAX_EXTRACTED_TEXT_BYTES",
        DEFAULT_MAX_EXTRACTED_TEXT_BYTES,
    )
}

fn truncate_text_to_max_bytes(text: &str, max_bytes: usize) -> String {
    if text.len() <= max_bytes {
        return text.to_string();
    }

    let suffix =
        "\n\n[Content truncated because extracted text exceeded the configured byte limit.]";
    let include_suffix = max_bytes > suffix.len();
    let content_limit = if include_suffix {
        max_bytes - suffix.len()
    } else {
        max_bytes
    };
    let mut end = content_limit.min(text.len());
    while end > 0 && !text.is_char_boundary(end) {
        end -= 1;
    }

    let mut truncated = text[..end].to_string();
    if include_suffix {
        truncated.push_str(suffix);
    }
    truncated
}

fn is_spreadsheet_extraction_target(mime_type: &str, filename: Option<&str>) -> bool {
    matches!(
        mime_type,
        "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet"
            | "application/vnd.ms-excel"
            | "text/csv"
    ) || (mime_type == "application/octet-stream"
        && (has_extension(filename, "xlsx")
            || has_extension(filename, "xls")
            || has_extension(filename, "csv")))
}

fn maybe_filter_xlsx_extracted_text(
    mime_type: &str,
    filename: Option<&str>,
    extracted_text: &str,
) -> String {
    if is_spreadsheet_extraction_target(mime_type, filename) {
        shared::content_extractor::filter_extracted_spreadsheet_text_with_row_limit(
            extracted_text,
            Some(spreadsheet_max_indexed_rows()),
        )
    } else {
        extracted_text.to_string()
    }
}

// ============================================================================
// SDK Handlers - Called by connectors
// ============================================================================

use crate::models::{
    SdkCancelSyncRequest, SdkCancelSyncResponse, SdkCreateSyncRequest, SdkCreateSyncResponse,
    SdkEmitBatchRequest, SdkEmitEventRequest, SdkExtractContentResponse, SdkExtractTextResponse,
    SdkFailRequest, SdkIncrementScannedRequest, SdkIncrementUpdatedRequest,
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

fn acquire_extraction_permit(state: &AppState) -> Result<OwnedSemaphorePermit, ApiError> {
    state
        .extraction_semaphore
        .clone()
        .try_acquire_owned()
        .map_err(|_| ApiError::TooManyRequests {
            message: "Document extraction is busy. Try again later.".to_string(),
            retry_after_secs: state.config.extraction_retry_after_seconds,
        })
}

async fn extract_content_blocking(
    data: Vec<u8>,
    mime_type: String,
    filename: Option<String>,
) -> Result<String, ApiError> {
    let spreadsheet_max_rows = spreadsheet_max_indexed_rows();
    let is_spreadsheet = is_spreadsheet_extraction_target(&mime_type, filename.as_deref());
    tokio::task::spawn_blocking(move || {
        if is_spreadsheet {
            shared::content_extractor::extract_spreadsheet_content_with_row_limit(
                &data,
                &mime_type,
                filename.as_deref(),
                spreadsheet_max_rows,
            )
        } else {
            shared::content_extractor::extract_content(&data, &mime_type, filename.as_deref())
        }
    })
    .await
    .map_err(|e| ApiError::Internal(format!("Content extraction task failed: {}", e)))?
    .map_err(|e| ApiError::Internal(format!("Content extraction failed: {}", e)))
}

fn is_pdf_extraction_target(mime_type: &str, filename: Option<&str>) -> bool {
    matches!(mime_type, "application/pdf" | "application/x-pdf")
        || (mime_type == "application/octet-stream" && has_extension(filename, "pdf"))
}

async fn extract_content(
    data: Vec<u8>,
    mime_type: String,
    filename: Option<String>,
    sync_run_id: &str,
    source_id: Option<&str>,
) -> Result<String, ApiError> {
    let is_pdf = is_pdf_extraction_target(&mime_type, filename.as_deref());
    match extract_content_blocking(data, mime_type.clone(), filename.clone()).await {
        Ok(text) if is_pdf && text.trim().is_empty() => {
            warn!(
                "PDF text extraction produced no text; sync_run_id={}, source_id={:?}, filename={:?}, mime_type={}",
                sync_run_id,
                source_id,
                filename,
                mime_type,
            );
            Ok("[Text extraction failed for this PDF. The document was skipped for extracted-text indexing because no text could be extracted.]".to_string())
        }
        Ok(text) => Ok(text),
        Err(e) if is_pdf => {
            warn!(
                "PDF text extraction failed; sync_run_id={}, source_id={:?}, filename={:?}, mime_type={}, reason={}",
                sync_run_id,
                source_id,
                filename,
                mime_type,
                e,
            );
            Ok(format!(
                "[Text extraction failed for this PDF. The document was skipped for extracted-text indexing. Reason: {}]",
                e
            ))
        }
        Err(e) => Err(e),
    }
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
                let bytes = field
                    .bytes()
                    .await
                    .map_err(|e| ApiError::BadRequest(format!("Failed to read data: {}", e)))?;
                let max_bytes = max_extract_input_bytes();
                if bytes.len() > max_bytes {
                    return Err(ApiError::PayloadTooLarge(format!(
                        "Extraction input too large: {} bytes exceeds {} byte limit",
                        bytes.len(),
                        max_bytes
                    )));
                }
                data = Some(bytes.to_vec());
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
    pool: &sqlx::PgPool,
    sync_run_id: &str,
    source_id: Option<&str>,
    mime_type: String,
    filename: Option<String>,
    data: Vec<u8>,
) -> Result<String, ApiError> {
    let is_spreadsheet = is_spreadsheet_extraction_target(&mime_type, filename.as_deref());
    let docling_candidate = is_docling_supported_mime(&mime_type)
        || (mime_type == "application/octet-stream"
            && is_docling_supported_extension(filename.as_deref()));
    let (docling_enabled, preset) = if docling_candidate {
        get_docling_settings(pool).await?
    } else {
        (false, DEFAULT_DOCLING_PRESET.to_string())
    };

    let extracted_text = if docling_candidate && docling_enabled {
        let docling_result = if let Some(client) = DoclingClient::from_env() {
            let file_name = filename.as_deref().unwrap_or("document");
            debug!(
                "Using docling-based document content extraction for file '{}' (preset={})",
                file_name, preset
            );
            match client.convert(&data, file_name, &preset).await {
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

        if let Some(markdown) = docling_result {
            markdown
        } else {
            debug!(
                "Using built-in document content extraction for file {:?}",
                filename
            );
            extract_content(
                data,
                mime_type.clone(),
                filename.clone(),
                sync_run_id,
                source_id,
            )
            .await?
        }
    } else {
        debug!("Using built-in document content extraction for file {:?} (docling_enabled={}, docling_candidate={})", filename, docling_enabled, docling_candidate);
        extract_content(
            data,
            mime_type.clone(),
            filename.clone(),
            sync_run_id,
            source_id,
        )
        .await?
    };

    let processed_text = if is_spreadsheet {
        maybe_filter_xlsx_extracted_text(&mime_type, filename.as_deref(), &extracted_text)
    } else {
        extracted_text
    };

    let max_bytes = max_extracted_text_bytes();
    if processed_text.len() > max_bytes {
        warn!(
            "Truncating extracted content for {:?}: {} bytes > {} byte limit",
            filename,
            processed_text.len(),
            max_bytes
        );
    }
    Ok(truncate_text_to_max_bytes(&processed_text, max_bytes))
}

pub async fn sdk_extract_content(
    State(state): State<AppState>,
    multipart: axum::extract::Multipart,
) -> Result<Json<SdkExtractContentResponse>, ApiError> {
    let _permit = acquire_extraction_permit(&state)?;
    let fields = parse_extract_multipart(multipart).await?;

    debug!(
        "SDK: Extracting content for sync_run={}, mime={}, filename={:?}, size={}",
        fields.sync_run_id,
        fields.mime_type,
        fields.filename,
        fields.data.len()
    );

    let sync_run_repo = SyncRunRepository::new(state.db_pool.pool());
    let source_id = sync_run_repo
        .find_by_id(&fields.sync_run_id)
        .await
        .map_err(|e| ApiError::Internal(format!("Failed to load sync run: {}", e)))?
        .map(|sync_run| sync_run.source_id);

    let extracted_text = do_extract_text(
        state.db_pool.pool(),
        &fields.sync_run_id,
        source_id.as_deref(),
        fields.mime_type.clone(),
        fields.filename.clone(),
        fields.data,
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
    let _permit = acquire_extraction_permit(&state)?;
    let fields = parse_extract_multipart(multipart).await?;

    debug!(
        "SDK: Extracting text for sync_run={}, mime={}, filename={:?}, size={}",
        fields.sync_run_id,
        fields.mime_type,
        fields.filename,
        fields.data.len()
    );

    let sync_run_repo = SyncRunRepository::new(state.db_pool.pool());
    let source_id = sync_run_repo
        .find_by_id(&fields.sync_run_id)
        .await
        .map_err(|e| ApiError::Internal(format!("Failed to load sync run: {}", e)))?
        .map(|sync_run| sync_run.source_id);

    let extracted_text = do_extract_text(
        state.db_pool.pool(),
        &fields.sync_run_id,
        source_id.as_deref(),
        fields.mime_type.clone(),
        fields.filename.clone(),
        fields.data,
    )
    .await?;

    let text = utils::normalize_whitespace(&extracted_text);

    // Update heartbeat
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

    let normalized_content = utils::normalize_whitespace(&request.content);
    let max_bytes = max_extracted_text_bytes();
    if normalized_content.len() > max_bytes {
        warn!(
            "Truncating stored content for sync_run={}: {} bytes > {} byte limit",
            request.sync_run_id,
            normalized_content.len(),
            max_bytes
        );
    }
    let content = truncate_text_to_max_bytes(&normalized_content, max_bytes);
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
) -> Result<Json<SdkStatusResponse>, ApiError> {
    info!("SDK: Completing sync_run={}", sync_run_id);

    let sync_run_repo = SyncRunRepository::new(state.db_pool.pool());

    // Atomically mark completed and publish this run's checkpoint to the source.
    let updated = sync_run_repo
        .complete_and_publish_checkpoint(&sync_run_id)
        .await
        .map_err(|e| ApiError::Internal(format!("Failed to mark completed: {}", e)))?;
    if !updated {
        warn!(
            "SDK: Ignoring stale complete for non-running sync_run={}",
            sync_run_id
        );
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
    let updated = sync_run_repo
        .mark_failed(&sync_run_id, &request.error)
        .await
        .map_err(|e| ApiError::Internal(format!("Failed to mark failed: {}", e)))?;
    if !updated {
        warn!(
            "SDK: Ignoring stale fail for non-running sync_run={}",
            sync_run_id
        );
    }

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
    let updated = sync_run_repo
        .increment_scanned(&sync_run_id, request.count)
        .await
        .map_err(|e| ApiError::Internal(format!("Failed to increment scanned: {}", e)))?;
    if !updated {
        warn!(
            "SDK: Ignoring stale scanned increment for non-running sync_run={}",
            sync_run_id
        );
    }

    Ok(Json(SdkStatusResponse {
        status: "ok".to_string(),
    }))
}

pub async fn sdk_increment_updated(
    State(state): State<AppState>,
    Path(sync_run_id): Path<String>,
    Json(request): Json<SdkIncrementUpdatedRequest>,
) -> Result<Json<SdkStatusResponse>, ApiError> {
    debug!(
        "SDK: Incrementing updated for sync_run={} by {}",
        sync_run_id, request.count
    );

    let sync_run_repo = SyncRunRepository::new(state.db_pool.pool());
    let updated = sync_run_repo
        .increment_updated(&sync_run_id, request.count)
        .await
        .map_err(|e| ApiError::Internal(format!("Failed to increment updated: {}", e)))?;
    if !updated {
        warn!(
            "SDK: Ignoring stale updated increment for non-running sync_run={}",
            sync_run_id
        );
    }

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
) -> Result<Json<shared::models::ServiceCredential>, ApiError> {
    debug!("SDK: Getting credentials for source_id={}", source_id);

    let source_repo = SourceRepository::new(state.db_pool.pool());
    let source = source_repo
        .find_by_id(source_id.clone())
        .await
        .map_err(|e| ApiError::Internal(format!("Database error: {}", e)))?
        .ok_or_else(|| ApiError::NotFound(format!("Source not found: {}", source_id)))?;

    let creds_repo = ServiceCredentialsRepo::new(state.db_pool.pool().clone())
        .map_err(|e| ApiError::Internal(format!("Failed to create credentials repo: {}", e)))?;

    let creds = creds_repo
        .find_owner_credential(&source)
        .await
        .map_err(|e| ApiError::Internal(format!("Database error: {}", e)))?
        .ok_or_else(|| {
            ApiError::NotFound(format!("Credentials not found for source: {}", source_id))
        })?;

    Ok(Json(creds))
}

// TODO: drop this endpoint once the Python SDK is updated to fetch source +
// credentials separately (matching the Rust SDK). Today the Rust SDK passes
// full Source/ServiceCredential directly to Connector::sync, so it has no
// need for this bundled endpoint — only Python connectors still call it.
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
        .find_owner_credential(&source)
        .await
        .map_err(|e| ApiError::Internal(format!("Database error: {}", e)))?
        .map(|c| c.credentials)
        .unwrap_or_else(|| serde_json::json!({}));

    Ok(Json(SdkSourceSyncConfigResponse {
        config: source.config,
        credentials,
        connector_state: source.connector_state,
        checkpoint: source.checkpoint,
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

    let source_repo = SourceRepository::new(state.db_pool.pool());
    let source = source_repo
        .find_by_id(request.source_id.clone())
        .await
        .map_err(|e| ApiError::Internal(format!("Database error: {}", e)))?
        .ok_or_else(|| ApiError::NotFound(format!("Source not found: {}", request.source_id)))?;
    if !source.is_active {
        return Err(ApiError::BadRequest(format!(
            "Source is inactive: {}",
            request.source_id
        )));
    }
    if state
        .sync_manager
        .is_sync_class_running(&request.source_id, request.sync_type.slot_class())
        .await?
    {
        return Err(ApiError::Conflict(format!(
            "Sync already running for source: {}",
            request.source_id
        )));
    }
    if state.sync_manager.active_sync_count().await? >= state.config.max_concurrent_syncs {
        return Err(ApiError::Conflict(
            "Concurrency limit reached, try again later".to_string(),
        ));
    }
    let slot_class = request.sync_type.slot_class();
    if state
        .sync_manager
        .active_sync_count_for_slot_class(slot_class)
        .await?
        >= state.config.max_concurrent_syncs_per_type
    {
        return Err(ApiError::Conflict(format!(
            "Concurrency limit reached for {} syncs, try again later",
            slot_class
        )));
    }

    let sync_run_repo = SyncRunRepository::new(state.db_pool.pool());
    let sync_run = sync_run_repo
        .create(&request.source_id, request.sync_type, "manual")
        .await
        .map_err(|e| match e {
            shared::db::error::DatabaseError::RunningSyncSlotConflict => ApiError::Conflict(
                format!("Sync already running for source: {}", request.source_id),
            ),
            other => ApiError::Internal(format!("Failed to create sync run: {}", other)),
        })?;

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
    let updated = sync_run_repo
        .mark_cancelled(&request.sync_run_id)
        .await
        .map_err(|e| ApiError::Internal(format!("Failed to cancel sync: {}", e)))?;
    if !updated {
        warn!(
            "SDK: Ignoring stale cancel for non-running sync_run={}",
            request.sync_run_id
        );
    }

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

pub async fn sdk_update_checkpoint(
    State(state): State<AppState>,
    Path(sync_run_id): Path<String>,
    Json(checkpoint): Json<serde_json::Value>,
) -> Result<Json<SdkStatusResponse>, ApiError> {
    debug!("SDK: Updating checkpoint for sync_run={}", sync_run_id);

    let sync_run_repo = SyncRunRepository::new(state.db_pool.pool());
    let updated = sync_run_repo
        .update_checkpoint(&sync_run_id, checkpoint)
        .await
        .map_err(|e| ApiError::Internal(format!("Failed to update checkpoint: {}", e)))?;
    if !updated {
        warn!(
            "SDK: Ignoring stale checkpoint update for non-running sync_run={}",
            sync_run_id
        );
    }

    Ok(Json(SdkStatusResponse {
        status: "ok".to_string(),
    }))
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

    fn manifest_with_action_schema(input_schema: serde_json::Value) -> ConnectorManifest {
        ConnectorManifest {
            name: "test_connector".to_string(),
            display_name: "Test Connector".to_string(),
            version: "1.0.0".to_string(),
            sync_modes: vec![SyncType::Full],
            connector_id: "test-connector".to_string(),
            connector_url: "http://test-connector:4000".to_string(),
            source_types: vec![SourceType::Notion],
            description: None,
            actions: vec![shared::models::ActionDefinition {
                name: "export_data_source_csv".to_string(),
                description: "Export a database".to_string(),
                input_schema,
                mode: ActionMode::Read,
                source_types: Vec::new(),
                admin_only: false,
            }],
            search_operators: Vec::new(),
            read_only: false,
            extra_schema: None,
            attributes_schema: None,
            mcp_enabled: false,
            resources: Vec::new(),
            prompts: Vec::new(),
            oauth: None,
        }
    }

    #[test]
    fn test_validate_action_schema_accepts_provider_safe_object_schema() {
        let manifest = manifest_with_action_schema(json!({
            "type": "object",
            "properties": {
                "data_source_id": {
                    "type": "string",
                    "description": "The Notion data source ID to export."
                },
                "include_content": {
                    "type": "boolean",
                    "default": false
                }
            },
            "required": ["data_source_id"]
        }));

        assert!(validate_connector_manifest_action_schemas(&manifest).is_ok());
    }

    #[test]
    fn test_validate_action_schema_rejects_top_level_any_of() {
        let manifest = manifest_with_action_schema(json!({
            "type": "object",
            "properties": {
                "data_source_id": { "type": "string" },
                "database_id": { "type": "string" }
            },
            "anyOf": [
                { "required": ["data_source_id"] },
                { "required": ["database_id"] }
            ]
        }));

        let err = validate_connector_manifest_action_schemas(&manifest).unwrap_err();
        assert_eq!(
            err,
            "test_connector.export_data_source_csv input_schema.anyOf is not supported at the top level"
        );
    }

    #[test]
    fn test_validate_action_schema_rejects_ref_in_nested_property() {
        let manifest = manifest_with_action_schema(json!({
            "type": "object",
            "properties": {
                "target": { "$ref": "#/$defs/Target" }
            }
        }));

        let err = validate_connector_manifest_action_schemas(&manifest).unwrap_err();
        assert_eq!(
            err,
            "test_connector.export_data_source_csv input_schema.properties.target.$ref is not supported"
        );
    }

    #[test]
    fn test_validate_action_schema_allows_legacy_empty_object_schema() {
        let manifest = manifest_with_action_schema(json!({}));

        assert!(validate_connector_manifest_action_schemas(&manifest).is_ok());
    }

    #[tokio::test]
    async fn test_malformed_pdf_extraction_returns_failure_marker() {
        let data = b"%PDF-1.4\nmalformed body\n%%EOF".to_vec();
        let text = extract_content(
            data,
            "application/pdf".to_string(),
            Some("bad.pdf".to_string()),
            "sync-test",
            Some("source-test"),
        )
        .await
        .expect("malformed PDF should be handled predictably");

        assert!(text.contains("Text extraction failed for this PDF"));
    }

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

    #[test]
    fn test_truncate_text_to_max_bytes_respects_utf8_boundary() {
        let text = "abc😀def";
        let truncated = truncate_text_to_max_bytes(text, 8);

        assert!(truncated.is_char_boundary(truncated.len()));
        assert!(truncated.len() <= 8);
        assert!(truncated.starts_with("abc"));
    }

    #[test]
    fn test_is_spreadsheet_extraction_target() {
        assert!(is_spreadsheet_extraction_target(
            "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet",
            None
        ));
        assert!(is_spreadsheet_extraction_target(
            "application/vnd.ms-excel",
            Some("legacy.xls")
        ));
        assert!(is_spreadsheet_extraction_target("text/csv", None));
        assert!(is_spreadsheet_extraction_target(
            "application/octet-stream",
            Some("Report.XLSX")
        ));
        assert!(is_spreadsheet_extraction_target(
            "application/octet-stream",
            Some("legacy.xls")
        ));
        assert!(is_spreadsheet_extraction_target(
            "application/octet-stream",
            Some("data.csv")
        ));
        assert!(!is_spreadsheet_extraction_target(
            "application/octet-stream",
            Some("notes.txt")
        ));
        assert!(!is_spreadsheet_extraction_target(
            "application/pdf",
            Some("sheet.xlsx")
        ));
    }

    #[test]
    fn test_xlsx_post_processing_for_tab_separated_output() {
        let input = "Name\tAge\tCost\nAlice\t30\t$10.00\n123\t456\nQ4 revenue\t1.2e6\n";
        let filtered = shared::content_extractor::filter_extracted_spreadsheet_text(input);

        assert!(filtered.contains("Name\tAge\tCost"));
        assert!(filtered.contains("Alice"));
        assert!(filtered.contains("Q4 revenue"));
        assert!(!filtered.contains("30"));
        assert!(!filtered.contains("$10.00"));
        assert!(!filtered.contains("123\t456"));
        assert!(!filtered.contains("1.2e6"));
    }

    #[test]
    fn test_xlsx_post_processing_for_markdown_table_output() {
        let input = concat!(
            "| Product | Count | Price |\n",
            "| --- | --- | --- |\n",
            "| Widget A | 100 | $9.99 |\n",
            "| 111 | 222 | 333 |\n"
        );
        let filtered = shared::content_extractor::filter_extracted_spreadsheet_text(input);

        assert!(filtered.contains("| Product | Count | Price |"));
        assert!(filtered.contains("| Widget A |"));
        assert!(!filtered.contains("$9.99"));
        assert!(!filtered.contains("| 111 | 222 | 333 |"));
    }

    #[test]
    fn test_non_spreadsheet_documents_do_not_match_spreadsheet_post_processing_target() {
        assert!(!is_spreadsheet_extraction_target(
            "application/vnd.openxmlformats-officedocument.wordprocessingml.document",
            Some("doc.docx")
        ));
        assert!(!is_spreadsheet_extraction_target(
            "text/plain",
            Some("notes.txt")
        ));

        let text = "Report total\n123\t456\n";
        assert_eq!(
            maybe_filter_xlsx_extracted_text("text/plain", Some("notes.txt"), text),
            text
        );
    }
}

use crate::client::{SdkClient, SdkError, build_connector_url};
use crate::connector::{Connector, SyncRequestValidationError};
use crate::context::SyncContext;
use crate::mcp_adapter::{MCP_AUTH_REQUIRED_MESSAGE, McpAdapter, McpServer};
use crate::models::{
    ActionRequest, ActionResponse, CancelRequest, CancelResponse, McpCredentials,
    OAuthCredentialReadyRequest, PromptRequest, ResourceRequest, SkillRequest, SkillResponse,
    SyncRequest, SyncResponse, SyncStatusResponse,
};
use shared::models::OAuthCredentialValidationRequest;
use anyhow::{Context, Result};
use axum::{
    Router,
    extract::{DefaultBodyLimit, Path, State},
    http::StatusCode,
    middleware,
    response::{IntoResponse, Json, Response},
    routing::{get, post},
};
use dashmap::DashMap;
use dashmap::mapref::entry::Entry;
use serde::de::DeserializeOwned;
use shared::models::{ConnectorSkillDefinition, SourceType, SyncSlotClass, SyncType};
use shared::telemetry;
use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, OnceLock};
use tokio::time::{Duration, interval};
use tower::ServiceBuilder;
use tower_http::cors::CorsLayer;
use tracing::{error, info, warn};

#[derive(Clone)]
pub struct ServerConfig {
    pub port: u16,
    pub connector_url: String,
}

impl ServerConfig {
    pub fn from_env() -> Result<Self> {
        let port = std::env::var("PORT")
            .context("PORT environment variable must be set")?
            .parse::<u16>()
            .context("PORT must be a valid u16")?;

        Ok(Self {
            port,
            connector_url: build_connector_url(),
        })
    }
}

struct ActiveSync {
    sync_run_id: String,
    cancelled: Arc<AtomicBool>,
}

/// Slot key: (source_id, sync_class). Realtime watchers and scheduled (Full /
/// Incremental) syncs occupy independent slots, so a long-running realtime
/// watcher does not block scheduled scans for the same source.
type SlotKey = (String, SyncSlotClass);
type ActiveSyncs = Arc<DashMap<SlotKey, ActiveSync>>;

/// Reserves a slot in `active_syncs` for a (source, sync_class) pair. The slot
/// is released when this guard is dropped, including on panic inside the
/// spawned sync task — so the entry cannot leak even if the connector's
/// `sync()` implementation panics.
struct ActiveSyncGuard {
    active_syncs: ActiveSyncs,
    slot_key: SlotKey,
    sync_run_id: String,
}

impl ActiveSyncGuard {
    /// Atomically reserve a slot. Returns `None` if a sync of the same class
    /// is already active for this source.
    fn reserve(
        active_syncs: ActiveSyncs,
        source_id: String,
        slot_class: SyncSlotClass,
        sync_run_id: String,
        cancelled: Arc<AtomicBool>,
    ) -> Option<Self> {
        let slot_key = (source_id, slot_class);
        let inserted = match active_syncs.entry(slot_key.clone()) {
            Entry::Occupied(_) => false,
            Entry::Vacant(vacant) => {
                vacant.insert(ActiveSync {
                    sync_run_id: sync_run_id.clone(),
                    cancelled,
                });
                true
            }
        };
        if inserted {
            Some(Self {
                active_syncs,
                slot_key,
                sync_run_id,
            })
        } else {
            None
        }
    }
}

impl Drop for ActiveSyncGuard {
    fn drop(&mut self) {
        self.active_syncs.remove_if(&self.slot_key, |_, sync| {
            sync.sync_run_id == self.sync_run_id
        });
    }
}

struct ServerState<C: Connector> {
    connector: Arc<C>,
    sdk_client: SdkClient,
    connector_url: String,
    active_syncs: ActiveSyncs,
    mcp_adapter: OnceLock<Option<Arc<McpAdapter>>>,
}

impl<C: Connector> ServerState<C> {
    fn new(connector: Arc<C>, sdk_client: SdkClient, connector_url: String) -> Self {
        Self {
            connector,
            sdk_client,
            connector_url,
            active_syncs: Arc::new(DashMap::new()),
            mcp_adapter: OnceLock::new(),
        }
    }

    fn mcp_adapter(&self) -> Option<&Arc<McpAdapter>> {
        self.mcp_adapter
            .get_or_init(|| {
                self.connector
                    .mcp_server()
                    .map(|server| Arc::new(McpAdapter::new(server)))
            })
            .as_ref()
    }
}

/// Build the env-vs-headers tuple to pass to the MCP adapter, dispatching
/// based on the configured transport variant.
async fn build_mcp_auth<C: Connector>(
    connector: &C,
    credentials: &McpCredentials,
) -> Result<(
    Option<HashMap<String, String>>,
    Option<HashMap<String, String>>,
)> {
    match connector.mcp_server() {
        Some(McpServer::Http(_)) => Ok((
            None,
            Some(connector.prepare_mcp_headers(credentials).await?),
        )),
        Some(McpServer::Stdio(_)) => {
            Ok((Some(connector.prepare_mcp_env(credentials).await?), None))
        }
        None => Ok((None, None)),
    }
}

pub fn create_router<C>(connector: Arc<C>, sdk_client: SdkClient, connector_url: String) -> Router
where
    C: Connector,
{
    let state = Arc::new(ServerState::new(connector, sdk_client, connector_url));
    create_router_with_state(state)
}

fn create_router_with_state<C>(state: Arc<ServerState<C>>) -> Router
where
    C: Connector,
{
    Router::new()
        .route("/health", get(health::<C>))
        .route("/manifest", get(manifest::<C>))
        .route("/sync", post(trigger_sync::<C>))
        .route("/sync/:sync_run_id", get(sync_status::<C>))
        .route("/cancel", post(cancel_sync::<C>))
        .route("/oauth/credential-ready", post(oauth_credential_ready::<C>))
        .route("/oauth/validate", post(oauth_validate::<C>))
        .route("/action", post(execute_action::<C>))
        .route("/resource", post(read_resource::<C>))
        .route("/prompt", post(get_prompt::<C>))
        .route("/skill", post(get_skill::<C>))
        .layer(DefaultBodyLimit::disable())
        .layer(
            ServiceBuilder::new()
                .layer(middleware::from_fn(telemetry::middleware::trace_layer))
                .layer(CorsLayer::permissive()),
        )
        .with_state(state)
}

pub async fn serve<C>(connector: C) -> Result<()>
where
    C: Connector,
{
    serve_with_config(connector, ServerConfig::from_env()?).await
}

pub async fn serve_with_config<C>(connector: C, config: ServerConfig) -> Result<()>
where
    C: Connector,
{
    serve_with_extra_routes(connector, config, Router::new()).await
}

/// Start the connector server with additional HTTP routes merged in alongside
/// the SDK-provided routes. Extra paths must not collide with the SDK's
/// reserved paths (`/health`, `/manifest`, `/sync`, `/sync/:sync_run_id`,
/// `/cancel`, `/action`, `/resource`, `/prompt`, `/skill`, `/oauth/validate`,
/// `/oauth/credential-ready`) — collisions cause axum to
/// panic at startup.
///
/// Connectors that need to return binary data from actions should return
/// `ActionResult::Binary` from `execute_action` instead of using extra routes
/// for `/action`.
pub async fn serve_with_extra_routes<C>(
    connector: C,
    config: ServerConfig,
    extra_routes: Router,
) -> Result<()>
where
    C: Connector,
{
    let connector = Arc::new(connector);
    let sdk_client = SdkClient::from_env()?;

    // Bind before the registration loop so that any connector-manager callback
    // triggered by registration finds the HTTP server already accepting
    // connections.
    let addr = std::net::SocketAddr::from(([0, 0, 0, 0], config.port));
    let listener = tokio::net::TcpListener::bind(addr).await?;
    info!("HTTP server listening on {}", addr);

    let state = Arc::new(ServerState::new(
        connector,
        sdk_client.clone(),
        config.connector_url.clone(),
    ));
    start_registration_loop(Arc::clone(&state));

    let app = create_router_with_state(state).merge(extra_routes);
    axum::serve(listener, app).await?;
    Ok(())
}

fn start_registration_loop<C>(state: Arc<ServerState<C>>) -> tokio::task::JoinHandle<()>
where
    C: Connector,
{
    tokio::spawn(async move {
        let mut ticker = interval(Duration::from_secs(30));
        let mut last_was_ok: Option<bool> = None;
        loop {
            ticker.tick().await;
            let manifest = build_manifest_with_mcp(&state).await;
            match state.sdk_client.register(&manifest).await {
                Ok(()) => {
                    if last_was_ok != Some(true) {
                        info!("Registered with connector manager");
                    }
                    last_was_ok = Some(true);
                }
                Err(error) => {
                    if last_was_ok != Some(false) {
                        warn!("Registration failed: {}", error);
                    }
                    last_was_ok = Some(false);
                }
            }
        }
    })
}

async fn health<C>(State(state): State<Arc<ServerState<C>>>) -> impl IntoResponse
where
    C: Connector,
{
    Json(serde_json::json!({
        "status": "healthy",
        "service": format!("{}-connector", state.connector.name()),
    }))
}

async fn manifest<C>(State(state): State<Arc<ServerState<C>>>) -> impl IntoResponse
where
    C: Connector,
{
    Json(build_manifest_with_mcp(&state).await)
}

async fn build_manifest_with_mcp<C>(state: &ServerState<C>) -> shared::models::ConnectorManifest
where
    C: Connector,
{
    let mut manifest = state
        .connector
        .build_manifest(state.connector_url.clone())
        .await;

    // If MCP is configured, layer the cached tools/resources/prompts from the
    // adapter on top of any actions the connector defined manually.
    if let Some(adapter) = state.mcp_adapter() {
        manifest.mcp_catalog_loaded = adapter.has_cached_catalog();
        match adapter.get_action_definitions(None, None).await {
            Ok(mcp_actions) => {
                let manual: std::collections::HashSet<String> =
                    manifest.actions.iter().map(|a| a.name.clone()).collect();
                for mut action in mcp_actions {
                    if action.source_types.is_empty() {
                        action.source_types = manifest
                            .source_types
                            .iter()
                            .filter_map(|source_type| {
                                SourceType::try_from(source_type.as_str()).ok()
                            })
                            .collect();
                    }
                    if !manual.contains(&action.name) {
                        action.origin = shared::models::ActionOrigin::Mcp;
                        manifest.actions.push(action);
                    }
                }
            }
            Err(e) => warn!("Failed to merge MCP actions into manifest: {}", e),
        }
        match adapter.get_resource_definitions(None, None).await {
            Ok(resources) => manifest.resources = resources,
            Err(e) => warn!("Failed to fetch MCP resources for manifest: {}", e),
        }
        match adapter.get_prompt_definitions(None, None).await {
            Ok(prompts) => {
                let manual: std::collections::HashSet<String> = manifest
                    .skills
                    .iter()
                    .map(|skill| skill.id.clone())
                    .collect();
                manifest.prompts = prompts.clone();
                for prompt in prompts {
                    let skill = mcp_prompt_skill(&prompt);
                    if !manual.contains(&skill.id) {
                        manifest.skills.push(skill);
                    }
                }
            }
            Err(e) => warn!("Failed to fetch MCP prompts for manifest: {}", e),
        }
    }

    manifest
}

/// Whether an MCP failure message is a terminal authentication rejection
/// that requires the acting user's OAuth reconnection.
fn mcp_auth_required<C: Connector>(connector: &C, message: &str) -> bool {
    message == MCP_AUTH_REQUIRED_MESSAGE || connector.mcp_authentication_error(message)
}

/// Build the stable 412 `needs_user_auth` response body. Mirrors the Python
/// SDK's response so connector-manager invalidates the user's credential and
/// the web layer can surface the same "connect" CTA. Returns `None` when the
/// acting source cannot be identified.
fn mcp_auth_required_body<C: Connector>(
    connector: &C,
    source_id: Option<&str>,
    source_type: Option<&str>,
) -> Option<serde_json::Value> {
    let source_id = source_id?;
    let source_type = source_type?;
    Some(serde_json::json!({
        "error": "needs_user_auth",
        "source_id": source_id,
        "source_type": source_type,
        "provider": connector.oauth_config().map(|config| config.provider),
        "oauth_start_url": format!("/api/oauth/start?source_id={}", source_id),
    }))
}

/// 412 `needs_user_auth` axum response for MCP action failures.
fn mcp_auth_required_action_response<C: Connector>(
    state: &ServerState<C>,
    request: &ActionRequest,
    message: &str,
) -> Option<Response> {
    if !mcp_auth_required(state.connector.as_ref(), message) {
        return None;
    }
    let source = request.source.as_ref();
    let source_id = source.map(|source| source.id.as_str()).or_else(|| {
        request
            .credentials
            .as_ref()
            .map(|credentials| credentials.source_id.as_str())
    });
    let source_type = source
        .map(|source| source.source_type.as_str())
        .or_else(|| state.connector.source_types().first().map(|t| t.as_str()));
    let body = mcp_auth_required_body(state.connector.as_ref(), source_id, source_type)?;
    Some(
        (
            StatusCode::PRECONDITION_FAILED,
            [("content-type", "application/json")],
            body.to_string(),
        )
            .into_response(),
    )
}

/// 412 `needs_user_auth` response for MCP resource/prompt failures, which
/// carry source identity on the credentials wrapper rather than a Source.
fn mcp_auth_required_credentials_response<C: Connector>(
    connector: &C,
    credentials: &McpCredentials,
    message: &str,
) -> Option<serde_json::Value> {
    if !mcp_auth_required(connector, message) {
        return None;
    }
    let source_type = connector.source_types().first().map(|t| t.as_str());
    mcp_auth_required_body(connector, credentials.source_id.as_deref(), source_type)
}

fn mcp_prompt_skill(prompt: &shared::models::McpPromptDefinition) -> ConnectorSkillDefinition {
    ConnectorSkillDefinition {
        id: format!("mcp:{}", prompt.name),
        title: prompt.name.clone(),
        description: prompt.description.clone(),
        source_types: vec![],
        content: None,
        mcp_prompt: Some(prompt.name.clone()),
    }
}

async fn sync_status<C>(
    State(state): State<Arc<ServerState<C>>>,
    Path(sync_run_id): Path<String>,
) -> impl IntoResponse
where
    C: Connector,
{
    let running = state
        .active_syncs
        .iter()
        .any(|sync| sync.sync_run_id == sync_run_id);
    Json(SyncStatusResponse { running })
}

async fn trigger_sync<C>(
    State(state): State<Arc<ServerState<C>>>,
    Json(request): Json<SyncRequest>,
) -> Result<Json<SyncResponse>, (StatusCode, Json<SyncResponse>)>
where
    C: Connector,
{
    let sync_run_id = request.sync_run_id.clone();
    let source_id = request.source_id.clone();

    info!(
        "Sync triggered for source {} (sync_run_id: {})",
        source_id, sync_run_id
    );

    let source = state
        .sdk_client
        .get_source(&source_id)
        .await
        .map_err(map_source_fetch_error)?;

    let credentials = if state.connector.requires_credentials() {
        Some(
            state
                .sdk_client
                .get_credentials(&source_id)
                .await
                .map_err(map_source_fetch_error)?,
        )
    } else {
        None
    };

    // Boundary validation: probe-decode source.config and creds.credentials
    // against the connector's declared shapes. The decoded values are dropped
    // — the connector receives the full structs and re-decodes its typed view
    // inside `sync()` if it wants one. This catches malformed payloads at
    // dispatch time so a bad source returns 400 right away.
    decode::<C::Config>(&source.config, "source config").map_err(|error| {
        (
            StatusCode::BAD_REQUEST,
            Json(SyncResponse::error(error.to_string())),
        )
    })?;
    if let Some(creds) = &credentials {
        decode::<C::Credentials>(&creds.credentials, "credentials").map_err(|error| {
            (
                StatusCode::BAD_REQUEST,
                Json(SyncResponse::error(error.to_string())),
            )
        })?;
    }

    let effective_checkpoint = request.checkpoint.as_ref().or(source.checkpoint.as_ref());
    let typed_state = decode_optional::<C::State>(effective_checkpoint, "sync checkpoint")
        .map_err(|error| {
            (
                StatusCode::BAD_REQUEST,
                Json(SyncResponse::error(error.to_string())),
            )
        })?;

    state
        .connector
        .validate_sync_request(&source, credentials.as_ref(), request.sync_mode)
        .await
        .map_err(|error| match error {
            SyncRequestValidationError::Unavailable(message) => {
                (StatusCode::NOT_FOUND, Json(SyncResponse::error(message)))
            }
            SyncRequestValidationError::BadRequest(message) => {
                (StatusCode::BAD_REQUEST, Json(SyncResponse::error(message)))
            }
        })?;

    let cancelled = Arc::new(AtomicBool::new(false));
    let slot_class = request.sync_mode.slot_class();
    let Some(guard) = ActiveSyncGuard::reserve(
        Arc::clone(&state.active_syncs),
        source_id.clone(),
        slot_class,
        sync_run_id.clone(),
        Arc::clone(&cancelled),
    ) else {
        return Err((
            StatusCode::CONFLICT,
            Json(SyncResponse::error(format!(
                "{} sync already in progress for this source",
                slot_class
            ))),
        ));
    };

    // Bootstrap MCP discovery now that we have credentials. Populates the
    // adapter's cache so subsequent /manifest reads (which run without creds)
    // can return the live tool/resource/prompt list.
    if let Some(adapter) = state.mcp_adapter() {
        let creds = credentials
            .as_ref()
            .map(McpCredentials::from_service_credential)
            .unwrap_or_default();
        match build_mcp_auth(&*state.connector, &creds).await {
            Ok((env, headers)) => {
                if let Err(e) = adapter.discover(env, headers).await {
                    warn!("MCP bootstrap failed: {}", e);
                }
            }
            Err(e) => warn!("MCP auth preparation failed: {:#}", e),
        }
    }

    state
        .sdk_client
        .register_sync(&sync_run_id, request.sync_mode)
        .await;

    let native_source_type = SourceType::try_from(source.source_type.as_str()).map_err(|e| {
        (
            StatusCode::BAD_REQUEST,
            Json(SyncResponse::error(format!(
                "source {} is not a native connector source: {}",
                source_id, e
            ))),
        )
    })?;

    let ctx = SyncContext::new_with_resume(
        state.sdk_client.clone(),
        sync_run_id.clone(),
        source_id.clone(),
        native_source_type,
        request.sync_mode,
        request.is_resume,
        cancelled,
    );
    let connector = Arc::clone(&state.connector);

    tokio::spawn(async move {
        // Moved into the task so the slot is released when this future
        // completes — including on panic, which unwinds through locals.
        let _slot = guard;
        let result = connector
            .sync(source, credentials, typed_state, ctx.clone())
            .await;

        match result {
            Ok(()) => {
                if ctx.sync_mode() != SyncType::Realtime && !ctx.is_cancelled() {
                    if let Err(error) = ctx.complete().await {
                        error!("Failed to auto-complete sync {}: {}", sync_run_id, error);
                    }
                }
            }
            Err(error) => {
                error!("Sync {} failed: {}", sync_run_id, error);
                if !ctx.is_cancelled() {
                    if let Err(report_error) = ctx.fail(&error.to_string()).await {
                        error!("Failed to report sync failure: {}", report_error);
                    }
                }
            }
        }
    });

    Ok(Json(SyncResponse::started()))
}

async fn cancel_sync<C>(
    State(state): State<Arc<ServerState<C>>>,
    Json(request): Json<CancelRequest>,
) -> impl IntoResponse
where
    C: Connector,
{
    info!("Cancel requested for sync {}", request.sync_run_id);

    let matching_sync = state
        .active_syncs
        .iter()
        .find(|sync| sync.sync_run_id == request.sync_run_id)
        .map(|sync| Arc::clone(&sync.cancelled));

    let Some(cancelled) = matching_sync else {
        return (
            StatusCode::NOT_FOUND,
            Json(CancelResponse {
                status: "not_found".to_string(),
            }),
        );
    };

    cancelled.store(true, Ordering::SeqCst);
    let _ = state.connector.cancel(&request.sync_run_id).await;

    (
        StatusCode::OK,
        Json(CancelResponse {
            status: "cancelled".to_string(),
        }),
    )
}

async fn oauth_credential_ready<C>(
    State(state): State<Arc<ServerState<C>>>,
    Json(request): Json<OAuthCredentialReadyRequest>,
) -> Result<Response, (StatusCode, Json<serde_json::Value>)>
where
    C: Connector,
{
    let Some(adapter) = state.mcp_adapter() else {
        return Ok((StatusCode::NO_CONTENT, "").into_response());
    };

    let credentials: McpCredentials =
        serde_json::from_value(request.credentials).map_err(|error| {
            (
                StatusCode::BAD_REQUEST,
                Json(serde_json::json!({
                    "error": format!("Invalid MCP OAuth credentials: {error}")
                })),
            )
        })?;
    let (env, headers) = build_mcp_auth(&*state.connector, &credentials)
        .await
        .map_err(|error| {
            (
                StatusCode::BAD_REQUEST,
                Json(serde_json::json!({ "error": error.to_string() })),
            )
        })?;

    if let Err(error) = adapter.discover(env, headers).await {
        // Keep any previously authenticated catalog available. A transient
        // OAuth or MCP failure should not make already-discovered tools
        // disappear from the connector manifest.
        warn!("OAuth credential-ready MCP discovery failed: {error:#}");
        return Err((
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({ "error": error.to_string() })),
        ));
    }

    Ok(Json(build_manifest_with_mcp(&state).await).into_response())
}

/// Validate a freshly exchanged OAuth credential. The default implementation
/// accepts the credential without binding anything; connectors override
/// `Connector::validate_oauth_credential` to enforce source bindings.
async fn oauth_validate<C>(
    State(state): State<Arc<ServerState<C>>>,
    Json(request): Json<OAuthCredentialValidationRequest>,
) -> Result<Json<shared::models::OAuthCredentialValidationResponse>, (StatusCode, Json<serde_json::Value>)>
where
    C: Connector,
{
    match state.connector.validate_oauth_credential(&request).await {
        Ok(response) => Ok(Json(response)),
        Err(error) => Err((
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({ "error": error.to_string() })),
        )),
    }
}

async fn execute_action<C>(
    State(state): State<Arc<ServerState<C>>>,
    Json(request): Json<ActionRequest>,
) -> Result<Response, (StatusCode, Json<ActionResponse>)>
where
    C: Connector,
{
    info!("Action requested: {}", request.action);

    // Native action names win on collisions: only actions outside the
    // connector's own manifest may be dispatched to the MCP server. This
    // keeps runtime dispatch consistent with ActionDefinition.origin, which
    // marks MCP-discovered manifest actions only when no native action of
    // the same name exists.
    let is_native_action = state
        .connector
        .actions()
        .iter()
        .any(|action| action.name == request.action);

    // MCP dispatch: if the action matches a tool exposed by the connector's
    // MCP server (and is not a native action), delegate to the adapter. Falls
    // through to the connector's own `execute_action` for connector-defined
    // actions.
    if let Some(adapter) = state.mcp_adapter().filter(|_| !is_native_action) {
        let creds = request
            .credentials
            .as_ref()
            .map(McpCredentials::from_service_credential)
            .unwrap_or_default();
        let (env, headers) = match build_mcp_auth(&*state.connector, &creds).await {
            Ok(auth) => auth,
            Err(e) => {
                warn!(
                    "MCP auth preparation failed; falling back to connector: {:#}",
                    e
                );
                (None, None)
            }
        };
        match adapter
            .get_action_definitions_live(env.clone(), headers.clone())
            .await
        {
            Ok(actions) => {
                if let Some(action) = actions.iter().find(|a| a.name == request.action) {
                    let source_read_only = request
                        .source
                        .as_ref()
                        .and_then(|source| source.config.get("read_only"))
                        .and_then(|value| value.as_bool())
                        .unwrap_or(false);
                    if (state.connector.read_only() || source_read_only)
                        && action.mode == shared::models::ActionMode::Write
                    {
                        return Ok(ActionResponse::failure(format!(
                            "Action '{}' is not allowed: source is read-only",
                            request.action
                        ))
                        .into_response_with_status(StatusCode::BAD_REQUEST));
                    }
                    let response = adapter
                        .execute_tool(&request.action, request.params.clone(), env, headers)
                        .await;
                    if response.status != "success" {
                        if let Some(error) = response.error.as_deref() {
                            // Terminal OAuth rejection: surface the standard
                            // 412 challenge instead of a generic failure so
                            // connector-manager invalidates the credential.
                            if let Some(auth_response) =
                                mcp_auth_required_action_response(&state, &request, error)
                            {
                                return Ok(auth_response);
                            }
                        }
                        return Ok(response.into_response_with_status(StatusCode::BAD_REQUEST));
                    }
                    return Ok(response.into_response_with_status(StatusCode::OK));
                }
            }
            Err(e) => {
                let message = format!("{:#}", e);
                // A live-validation failure caused by a terminal OAuth
                // rejection must produce the auth challenge; never fall
                // through to the connector's native handler for a tool that
                // needs the acting user's credential.
                if let Some(auth_response) =
                    mcp_auth_required_action_response(&state, &request, &message)
                {
                    return Ok(auth_response);
                }
                // Never execute a cached MCP action after live validation fails.
                // Falling through is safe only for connector-defined actions.
                if adapter.has_cached_action(&request.action).await {
                    return Ok(ActionResponse::failure(format!(
                        "MCP action '{}' could not be validated: {}",
                        request.action, e
                    ))
                    .into_response_with_status(StatusCode::BAD_REQUEST));
                }
                warn!("MCP action lookup failed; falling back to connector: {}", e);
            }
        }
    }

    state
        .connector
        .execute_action(
            &request.action,
            request.params,
            request.credentials,
            request.source,
            request.actor_email,
        )
        .await
        .map_err(|error| {
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(ActionResponse::failure(format!("{:#}", error))),
            )
        })
}

async fn read_resource<C>(
    State(state): State<Arc<ServerState<C>>>,
    Json(request): Json<ResourceRequest>,
) -> Result<Json<serde_json::Value>, (StatusCode, Json<serde_json::Value>)>
where
    C: Connector,
{
    info!("Resource requested: {}", request.uri);
    let adapter = state.mcp_adapter().ok_or_else(|| {
        (
            StatusCode::NOT_FOUND,
            Json(serde_json::json!({ "error": "MCP not enabled for this connector" })),
        )
    })?;
    let (env, headers) = build_mcp_auth(&*state.connector, &request.credentials)
        .await
        .map_err(|e| {
            error!(
                "MCP auth preparation failed for resource {}: {:#}",
                request.uri, e
            );
            (
                StatusCode::BAD_REQUEST,
                Json(serde_json::json!({ "error": e.to_string() })),
            )
        })?;
    adapter
        .read_resource(&request.uri, env, headers)
        .await
        .map(Json)
        .map_err(|e| {
            let message = format!("{:#}", e);
            error!("Resource read failed for {}: {}", request.uri, message);
            if let Some(body) = mcp_auth_required_credentials_response(
                state.connector.as_ref(),
                &request.credentials,
                &message,
            ) {
                return (StatusCode::PRECONDITION_FAILED, Json(body));
            }
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(serde_json::json!({ "error": message })),
            )
        })
}

async fn get_prompt<C>(
    State(state): State<Arc<ServerState<C>>>,
    Json(request): Json<PromptRequest>,
) -> Result<Json<serde_json::Value>, (StatusCode, Json<serde_json::Value>)>
where
    C: Connector,
{
    info!("Prompt requested: {}", request.name);
    get_mcp_prompt_json(state, request.name, request.arguments, request.credentials)
        .await
        .map(Json)
}

async fn get_skill<C>(
    State(state): State<Arc<ServerState<C>>>,
    Json(request): Json<SkillRequest>,
) -> Result<Json<SkillResponse>, (StatusCode, Json<serde_json::Value>)>
where
    C: Connector,
{
    info!("Skill requested: {}", request.skill_id);
    for skill in state.connector.skills() {
        if skill.id != request.skill_id {
            continue;
        }
        if let Some(content) = skill.content {
            return Ok(Json(SkillResponse {
                skill_id: skill.id,
                title: skill.title,
                content,
            }));
        }
        if let Some(prompt_name) = skill.mcp_prompt {
            let value =
                get_mcp_prompt_json(state, prompt_name, request.arguments, request.credentials)
                    .await?;
            return Ok(Json(SkillResponse {
                skill_id: request.skill_id,
                title: skill.title,
                content: mcp_prompt_json_to_text(&value),
            }));
        }
    }

    let prompt_name = request
        .skill_id
        .strip_prefix("mcp:")
        .map(|name| name.to_string());
    if let Some(prompt_name) = prompt_name {
        let value =
            get_mcp_prompt_json(state, prompt_name, request.arguments, request.credentials).await?;
        return Ok(Json(SkillResponse {
            skill_id: request.skill_id,
            title: "MCP Prompt".to_string(),
            content: mcp_prompt_json_to_text(&value),
        }));
    }

    Err((
        StatusCode::NOT_FOUND,
        Json(serde_json::json!({ "error": format!("Unknown skill: {}", request.skill_id) })),
    ))
}

async fn get_mcp_prompt_json<C>(
    state: Arc<ServerState<C>>,
    name: String,
    arguments: Option<serde_json::Value>,
    credentials: McpCredentials,
) -> Result<serde_json::Value, (StatusCode, Json<serde_json::Value>)>
where
    C: Connector,
{
    let adapter = state.mcp_adapter().ok_or_else(|| {
        (
            StatusCode::NOT_FOUND,
            Json(serde_json::json!({ "error": "MCP not enabled for this connector" })),
        )
    })?;
    let (env, headers) = build_mcp_auth(&*state.connector, &credentials)
        .await
        .map_err(|e| {
            error!("MCP auth preparation failed for prompt {}: {:#}", name, e);
            (
                StatusCode::BAD_REQUEST,
                Json(serde_json::json!({ "error": e.to_string() })),
            )
        })?;
    adapter
        .get_prompt(&name, arguments, env, headers)
        .await
        .map_err(|e| {
            let message = format!("{:#}", e);
            error!("Prompt get failed for {}: {}", name, message);
            if let Some(body) =
                mcp_auth_required_credentials_response(
                    state.connector.as_ref(),
                    &credentials,
                    &message,
                )
            {
                return (StatusCode::PRECONDITION_FAILED, Json(body));
            }
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(serde_json::json!({ "error": message })),
            )
        })
}

fn mcp_prompt_json_to_text(value: &serde_json::Value) -> String {
    let mut parts = Vec::new();
    if let Some(description) = value.get("description").and_then(|v| v.as_str()) {
        if !description.is_empty() {
            parts.push(description.to_string());
        }
    }
    if let Some(messages) = value.get("messages").and_then(|v| v.as_array()) {
        for message in messages {
            if let Some(text) = message
                .get("content")
                .and_then(|v| v.get("text"))
                .and_then(|v| v.as_str())
            {
                parts.push(text.to_string());
            }
        }
    }
    parts.join("\n\n")
}

fn decode<T: DeserializeOwned>(value: &serde_json::Value, label: &str) -> Result<T> {
    serde_json::from_value(value.clone()).with_context(|| format!("Failed to decode {}", label))
}

fn decode_optional<T: DeserializeOwned>(
    value: Option<&serde_json::Value>,
    label: &str,
) -> Result<Option<T>> {
    value.map(|value| decode(value, label)).transpose()
}

fn map_source_fetch_error(error: SdkError) -> (StatusCode, Json<SyncResponse>) {
    let status = if error.is_not_found() {
        StatusCode::NOT_FOUND
    } else {
        StatusCode::INTERNAL_SERVER_ERROR
    };
    (status, Json(SyncResponse::error(error.to_string())))
}

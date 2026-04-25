use crate::connector::Connector;
use crate::context::SyncContext;
use crate::models::{
    ActionRequest, ActionResponse, CancelRequest, CancelResponse, SyncRequest, SyncResponse,
    SyncStatusResponse,
};
use anyhow::{Context, Result};
use axum::{
    extract::{Path, State},
    http::StatusCode,
    middleware,
    response::{IntoResponse, Json, Response},
    routing::{get, post},
    Router,
};
use dashmap::mapref::entry::Entry;
use dashmap::DashMap;
use serde::de::DeserializeOwned;
use shared::telemetry;
use shared::{SdkClient, SdkError};
use std::sync::atomic::AtomicBool;
use std::sync::Arc;
use tokio::time::{interval, Duration};
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
            connector_url: shared::build_connector_url(),
        })
    }
}

struct ActiveSync {
    sync_run_id: String,
    cancelled: Arc<AtomicBool>,
}

type ActiveSyncs = Arc<DashMap<String, ActiveSync>>;

/// Reserves a slot in `active_syncs` for a source. The slot is released when
/// this guard is dropped, including on panic inside the spawned sync task.
/// This is the only mechanism that removes from `active_syncs` — so the entry
/// cannot leak even if the connector's `sync()` implementation panics.
struct ActiveSyncGuard {
    active_syncs: ActiveSyncs,
    source_id: String,
}

impl ActiveSyncGuard {
    /// Atomically reserve a slot. Returns `None` if a sync is already active
    /// for this source.
    fn reserve(
        active_syncs: ActiveSyncs,
        source_id: String,
        sync_run_id: String,
        cancelled: Arc<AtomicBool>,
    ) -> Option<Self> {
        let inserted = match active_syncs.entry(source_id.clone()) {
            Entry::Occupied(_) => false,
            Entry::Vacant(vacant) => {
                vacant.insert(ActiveSync {
                    sync_run_id,
                    cancelled,
                });
                true
            }
        };
        if inserted {
            Some(Self {
                active_syncs,
                source_id,
            })
        } else {
            None
        }
    }
}

impl Drop for ActiveSyncGuard {
    fn drop(&mut self) {
        self.active_syncs.remove(&self.source_id);
    }
}

struct ServerState<C: Connector> {
    connector: Arc<C>,
    sdk_client: SdkClient,
    connector_url: String,
    active_syncs: ActiveSyncs,
}

impl<C: Connector> ServerState<C> {
    fn new(connector: Arc<C>, sdk_client: SdkClient, connector_url: String) -> Self {
        Self {
            connector,
            sdk_client,
            connector_url,
            active_syncs: Arc::new(DashMap::new()),
        }
    }
}

pub fn create_router<C>(connector: Arc<C>, sdk_client: SdkClient, connector_url: String) -> Router
where
    C: Connector,
{
    let router = Router::new()
        .route("/health", get(health::<C>))
        .route("/manifest", get(manifest::<C>))
        .route("/sync", post(trigger_sync::<C>))
        .route("/sync/:sync_run_id", get(sync_status::<C>))
        .route("/cancel", post(cancel_sync::<C>))
        .route("/action", post(execute_action::<C>));

    router
        .layer(
            ServiceBuilder::new()
                .layer(middleware::from_fn(telemetry::middleware::trace_layer))
                .layer(CorsLayer::permissive()),
        )
        .with_state(Arc::new(ServerState::new(
            connector,
            sdk_client,
            connector_url,
        )))
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
/// `/cancel`, `/action`) — collisions cause axum to panic at startup.
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

    start_registration_loop(
        Arc::clone(&connector),
        sdk_client.clone(),
        config.connector_url.clone(),
    );

    let app = create_router(connector, sdk_client, config.connector_url).merge(extra_routes);
    axum::serve(listener, app).await?;
    Ok(())
}

fn start_registration_loop<C>(
    connector: Arc<C>,
    sdk_client: SdkClient,
    connector_url: String,
) -> tokio::task::JoinHandle<()>
where
    C: Connector,
{
    tokio::spawn(async move {
        let mut ticker = interval(Duration::from_secs(30));
        let mut last_was_ok: Option<bool> = None;
        loop {
            ticker.tick().await;
            let manifest = connector.build_manifest(connector_url.clone()).await;
            match sdk_client.register(&manifest).await {
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
    Json(
        state
            .connector
            .build_manifest(state.connector_url.clone())
            .await,
    )
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

    let cancelled = Arc::new(AtomicBool::new(false));
    let Some(guard) = ActiveSyncGuard::reserve(
        Arc::clone(&state.active_syncs),
        source_id.clone(),
        sync_run_id.clone(),
        Arc::clone(&cancelled),
    ) else {
        return Err((
            StatusCode::CONFLICT,
            Json(SyncResponse::error(
                "Sync already in progress for this source",
            )),
        ));
    };

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

    let typed_state =
        decode_optional::<C::State>(source.connector_state.as_ref(), "connector state").map_err(
            |error| {
                (
                    StatusCode::BAD_REQUEST,
                    Json(SyncResponse::error(error.to_string())),
                )
            },
        )?;

    state
        .sdk_client
        .register_sync(&sync_run_id, request.sync_mode)
        .await;

    // The manager carries its running tally for this sync run in the
    // dispatch payload — zero on a fresh sync, non-zero on resume. Use
    // those values directly to seed the context rather than making a
    // separate HTTP round trip.
    let ctx = SyncContext::new(
        state.sdk_client.clone(),
        sync_run_id.clone(),
        source_id.clone(),
        source.source_type,
        request.sync_mode,
        cancelled,
        request.documents_scanned,
        request.documents_updated,
    );
    let connector = Arc::clone(&state.connector);

    tokio::spawn(async move {
        // Moved into the task so the slot is released when this future
        // completes — including on panic, which unwinds through locals.
        let _slot = guard;
        let result = connector
            .sync(source, credentials, typed_state, ctx.clone())
            .await;

        if let Err(error) = result {
            error!("Sync {} failed: {}", sync_run_id, error);
            if !ctx.is_cancelled() {
                if let Err(report_error) = ctx.fail(&error.to_string()).await {
                    error!("Failed to report sync failure: {}", report_error);
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

    for sync in state.active_syncs.iter() {
        if sync.sync_run_id == request.sync_run_id {
            sync.cancelled
                .store(true, std::sync::atomic::Ordering::SeqCst);
            let _ = state.connector.cancel(&request.sync_run_id).await;

            return Json(CancelResponse {
                status: "cancelled".to_string(),
            });
        }
    }

    Json(CancelResponse {
        status: "not_found".to_string(),
    })
}

async fn execute_action<C>(
    State(state): State<Arc<ServerState<C>>>,
    Json(request): Json<ActionRequest>,
) -> Result<Response, (StatusCode, Json<ActionResponse>)>
where
    C: Connector,
{
    info!("Action requested: {}", request.action);
    state
        .connector
        .execute_action(&request.action, request.params, request.credentials)
        .await
        .map_err(|error| {
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(ActionResponse::failure(error.to_string())),
            )
        })
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

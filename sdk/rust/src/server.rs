use crate::connector::Connector;
use crate::context::SyncContext;
use crate::models::{
    ActionRequest, ActionResponse, CancelRequest, CancelResponse, SyncRequest, SyncResponse,
};
use anyhow::{Context, Result};
use axum::{
    extract::State,
    http::StatusCode,
    middleware,
    response::{IntoResponse, Json},
    routing::{get, post},
    Router,
};
use dashmap::DashMap;
use serde::de::DeserializeOwned;
use shared::telemetry;
use shared::SdkClient;
use std::sync::atomic::AtomicBool;
use std::sync::Arc;
use tokio::time::{interval, Duration};
use tower::ServiceBuilder;
use tower_http::cors::CorsLayer;
use tracing::{error, info, warn};

#[derive(Clone)]
pub struct ServerConfig {
    pub host: String,
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
            host: "0.0.0.0".to_string(),
            port,
            connector_url: shared::build_connector_url(),
        })
    }
}

struct ActiveSync {
    sync_run_id: String,
    cancelled: Arc<AtomicBool>,
}

struct ServerState<C: Connector> {
    connector: Arc<C>,
    sdk_client: SdkClient,
    connector_url: String,
    active_syncs: DashMap<String, ActiveSync>,
}

impl<C: Connector> ServerState<C> {
    fn new(connector: Arc<C>, sdk_client: SdkClient, connector_url: String) -> Self {
        Self {
            connector,
            sdk_client,
            connector_url,
            active_syncs: DashMap::new(),
        }
    }
}

pub fn create_router<C>(connector: Arc<C>, sdk_client: SdkClient, connector_url: String) -> Router
where
    C: Connector,
{
    Router::new()
        .route("/health", get(health::<C>))
        .route("/manifest", get(manifest::<C>))
        .route("/sync", post(trigger_sync::<C>))
        .route("/cancel", post(cancel_sync::<C>))
        .route("/action", post(execute_action::<C>))
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
    let connector = Arc::new(connector);
    let sdk_client = SdkClient::from_env()?;
    start_registration_loop(
        Arc::clone(&connector),
        sdk_client.clone(),
        config.connector_url.clone(),
    );

    let app = create_router(connector, sdk_client, config.connector_url);
    let addr = std::net::SocketAddr::from(([0, 0, 0, 0], config.port));
    let listener = tokio::net::TcpListener::bind(addr).await?;

    info!("HTTP server listening on {}", addr);
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
        loop {
            ticker.tick().await;
            let manifest = connector.build_manifest(connector_url.clone()).await;
            match sdk_client.register(&manifest).await {
                Ok(()) => info!("Registered with connector manager"),
                Err(error) => warn!("Registration failed: {}", error),
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

    if state.active_syncs.contains_key(&source_id) {
        return Err((
            StatusCode::CONFLICT,
            Json(SyncResponse::error(
                "Sync already in progress for this source",
            )),
        ));
    }

    let source = state
        .sdk_client
        .get_source(&source_id)
        .await
        .map_err(map_source_fetch_error)?;
    let source_config = decode::<C::Config>(&source.config, "source config").map_err(|error| {
        (
            StatusCode::BAD_REQUEST,
            Json(SyncResponse::error(error.to_string())),
        )
    })?;

    let raw_credentials = if state.connector.requires_credentials() {
        state
            .sdk_client
            .get_credentials(&source_id)
            .await
            .map_err(map_source_fetch_error)?
            .credentials
    } else {
        serde_json::json!({})
    };

    let typed_credentials =
        decode::<C::Credentials>(&raw_credentials, "credentials").map_err(|error| {
            (
                StatusCode::BAD_REQUEST,
                Json(SyncResponse::error(error.to_string())),
            )
        })?;
    let typed_state =
        decode_optional::<C::State>(source.connector_state.as_ref(), "connector state").map_err(
            |error| {
                (
                    StatusCode::BAD_REQUEST,
                    Json(SyncResponse::error(error.to_string())),
                )
            },
        )?;

    let cancelled = Arc::new(AtomicBool::new(false));
    state.active_syncs.insert(
        source_id.clone(),
        ActiveSync {
            sync_run_id: sync_run_id.clone(),
            cancelled: Arc::clone(&cancelled),
        },
    );

    let ctx = SyncContext::new(
        state.sdk_client.clone(),
        sync_run_id.clone(),
        source_id.clone(),
        source.source_type,
        cancelled,
    );
    let connector = Arc::clone(&state.connector);
    let state_for_task = Arc::clone(&state);

    tokio::spawn(async move {
        let result = connector
            .sync(source_config, typed_credentials, typed_state, ctx.clone())
            .await;

        if let Err(error) = result {
            error!("Sync {} failed: {}", sync_run_id, error);
            if !ctx.is_cancelled() {
                if let Err(report_error) = ctx.fail(&error.to_string()).await {
                    error!("Failed to report sync failure: {}", report_error);
                }
            }
        }

        state_for_task.active_syncs.remove(&source_id);
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
            let _ = state.connector.cancel(&request.sync_run_id);

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
) -> impl IntoResponse
where
    C: Connector,
{
    info!("Action requested: {}", request.action);

    match state
        .connector
        .execute_action(&request.action, request.params, request.credentials)
        .await
    {
        Ok(response) => Json(response),
        Err(error) => Json(ActionResponse::failure(error.to_string())),
    }
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

fn map_source_fetch_error(error: anyhow::Error) -> (StatusCode, Json<SyncResponse>) {
    let message = error.to_string();
    if message.contains("404") {
        (StatusCode::NOT_FOUND, Json(SyncResponse::error(message)))
    } else {
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(SyncResponse::error(message)),
        )
    }
}

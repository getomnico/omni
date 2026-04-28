//! Test harness for the connector SDK server.
//!
//! `MockConnectorManager` is a hand-rolled axum server that responds to the
//! `SdkClient` endpoints the SDK calls during sync dispatch. Each request is
//! recorded so tests can assert on what the connector asked for, and
//! responses are configurable per-test to simulate 404/500 upstream errors.
//!
//! `TestConnector` is a minimal `Connector` impl whose `sync()` body is
//! driven by a channel — tests can hold a sync in flight, signal it, or
//! have it panic, to drive edge cases in the SDK server.

// This module is shared across multiple integration test files; not all
// tests use every helper.
#![allow(dead_code)]

use std::net::SocketAddr;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use anyhow::Result;
use async_trait::async_trait;
use axum::{
    extract::{Path, State},
    http::StatusCode,
    response::{IntoResponse, Json},
    routing::{get, post, put},
    Router,
};
use omni_connector_sdk::{
    models::ActionResponse, Connector, ServiceCredential, Source, SourceType, SyncContext,
};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value as JsonValue};
use shared::SdkClient;
use tokio::net::TcpListener;
use tokio::sync::Notify;

// ---------------------------------------------------------------------------
// MockConnectorManager
// ---------------------------------------------------------------------------

#[derive(Debug, Default, Clone)]
pub struct MockState {
    pub get_source_response: Option<GetSourceBehavior>,
    pub register_calls: u32,
    pub fail_calls: Vec<(String, String)>,
    pub complete_calls: Vec<String>,
}

#[derive(Debug, Clone)]
pub enum GetSourceBehavior {
    Ok {
        config: JsonValue,
        connector_state: Option<JsonValue>,
    },
    NotFound,
    ServerError,
    BadConfig,
}

pub type SharedState = Arc<Mutex<MockState>>;

pub struct MockConnectorManager {
    pub url: String,
    pub state: SharedState,
    _handle: tokio::task::JoinHandle<()>,
}

impl MockConnectorManager {
    pub async fn spawn() -> Self {
        let state: SharedState = Arc::new(Mutex::new(MockState::default()));
        let app = Router::new()
            .route("/sdk/source/:id", get(handle_get_source))
            .route("/sdk/credentials/:id", get(handle_get_credentials))
            .route("/sdk/sync/:id/fail", post(handle_fail))
            .route("/sdk/sync/:id/complete", post(handle_complete))
            .route("/sdk/sync/:id/scanned", post(handle_noop_post))
            .route("/sdk/sync/:id/heartbeat", post(handle_noop_post))
            .route("/sdk/events", post(handle_noop_post))
            .route("/sdk/register", post(handle_register))
            .route("/sdk/source/:id/connector-state", put(handle_noop_post))
            .with_state(Arc::clone(&state));

        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr: SocketAddr = listener.local_addr().unwrap();
        let handle = tokio::spawn(async move {
            axum::serve(listener, app).await.ok();
        });

        Self {
            url: format!("http://{}", addr),
            state,
            _handle: handle,
        }
    }

    pub fn sdk_client(&self) -> SdkClient {
        SdkClient::new(&self.url)
    }

    pub fn set_source(&self, config: JsonValue) {
        self.set_source_behavior(GetSourceBehavior::Ok {
            config,
            connector_state: None,
        });
    }

    pub fn set_source_behavior(&self, behavior: GetSourceBehavior) {
        self.state.lock().unwrap().get_source_response = Some(behavior);
    }
}

async fn handle_get_source(
    State(state): State<SharedState>,
    Path(id): Path<String>,
) -> impl IntoResponse {
    let behavior = state
        .lock()
        .unwrap()
        .get_source_response
        .clone()
        .unwrap_or(GetSourceBehavior::NotFound);
    match behavior {
        GetSourceBehavior::Ok {
            config,
            connector_state,
        } => Json(test_source_json(&id, config, connector_state)).into_response(),
        GetSourceBehavior::NotFound => (StatusCode::NOT_FOUND, "not found").into_response(),
        GetSourceBehavior::ServerError => {
            (StatusCode::INTERNAL_SERVER_ERROR, "boom").into_response()
        }
        GetSourceBehavior::BadConfig => Json(test_source_json(
            &id,
            // Config is a string where we expect an object → decode into
            // TestConfig will fail with a deserialization error.
            JsonValue::String("not-an-object".into()),
            None,
        ))
        .into_response(),
    }
}

fn test_source_json(id: &str, config: JsonValue, connector_state: Option<JsonValue>) -> JsonValue {
    json!({
        "id": id,
        "name": "test-source",
        "source_type": "web",
        "config": config,
        "is_active": true,
        "is_deleted": false,
        "user_filter_mode": "all",
        "user_whitelist": null,
        "user_blacklist": null,
        "connector_state": connector_state,
        "sync_interval_seconds": null,
        "created_at": "2024-01-01T00:00:00Z",
        "updated_at": "2024-01-01T00:00:00Z",
        "created_by": "test-user",
    })
}

async fn handle_get_credentials(Path(_id): Path<String>) -> impl IntoResponse {
    // Not exercised — `TestConnector::requires_credentials` is false.
    (StatusCode::NOT_FOUND, "not used").into_response()
}

#[derive(Debug, Deserialize)]
struct FailBody {
    error: String,
}

async fn handle_fail(
    State(state): State<SharedState>,
    Path(sync_run_id): Path<String>,
    Json(body): Json<FailBody>,
) -> impl IntoResponse {
    state
        .lock()
        .unwrap()
        .fail_calls
        .push((sync_run_id, body.error));
    StatusCode::NO_CONTENT
}

async fn handle_complete(
    State(state): State<SharedState>,
    Path(sync_run_id): Path<String>,
    Json(_body): Json<JsonValue>,
) -> impl IntoResponse {
    state.lock().unwrap().complete_calls.push(sync_run_id);
    StatusCode::NO_CONTENT
}

async fn handle_noop_post() -> impl IntoResponse {
    StatusCode::NO_CONTENT
}

async fn handle_register(State(state): State<SharedState>) -> impl IntoResponse {
    state.lock().unwrap().register_calls += 1;
    StatusCode::NO_CONTENT
}

// ---------------------------------------------------------------------------
// TestConnector
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct TestConfig {
    #[serde(default)]
    pub label: String,
}

#[derive(Debug, Clone)]
pub enum SyncBehavior {
    /// Succeed immediately.
    Ok,
    /// Panic immediately, to exercise panic-safe active_syncs cleanup.
    Panic,
    /// Block until the given Notify is signalled, then exit.
    BlockUntil(Arc<Notify>),
}

pub struct TestConnector {
    pub behavior: Arc<Mutex<SyncBehavior>>,
    pub sync_called: Arc<Mutex<u32>>,
}

impl TestConnector {
    pub fn new(behavior: SyncBehavior) -> Self {
        Self {
            behavior: Arc::new(Mutex::new(behavior)),
            sync_called: Arc::new(Mutex::new(0)),
        }
    }

    pub fn sync_call_count(&self) -> u32 {
        *self.sync_called.lock().unwrap()
    }

    pub fn set_behavior(&self, behavior: SyncBehavior) {
        *self.behavior.lock().unwrap() = behavior;
    }
}

#[async_trait]
impl Connector for TestConnector {
    type Config = TestConfig;
    type Credentials = JsonValue;
    type State = JsonValue;

    fn name(&self) -> &'static str {
        "test"
    }

    fn version(&self) -> &'static str {
        "0.0.0"
    }

    fn source_types(&self) -> Vec<SourceType> {
        vec![SourceType::Web]
    }

    fn requires_credentials(&self) -> bool {
        false
    }

    async fn sync(
        &self,
        _source: Source,
        _credentials: Option<ServiceCredential>,
        _state: Option<Self::State>,
        ctx: SyncContext,
    ) -> Result<()> {
        *self.sync_called.lock().unwrap() += 1;
        let behavior = self.behavior.lock().unwrap().clone();
        match behavior {
            SyncBehavior::Ok => Ok(()),
            SyncBehavior::Panic => panic!("intentional test panic"),
            SyncBehavior::BlockUntil(notify) => {
                // Poll cancellation while waiting so /cancel can unblock us.
                loop {
                    if ctx.is_cancelled() {
                        return Ok(());
                    }
                    tokio::select! {
                        _ = notify.notified() => return Ok(()),
                        _ = tokio::time::sleep(Duration::from_millis(25)) => {}
                    }
                }
            }
        }
    }

    async fn execute_action(
        &self,
        action: &str,
        _params: JsonValue,
        _credentials: Option<ServiceCredential>,
    ) -> Result<axum::response::Response> {
        Ok(ActionResponse::not_supported(action).into_response_with_status(StatusCode::NOT_FOUND))
    }
}

use axum::{
    extract::{Path, State},
    http::StatusCode,
    response::Json,
    routing::{get, post},
    Router,
};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value as JsonValue};
use std::collections::HashSet;
use std::sync::{Arc, Mutex};
use tokio::net::TcpListener;
use tokio::sync::oneshot;
use tokio::time::{sleep, Duration};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RecordedSyncRequest {
    pub sync_run_id: String,
    pub source_id: String,
    pub sync_mode: String,
    #[serde(default)]
    pub last_sync_at: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RecordedCancelRequest {
    pub sync_run_id: String,
}

#[derive(Clone)]
struct MockState {
    sync_requests: Arc<Mutex<Vec<RecordedSyncRequest>>>,
    cancel_requests: Arc<Mutex<Vec<RecordedCancelRequest>>>,
    sync_response_status: Arc<Mutex<StatusCode>>,
    sync_response_body: Arc<Mutex<JsonValue>>,
    active_syncs: Arc<Mutex<HashSet<String>>>,
    /// When false, `GET /sync/{id}` returns 404 — lets a test exercise the
    /// "connector hasn't implemented the status endpoint" path (current
    /// Rust connectors).
    status_endpoint_enabled: Arc<Mutex<bool>>,
}

pub struct MockConnector {
    pub base_url: String,
    port: u16,
    pub sync_requests: Arc<Mutex<Vec<RecordedSyncRequest>>>,
    pub cancel_requests: Arc<Mutex<Vec<RecordedCancelRequest>>>,
    sync_response_status: Arc<Mutex<StatusCode>>,
    sync_response_body: Arc<Mutex<JsonValue>>,
    active_syncs: Arc<Mutex<HashSet<String>>>,
    status_endpoint_enabled: Arc<Mutex<bool>>,
    server_handle: Mutex<tokio::task::JoinHandle<()>>,
    shutdown_tx: Mutex<Option<oneshot::Sender<()>>>,
}

impl MockConnector {
    pub async fn start() -> anyhow::Result<Self> {
        let sync_requests: Arc<Mutex<Vec<RecordedSyncRequest>>> = Arc::new(Mutex::new(Vec::new()));
        let cancel_requests: Arc<Mutex<Vec<RecordedCancelRequest>>> =
            Arc::new(Mutex::new(Vec::new()));
        let sync_response_status = Arc::new(Mutex::new(StatusCode::OK));
        let sync_response_body = Arc::new(Mutex::new(json!({"status": "accepted"})));
        let active_syncs = Arc::new(Mutex::new(HashSet::new()));
        let status_endpoint_enabled = Arc::new(Mutex::new(true));

        let listener = TcpListener::bind("127.0.0.1:0").await?;
        let port = listener.local_addr()?.port();

        let (shutdown_tx, shutdown_rx) = oneshot::channel();
        let server_handle = spawn_server(
            listener,
            shutdown_rx,
            sync_requests.clone(),
            cancel_requests.clone(),
            sync_response_status.clone(),
            sync_response_body.clone(),
            active_syncs.clone(),
            status_endpoint_enabled.clone(),
        );

        sleep(Duration::from_millis(50)).await;

        Ok(Self {
            base_url: format!("http://127.0.0.1:{}", port),
            port,
            sync_requests,
            cancel_requests,
            sync_response_status,
            sync_response_body,
            active_syncs,
            status_endpoint_enabled,
            server_handle: Mutex::new(server_handle),
            shutdown_tx: Mutex::new(Some(shutdown_tx)),
        })
    }

    /// Kill the HTTP server without restarting. Simulates a connector that
    /// is down but whose registration is still live in Redis (i.e. within
    /// the 90s TTL window). Probes will fail with connection-refused.
    ///
    /// Uses graceful_shutdown (not `abort()`) so axum actually closes
    /// pooled keep-alive connections — otherwise reqwest's idle conn on the
    /// caller side would happily roundtrip to still-alive handler tasks.
    pub async fn stop(&self) {
        let sent = if let Some(tx) = self.shutdown_tx.lock().unwrap().take() {
            tx.send(()).is_ok()
        } else {
            false
        };
        eprintln!("DEBUG stop: shutdown signal sent={}", sent);
        let handle = {
            let mut h = self.server_handle.lock().unwrap();
            std::mem::replace(&mut *h, tokio::spawn(async {}))
        };
        let res = tokio::time::timeout(Duration::from_secs(2), handle).await;
        eprintln!("DEBUG stop: handle.await timed_out={}", res.is_err());
    }

    /// Kill and restart the HTTP server on the same port. In-memory state
    /// (`active_syncs`) is cleared — matching what a real connector process
    /// loses on restart. Recorded history (`sync_requests`, `cancel_requests`)
    /// is preserved so tests can still inspect it.
    pub async fn restart(&self) -> anyhow::Result<()> {
        self.stop().await;
        self.active_syncs.lock().unwrap().clear();

        // Give the kernel a moment to release the port.
        let listener = loop {
            match TcpListener::bind(format!("127.0.0.1:{}", self.port)).await {
                Ok(l) => break l,
                Err(_) => sleep(Duration::from_millis(20)).await,
            }
        };

        let (shutdown_tx, shutdown_rx) = oneshot::channel();
        let new_handle = spawn_server(
            listener,
            shutdown_rx,
            self.sync_requests.clone(),
            self.cancel_requests.clone(),
            self.sync_response_status.clone(),
            self.sync_response_body.clone(),
            self.active_syncs.clone(),
            self.status_endpoint_enabled.clone(),
        );
        *self.server_handle.lock().unwrap() = new_handle;
        *self.shutdown_tx.lock().unwrap() = Some(shutdown_tx);
        sleep(Duration::from_millis(50)).await;
        Ok(())
    }

    pub fn set_sync_response(&self, status: StatusCode, body: JsonValue) {
        *self.sync_response_status.lock().unwrap() = status;
        *self.sync_response_body.lock().unwrap() = body;
    }

    pub fn set_status_endpoint_enabled(&self, enabled: bool) {
        *self.status_endpoint_enabled.lock().unwrap() = enabled;
    }

    pub fn get_sync_requests(&self) -> Vec<RecordedSyncRequest> {
        self.sync_requests.lock().unwrap().clone()
    }

    pub fn get_cancel_requests(&self) -> Vec<RecordedCancelRequest> {
        self.cancel_requests.lock().unwrap().clone()
    }
}

#[allow(clippy::too_many_arguments)]
fn spawn_server(
    listener: TcpListener,
    shutdown_rx: oneshot::Receiver<()>,
    sync_requests: Arc<Mutex<Vec<RecordedSyncRequest>>>,
    cancel_requests: Arc<Mutex<Vec<RecordedCancelRequest>>>,
    sync_response_status: Arc<Mutex<StatusCode>>,
    sync_response_body: Arc<Mutex<JsonValue>>,
    active_syncs: Arc<Mutex<HashSet<String>>>,
    status_endpoint_enabled: Arc<Mutex<bool>>,
) -> tokio::task::JoinHandle<()> {
    let state = MockState {
        sync_requests,
        cancel_requests,
        sync_response_status,
        sync_response_body,
        active_syncs,
        status_endpoint_enabled,
    };

    let app = Router::new()
        .route("/health", get(health))
        .route(
            "/manifest",
            get(|| async {
                Json(json!({
                    "name": "mock-connector",
                    "version": "1.0.0",
                    "sync_modes": ["full", "incremental"],
                    "actions": []
                }))
            }),
        )
        .route("/sync", post(handle_sync))
        .route("/sync/:sync_run_id", get(handle_sync_status))
        .route("/cancel", post(handle_cancel))
        .with_state(state);

    tokio::spawn(async move {
        let _ = axum::serve(listener, app)
            .with_graceful_shutdown(async move {
                let _ = shutdown_rx.await;
            })
            .await;
    })
}

async fn health() -> StatusCode {
    StatusCode::OK
}

async fn handle_sync(
    State(state): State<MockState>,
    Json(request): Json<RecordedSyncRequest>,
) -> (StatusCode, Json<JsonValue>) {
    let source_id = request.source_id.clone();
    {
        let mut active = state.active_syncs.lock().unwrap();
        if active.contains(&source_id) {
            return (
                StatusCode::CONFLICT,
                Json(json!({"error": "sync already running for this source"})),
            );
        }
        active.insert(source_id);
    }
    state.sync_requests.lock().unwrap().push(request);
    let status = *state.sync_response_status.lock().unwrap();
    let body = state.sync_response_body.lock().unwrap().clone();
    (status, Json(body))
}

async fn handle_sync_status(
    State(state): State<MockState>,
    Path(sync_run_id): Path<String>,
) -> (StatusCode, Json<JsonValue>) {
    if !*state.status_endpoint_enabled.lock().unwrap() {
        return (StatusCode::NOT_FOUND, Json(json!({"error": "not found"})));
    }
    let source_id = state
        .sync_requests
        .lock()
        .unwrap()
        .iter()
        .find(|r| r.sync_run_id == sync_run_id)
        .map(|r| r.source_id.clone());
    let running = source_id
        .map(|s| state.active_syncs.lock().unwrap().contains(&s))
        .unwrap_or(false);
    (StatusCode::OK, Json(json!({"running": running})))
}

async fn handle_cancel(
    State(state): State<MockState>,
    Json(request): Json<RecordedCancelRequest>,
) -> StatusCode {
    let source_id = state
        .sync_requests
        .lock()
        .unwrap()
        .iter()
        .find(|r| r.sync_run_id == request.sync_run_id)
        .map(|r| r.source_id.clone());
    if let Some(source_id) = source_id {
        state.active_syncs.lock().unwrap().remove(&source_id);
    }
    state.cancel_requests.lock().unwrap().push(request);
    StatusCode::OK
}

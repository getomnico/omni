use anyhow::Result;
use reqwest::{Client, Response, StatusCode};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::Mutex;
use tracing::{debug, info, warn};

use crate::models::{ConnectorEvent, ConnectorManifest, ServiceCredentials, Source, SyncType};

/// Errors produced by [`SdkClient`]. Callers that use `anyhow::Result` can
/// still bubble these up via `?` because `anyhow::Error: From<E>` for any
/// `E: std::error::Error + Send + Sync + 'static`.
#[derive(Debug, thiserror::Error)]
pub enum SdkError {
    #[error("{operation}: HTTP {status}: {body}")]
    Http {
        operation: &'static str,
        status: StatusCode,
        body: String,
    },
    #[error(transparent)]
    Transport(#[from] reqwest::Error),
    #[error(transparent)]
    Other(#[from] anyhow::Error),
}

impl SdkError {
    pub fn is_not_found(&self) -> bool {
        matches!(self, SdkError::Http { status, .. } if *status == StatusCode::NOT_FOUND)
    }

    pub fn status(&self) -> Option<StatusCode> {
        match self {
            SdkError::Http { status, .. } => Some(*status),
            _ => None,
        }
    }
}

pub type SdkResult<T> = Result<T, SdkError>;

/// Return the response if the status is 2xx, otherwise capture the body and
/// return a typed `SdkError::Http`.
async fn ensure_ok(response: Response, operation: &'static str) -> SdkResult<Response> {
    if response.status().is_success() {
        return Ok(response);
    }
    let status = response.status();
    let body = response.text().await.unwrap_or_default();
    Err(SdkError::Http {
        operation,
        status,
        body,
    })
}

type BufferKey = (String, String); // (sync_run_id, source_id)

struct BufferEntry {
    events: Vec<ConnectorEvent>,
    oldest_at: Instant,
}

/// Per-`SyncType` buffer thresholds: (size, time). `None` time means flush-on-emit.
fn thresholds_for(sync_type: SyncType) -> (usize, Option<Duration>) {
    match sync_type {
        SyncType::Full => (500, Some(Duration::from_secs(10))),
        SyncType::Incremental => (100, Some(Duration::from_secs(1))),
        SyncType::Realtime => (1, None),
    }
}

/// HTTP client for communicating with connector-manager SDK endpoints.
/// This is the standard way for connectors to interact with the connector-manager
/// for emitting events, storing content, and reporting sync status.
///
/// `emit_event()` buffers events in memory and auto-flushes using per-`SyncType`
/// rules (see [`thresholds_for`]). All clones share the buffer.
///
/// The SDK learns each sync's type from `create_sync_run` (auto-registered) or
/// from an explicit `register_sync` call (used by connectors whose sync was
/// created by connector-manager, e.g. scheduled or webhook-triggered syncs).
/// Unknown sync_run_ids default to `Incremental` — safe middle ground.
///
/// **Invariant**: any operation that persists a checkpoint or terminates a sync
/// (`save_connector_state`, `complete`, `fail`) must flush the relevant buffered
/// events first — otherwise a crash after checkpoint would lose those events forever.
#[derive(Clone)]
pub struct SdkClient {
    client: Client,
    base_url: String,
    event_buffer: Arc<Mutex<HashMap<BufferKey, BufferEntry>>>,
    sync_types: Arc<Mutex<HashMap<String, SyncType>>>,
}

#[derive(Debug, Serialize)]
struct EmitBatchRequest {
    sync_run_id: String,
    source_id: String,
    events: Vec<ConnectorEvent>,
}

#[derive(Debug, Serialize)]
struct StoreContentRequest {
    sync_run_id: String,
    content: String,
    content_type: Option<String>,
}

#[derive(Debug, Deserialize)]
struct StoreContentResponse {
    content_id: String,
}

#[derive(Debug, Serialize)]
struct CompleteRequest {
    documents_scanned: Option<i32>,
    documents_updated: Option<i32>,
    new_state: Option<serde_json::Value>,
}

#[derive(Debug, Deserialize)]
struct SyncConfigResponse {
    connector_state: Option<serde_json::Value>,
}

#[derive(Debug, Serialize)]
struct FailRequest {
    error: String,
}

#[derive(Debug, Serialize)]
struct CreateSyncRequest {
    source_id: String,
    sync_type: SyncType,
}

#[derive(Debug, Deserialize)]
struct CreateSyncResponse {
    sync_run_id: String,
}

#[derive(Debug, Serialize)]
struct CancelSyncRequest {
    sync_run_id: String,
}

#[derive(Debug, Deserialize)]
struct UserEmailResponse {
    email: String,
}

#[derive(Debug, Serialize)]
struct WebhookNotificationRequest {
    source_id: String,
    event_type: String,
}

#[derive(Debug, Deserialize)]
struct WebhookNotificationResponse {
    sync_run_id: String,
}

#[derive(Debug, Deserialize)]
struct ExtractTextResponse {
    text: String,
}

impl SdkClient {
    pub fn new(connector_manager_url: &str) -> Self {
        Self {
            client: Client::new(),
            base_url: connector_manager_url.trim_end_matches('/').to_string(),
            event_buffer: Arc::new(Mutex::new(HashMap::new())),
            sync_types: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    /// Register a sync's type so subsequent `emit_event` calls apply the right
    /// batching rule. Call this from your sync handler before emitting — or,
    /// if you created the sync via `create_sync_run`, the registration happens
    /// automatically.
    pub async fn register_sync(&self, sync_run_id: &str, sync_type: SyncType) {
        self.sync_types
            .lock()
            .await
            .insert(sync_run_id.to_string(), sync_type);
    }

    async fn sync_type_for(&self, sync_run_id: &str) -> SyncType {
        self.sync_types
            .lock()
            .await
            .get(sync_run_id)
            .copied()
            .unwrap_or(SyncType::Incremental)
    }

    pub fn from_env() -> Result<Self> {
        let url = std::env::var("CONNECTOR_MANAGER_URL")
            .map_err(|_| anyhow::anyhow!("CONNECTOR_MANAGER_URL not set"))?;
        Ok(Self::new(&url))
    }

    /// Build a multipart form for binary extraction endpoints.
    fn build_extract_form(
        sync_run_id: &str,
        data: Vec<u8>,
        mime_type: &str,
        filename: Option<&str>,
    ) -> reqwest::multipart::Form {
        let form = reqwest::multipart::Form::new()
            .text("sync_run_id", sync_run_id.to_string())
            .text("mime_type", mime_type.to_string())
            .part(
                "data",
                reqwest::multipart::Part::bytes(data)
                    .file_name("file")
                    .mime_str("application/octet-stream")
                    .expect("valid mime string"),
            );

        if let Some(name) = filename {
            form.text("filename", name.to_string())
        } else {
            form
        }
    }

    /// Extract text from binary file content via the connector manager.
    ///
    /// Sends the raw bytes to the connector manager which performs extraction
    /// using Docling (when enabled) or the built-in extractor. Returns the
    /// extracted text without storing it — useful when the caller needs to
    /// post-process or combine the text before storing.
    pub async fn extract_text(
        &self,
        sync_run_id: &str,
        data: Vec<u8>,
        mime_type: &str,
        filename: Option<&str>,
    ) -> SdkResult<String> {
        debug!(
            "SDK: Extracting text for sync_run={}, mime={}, size={}",
            sync_run_id,
            mime_type,
            data.len()
        );

        let form = Self::build_extract_form(sync_run_id, data, mime_type, filename);

        let response = self
            .client
            .post(format!("{}/sdk/extract-text", self.base_url))
            .multipart(form)
            .send()
            .await?;
        let response = ensure_ok(response, "extract_text").await?;
        let result: ExtractTextResponse = response.json().await?;
        Ok(result.text)
    }

    /// Emit a document event. Events are buffered in memory and auto-flushed
    /// according to the sync's type (see [`thresholds_for`]):
    /// - Full: 500 events or 10s, whichever first
    /// - Incremental: 100 events or 1s
    /// - Realtime: flush on every emit
    ///
    /// If an auto-flush fails, the error is returned to the caller so the connector
    /// knows the event was not persisted before checkpointing.
    pub async fn emit_event(
        &self,
        sync_run_id: &str,
        source_id: &str,
        event: ConnectorEvent,
    ) -> SdkResult<()> {
        let sync_type = self.sync_type_for(sync_run_id).await;
        let (size_threshold, time_threshold) = thresholds_for(sync_type);

        let key = (sync_run_id.to_string(), source_id.to_string());
        let should_flush = {
            let mut buffer = self.event_buffer.lock().await;
            let entry = buffer.entry(key).or_insert_with(|| BufferEntry {
                events: Vec::new(),
                oldest_at: Instant::now(),
            });
            entry.events.push(event);

            let size_hit = entry.events.len() >= size_threshold;
            let time_hit = time_threshold
                .map(|t| entry.oldest_at.elapsed() >= t)
                .unwrap_or(false);
            size_hit || time_hit
        };

        if should_flush {
            self.flush_events(sync_run_id, source_id).await?;
        }

        Ok(())
    }

    /// Flush buffered events for a specific (sync_run_id, source_id) pair.
    pub async fn flush_events(&self, sync_run_id: &str, source_id: &str) -> Result<()> {
        let key = (sync_run_id.to_string(), source_id.to_string());
        let events = {
            let mut buffer = self.event_buffer.lock().await;
            buffer
                .remove(&key)
                .map(|entry| entry.events)
                .unwrap_or_default()
        };

        if events.is_empty() {
            return Ok(());
        }

        let batch_size = events.len();
        debug!(
            "SDK: Flushing {} events for sync_run={}, source={}",
            batch_size, sync_run_id, source_id
        );

        let request = EmitBatchRequest {
            sync_run_id: sync_run_id.to_string(),
            source_id: source_id.to_string(),
            events,
        };

        let response = self
            .client
            .post(format!("{}/sdk/events/batch", self.base_url))
            .json(&request)
            .send()
            .await?;
        ensure_ok(response, "flush_events").await?;
        Ok(())
    }

    /// Flush all buffered events for a given source_id across any sync_runs.
    /// Used before persisting connector state for that source.
    pub async fn flush_source(&self, source_id: &str) -> Result<()> {
        let keys: Vec<BufferKey> = {
            let buffer = self.event_buffer.lock().await;
            buffer
                .keys()
                .filter(|(_, sid)| sid == source_id)
                .cloned()
                .collect()
        };

        for (sync_run_id, sid) in keys {
            self.flush_events(&sync_run_id, &sid).await?;
        }
        Ok(())
    }

    /// Flush all buffered events across all (sync_run_id, source_id) pairs.
    pub async fn flush_all(&self) -> Result<()> {
        let keys: Vec<BufferKey> = {
            let buffer = self.event_buffer.lock().await;
            buffer.keys().cloned().collect()
        };

        for (sync_run_id, source_id) in keys {
            self.flush_events(&sync_run_id, &source_id).await?;
        }
        Ok(())
    }

    /// Extract text from binary file content and store it, returning content_id.
    ///
    /// The connector manager extracts text based on the MIME type (PDF, DOCX,
    /// XLSX, PPTX, HTML, etc.) and stores the result. When the MIME type is
    /// `application/octet-stream`, the optional filename is used to infer
    /// the actual format.
    pub async fn extract_and_store_content(
        &self,
        sync_run_id: &str,
        data: Vec<u8>,
        mime_type: &str,
        filename: Option<&str>,
    ) -> SdkResult<String> {
        debug!(
            "SDK: Extracting content for sync_run={}, mime={}, size={}",
            sync_run_id,
            mime_type,
            data.len()
        );

        let form = Self::build_extract_form(sync_run_id, data, mime_type, filename);

        let response = self
            .client
            .post(format!("{}/sdk/extract-content", self.base_url))
            .multipart(form)
            .send()
            .await?;
        let response = ensure_ok(response, "extract_and_store_content").await?;
        let result: StoreContentResponse = response.json().await?;
        Ok(result.content_id)
    }

    /// Store content and return content_id
    pub async fn store_content(&self, sync_run_id: &str, content: &str) -> SdkResult<String> {
        debug!("SDK: Storing content for sync_run={}", sync_run_id);

        let request = StoreContentRequest {
            sync_run_id: sync_run_id.to_string(),
            content: content.to_string(),
            content_type: Some("text/plain".to_string()),
        };

        let response = self
            .client
            .post(format!("{}/sdk/content", self.base_url))
            .json(&request)
            .send()
            .await?;
        let response = ensure_ok(response, "store_content").await?;
        let result: StoreContentResponse = response.json().await?;
        Ok(result.content_id)
    }

    /// Send heartbeat to update last_activity_at
    pub async fn heartbeat(&self, sync_run_id: &str) -> SdkResult<()> {
        debug!("SDK: Heartbeat for sync_run={}", sync_run_id);

        let response = self
            .client
            .post(format!(
                "{}/sdk/sync/{}/heartbeat",
                self.base_url, sync_run_id
            ))
            .send()
            .await?;
        ensure_ok(response, "heartbeat").await?;
        Ok(())
    }

    /// Increment scanned count and update heartbeat
    pub async fn increment_scanned(&self, sync_run_id: &str, count: i32) -> SdkResult<()> {
        debug!(
            "SDK: Incrementing scanned for sync_run={} by {}",
            sync_run_id, count
        );

        let response = self
            .client
            .post(format!(
                "{}/sdk/sync/{}/scanned",
                self.base_url, sync_run_id
            ))
            .json(&serde_json::json!({ "count": count }))
            .send()
            .await?;
        ensure_ok(response, "increment_scanned").await?;
        Ok(())
    }

    /// Mark sync as completed. Flushes any buffered events first so the
    /// completion never races ahead of the final events for this sync.
    pub async fn complete(
        &self,
        sync_run_id: &str,
        documents_scanned: i32,
        documents_updated: i32,
        new_state: Option<serde_json::Value>,
    ) -> SdkResult<()> {
        debug!("SDK: Completing sync_run={}", sync_run_id);

        self.flush_all().await?;

        let request = CompleteRequest {
            documents_scanned: Some(documents_scanned),
            documents_updated: Some(documents_updated),
            new_state,
        };

        let response = self
            .client
            .post(format!(
                "{}/sdk/sync/{}/complete",
                self.base_url, sync_run_id
            ))
            .json(&request)
            .send()
            .await?;
        ensure_ok(response, "complete").await?;
        Ok(())
    }

    /// Mark sync as failed. Best-effort flush of buffered events first — if
    /// the flush itself fails we log and proceed, because marking the sync as
    /// failed is more important than preserving partial progress.
    pub async fn fail(&self, sync_run_id: &str, error: &str) -> SdkResult<()> {
        debug!("SDK: Failing sync_run={}: {}", sync_run_id, error);

        if let Err(e) = self.flush_all().await {
            warn!(
                "SDK: flush before fail() failed (continuing): sync_run={}: {}",
                sync_run_id, e
            );
        }

        let request = FailRequest {
            error: error.to_string(),
        };

        let response = self
            .client
            .post(format!("{}/sdk/sync/{}/fail", self.base_url, sync_run_id))
            .json(&request)
            .send()
            .await?;
        ensure_ok(response, "fail").await?;
        Ok(())
    }

    /// Get source configuration
    pub async fn get_source(&self, source_id: &str) -> SdkResult<Source> {
        debug!("SDK: Getting source config for source_id={}", source_id);

        let response = self
            .client
            .get(format!("{}/sdk/source/{}", self.base_url, source_id))
            .send()
            .await?;
        let response = ensure_ok(response, "get_source").await?;
        let source: Source = response.json().await?;
        Ok(source)
    }

    /// Get connector state for a source
    pub async fn get_connector_state(
        &self,
        source_id: &str,
    ) -> SdkResult<Option<serde_json::Value>> {
        debug!("SDK: Getting connector state for source_id={}", source_id);

        let response = self
            .client
            .get(format!(
                "{}/sdk/source/{}/sync-config",
                self.base_url, source_id
            ))
            .send()
            .await?;
        let response = ensure_ok(response, "get_connector_state").await?;
        let config: SyncConfigResponse = response.json().await?;
        Ok(config.connector_state)
    }

    /// Get credentials for a source
    pub async fn get_credentials(&self, source_id: &str) -> SdkResult<ServiceCredentials> {
        debug!("SDK: Getting credentials for source_id={}", source_id);

        let response = self
            .client
            .get(format!("{}/sdk/credentials/{}", self.base_url, source_id))
            .send()
            .await?;
        let response = ensure_ok(response, "get_credentials").await?;
        let credentials: ServiceCredentials = response.json().await?;
        Ok(credentials)
    }

    /// Create a new sync run for a source
    pub async fn create_sync_run(&self, source_id: &str, sync_type: SyncType) -> SdkResult<String> {
        debug!(
            "SDK: Creating sync run for source_id={}, type={:?}",
            source_id, sync_type
        );

        let request = CreateSyncRequest {
            source_id: source_id.to_string(),
            sync_type,
        };

        let response = self
            .client
            .post(format!("{}/sdk/sync/create", self.base_url))
            .json(&request)
            .send()
            .await?;
        let response = ensure_ok(response, "create_sync_run").await?;
        let result: CreateSyncResponse = response.json().await?;
        self.register_sync(&result.sync_run_id, sync_type).await;
        Ok(result.sync_run_id)
    }

    /// Cancel a sync run
    pub async fn cancel(&self, sync_run_id: &str) -> SdkResult<()> {
        debug!("SDK: Cancelling sync_run={}", sync_run_id);

        let request = CancelSyncRequest {
            sync_run_id: sync_run_id.to_string(),
        };

        let response = self
            .client
            .post(format!("{}/sdk/sync/cancel", self.base_url))
            .json(&request)
            .send()
            .await?;
        ensure_ok(response, "cancel").await?;
        Ok(())
    }

    /// Get user email for a source
    pub async fn get_user_email_for_source(&self, source_id: &str) -> SdkResult<String> {
        debug!("SDK: Getting user email for source_id={}", source_id);

        let response = self
            .client
            .get(format!(
                "{}/sdk/source/{}/user-email",
                self.base_url, source_id
            ))
            .send()
            .await?;
        let response = ensure_ok(response, "get_user_email_for_source").await?;
        let result: UserEmailResponse = response.json().await?;
        Ok(result.email)
    }

    /// Notify connector-manager of a webhook event
    /// Returns the sync_run_id created for this webhook
    pub async fn notify_webhook(&self, source_id: &str, event_type: &str) -> SdkResult<String> {
        debug!(
            "SDK: Notifying webhook for source_id={}, event_type={}",
            source_id, event_type
        );

        let request = WebhookNotificationRequest {
            source_id: source_id.to_string(),
            event_type: event_type.to_string(),
        };

        let response = self
            .client
            .post(format!("{}/sdk/webhook/notify", self.base_url))
            .json(&request)
            .send()
            .await?;
        let response = ensure_ok(response, "notify_webhook").await?;
        let result: WebhookNotificationResponse = response.json().await?;
        Ok(result.sync_run_id)
    }

    /// Save connector state for a source. **Critical**: buffered events for
    /// this source are flushed before the checkpoint is persisted. Without
    /// this, a crash after save_state could lose events that the connector
    /// already considers emitted (the next run resumes past them).
    pub async fn save_connector_state(
        &self,
        source_id: &str,
        state: serde_json::Value,
    ) -> SdkResult<()> {
        debug!("SDK: Saving connector state for source_id={}", source_id);

        self.flush_source(source_id).await?;

        let response = self
            .client
            .put(format!(
                "{}/sdk/source/{}/connector-state",
                self.base_url, source_id
            ))
            .json(&state)
            .send()
            .await?;
        ensure_ok(response, "save_connector_state").await?;

        Ok(())
    }

    /// Get connector config for a provider (e.g. OAuth app credentials)
    pub async fn get_connector_config(&self, provider: &str) -> SdkResult<serde_json::Value> {
        debug!("SDK: Getting connector config for provider={}", provider);

        let response = self
            .client
            .get(format!(
                "{}/sdk/connector-configs/{}",
                self.base_url, provider
            ))
            .send()
            .await?;
        let response = ensure_ok(response, "get_connector_config").await?;
        let config: serde_json::Value = response.json().await?;
        Ok(config)
    }

    /// Register this connector with the connector manager
    pub async fn register(&self, manifest: &ConnectorManifest) -> SdkResult<()> {
        debug!("SDK: Registering connector");

        let response = self
            .client
            .post(format!("{}/sdk/register", self.base_url))
            .json(manifest)
            .send()
            .await?;
        ensure_ok(response, "register").await?;
        Ok(())
    }

    /// Get all active sources of a given type
    pub async fn get_sources_by_type(&self, source_type: &str) -> SdkResult<Vec<Source>> {
        debug!("SDK: Getting sources by type={}", source_type);

        let response = self
            .client
            .get(format!(
                "{}/sdk/sources/by-type/{}",
                self.base_url, source_type
            ))
            .send()
            .await?;
        let response = ensure_ok(response, "get_sources_by_type").await?;
        let result: Vec<Source> = response.json().await?;
        Ok(result)
    }
}

/// Build the connector's own URL from CONNECTOR_HOST_NAME and PORT env vars.
/// Panics if CONNECTOR_HOST_NAME is not set — connectors cannot operate without
/// being reachable by the connector manager.
pub fn build_connector_url() -> String {
    let hostname = std::env::var("CONNECTOR_HOST_NAME").unwrap_or_else(|_| {
        panic!("CONNECTOR_HOST_NAME environment variable is required. Set it to this connector's hostname (e.g. the Docker service name).")
    });
    let port =
        std::env::var("PORT").unwrap_or_else(|_| panic!("PORT environment variable is required."));
    format!("http://{}:{}", hostname, port)
}

/// Spawn a background registration loop that re-registers with the connector
/// manager every 30 seconds. The manifest should already have `connector_url` set.
/// Panics if CONNECTOR_MANAGER_URL is not set.
pub fn start_registration_loop(manifest: ConnectorManifest) -> tokio::task::JoinHandle<()> {
    let sdk_client = SdkClient::from_env().unwrap_or_else(|_| {
        panic!("CONNECTOR_MANAGER_URL environment variable is required for connector registration.")
    });

    let handle = tokio::spawn(async move {
        let mut interval = tokio::time::interval(Duration::from_secs(30));

        loop {
            interval.tick().await;
            match sdk_client.register(&manifest).await {
                Ok(()) => info!("Registered with connector manager"),
                Err(e) => warn!("Registration failed: {}", e),
            }
        }
    });

    info!("Registration loop started");
    handle
}

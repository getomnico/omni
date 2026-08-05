use anyhow::Result;
use reqwest::{Client, Response, StatusCode};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::Mutex;
use tracing::{debug, error, warn};

use shared::models::{ConnectorEvent, ConnectorManifest, ServiceCredential, Source, SyncType};

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

/// Maximum number of retries when the extraction service responds with 429.
const MAX_EXTRACT_RETRIES: u32 = 20;

/// Minimum seconds to wait between extraction retries; the server-supplied
/// Retry-After value is used when it is larger than this.
const MIN_EXTRACT_RETRY_WAIT_SECS: u64 = 2;

/// Per-`SyncType` buffer thresholds: (size, time). `None` time means flush-on-emit.
fn thresholds_for(sync_type: SyncType) -> (usize, Option<Duration>) {
    match sync_type {
        SyncType::Full => (500, Some(Duration::from_secs(300))),
        SyncType::Incremental => (100, Some(Duration::from_secs(60))),
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
/// (`save_checkpoint`, `complete`, `fail`) must flush the relevant buffered
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

#[derive(Debug, Deserialize)]
struct SyncConfigResponse {
    connector_state: Option<serde_json::Value>,
    checkpoint: Option<serde_json::Value>,
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
        data: &[u8],
        mime_type: &str,
        filename: Option<&str>,
    ) -> reqwest::multipart::Form {
        let form = reqwest::multipart::Form::new()
            .text("sync_run_id", sync_run_id.to_string())
            .text("mime_type", mime_type.to_string())
            .part(
                "data",
                reqwest::multipart::Part::bytes(data.to_vec())
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

        for attempt in 0..MAX_EXTRACT_RETRIES {
            let form = Self::build_extract_form(sync_run_id, &data, mime_type, filename);
            let response = self
                .client
                .post(format!("{}/sdk/extract-text", self.base_url))
                .multipart(form)
                .send()
                .await?;

            if response.status() == StatusCode::TOO_MANY_REQUESTS {
                let retry_after = response
                    .headers()
                    .get("retry-after")
                    .and_then(|v| v.to_str().ok())
                    .and_then(|v| v.parse::<u64>().ok())
                    .unwrap_or(30);
                if attempt + 1 >= MAX_EXTRACT_RETRIES {
                    return Err(SdkError::Http {
                        operation: "extract_text",
                        status: StatusCode::TOO_MANY_REQUESTS,
                        body: "extraction service overloaded, max retries exceeded".to_string(),
                    });
                }
                let wait = retry_after.max(MIN_EXTRACT_RETRY_WAIT_SECS);
                warn!(
                    "Extraction service overloaded for '{}', retrying in {}s ({}/{})",
                    filename.unwrap_or("<unnamed>"),
                    wait,
                    attempt + 1,
                    MAX_EXTRACT_RETRIES
                );
                tokio::time::sleep(Duration::from_secs(wait)).await;
                continue;
            }

            let response = ensure_ok(response, "extract_text").await?;
            let result: ExtractTextResponse = response.json().await?;
            return Ok(result.text);
        }

        unreachable!()
    }

    /// Emit a document event. Events are buffered in memory and auto-flushed
    /// according to the sync's type (see [`thresholds_for`]):
    /// - Full: 500 events or 5min, whichever first
    /// - Incremental: 100 events or 60s
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

        let result = async {
            let response = self
                .client
                .post(format!("{}/sdk/events/batch", self.base_url))
                .json(&request)
                .send()
                .await?;
            ensure_ok(response, "flush_events").await?;
            SdkResult::Ok(())
        }
        .await;

        if let Err(error) = result {
            error!(
                sync_run_id,
                source_id,
                batch_size,
                error = %error,
                "SDK: Failed to flush events; retaining batch for retry"
            );
            let mut buffer = self.event_buffer.lock().await;
            let entry = buffer.entry(key).or_insert_with(|| BufferEntry {
                events: Vec::new(),
                oldest_at: Instant::now(),
            });
            let mut retained = request.events;
            retained.append(&mut entry.events);
            entry.events = retained;
            return Err(error.into());
        }
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

    /// Flush only the buffers belonging to one sync run.
    ///
    /// Completion/failure for one run must never be blocked by another run's
    /// retained buffer (for example an old failed run whose events connector
    /// manager now rejects because the run is terminal).
    pub async fn flush_sync(&self, sync_run_id: &str) -> Result<()> {
        let keys: Vec<BufferKey> = {
            let buffer = self.event_buffer.lock().await;
            buffer
                .keys()
                .filter(|(sid, _)| sid == sync_run_id)
                .cloned()
                .collect()
        };

        for (sid, source_id) in keys {
            self.flush_events(&sid, &source_id).await?;
        }
        Ok(())
    }

    /// Discard any buffered events for a sync run that is now terminal
    /// (failed or cancelled). Connector manager rejects events belonging to
    /// non-running runs, so a retained batch can never be admitted; leaving
    /// it in the buffer would only wedge later runs that used to flush all
    /// runs before completing.
    ///
    /// Callers must only invoke this after connector manager has confirmed
    /// the run is terminal; discarding first would drop events that could
    /// still be admitted if the terminalization request failed.
    pub async fn discard_sync(&self, sync_run_id: &str) {
        let mut buffer = self.event_buffer.lock().await;
        buffer.retain(|(sid, _), _| sid != sync_run_id);
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

        for attempt in 0..MAX_EXTRACT_RETRIES {
            let form = Self::build_extract_form(sync_run_id, &data, mime_type, filename);
            let response = self
                .client
                .post(format!("{}/sdk/extract-content", self.base_url))
                .multipart(form)
                .send()
                .await?;

            if response.status() == StatusCode::TOO_MANY_REQUESTS {
                let retry_after = response
                    .headers()
                    .get("retry-after")
                    .and_then(|v| v.to_str().ok())
                    .and_then(|v| v.parse::<u64>().ok())
                    .unwrap_or(30);
                if attempt + 1 >= MAX_EXTRACT_RETRIES {
                    return Err(SdkError::Http {
                        operation: "extract_and_store_content",
                        status: StatusCode::TOO_MANY_REQUESTS,
                        body: "extraction service overloaded, max retries exceeded".to_string(),
                    });
                }
                let wait = retry_after.max(MIN_EXTRACT_RETRY_WAIT_SECS);
                warn!(
                    "Extraction service overloaded for '{}', retrying in {}s ({}/{})",
                    filename.unwrap_or("<unnamed>"),
                    wait,
                    attempt + 1,
                    MAX_EXTRACT_RETRIES
                );
                tokio::time::sleep(Duration::from_secs(wait)).await;
                continue;
            }

            let response = ensure_ok(response, "extract_and_store_content").await?;
            let result: StoreContentResponse = response.json().await?;
            return Ok(result.content_id);
        }

        unreachable!()
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

    /// Increment updated count. Use alongside `increment_scanned` so the
    /// running tally on the manager survives mid-sync crashes — the absolute
    /// value reported via `complete()` reflects only the current attempt.
    pub async fn increment_updated(&self, sync_run_id: &str, count: i32) -> SdkResult<()> {
        debug!(
            "SDK: Incrementing updated for sync_run={} by {}",
            sync_run_id, count
        );

        let response = self
            .client
            .post(format!(
                "{}/sdk/sync/{}/updated",
                self.base_url, sync_run_id
            ))
            .json(&serde_json::json!({ "count": count }))
            .send()
            .await?;
        ensure_ok(response, "increment_updated").await?;
        Ok(())
    }

    /// Mark sync as completed. Flushes this run's buffered events first so the
    /// completion never races ahead of the final events for this sync. Only the
    /// current run's buffers are flushed — another run's retained batch (e.g. a
    /// terminal run connector manager now rejects) must never block completion.
    pub async fn complete(&self, sync_run_id: &str) -> SdkResult<()> {
        debug!("SDK: Completing sync_run={}", sync_run_id);

        self.flush_sync(sync_run_id).await?;

        let response = self
            .client
            .post(format!(
                "{}/sdk/sync/{}/complete",
                self.base_url, sync_run_id
            ))
            .send()
            .await?;
        ensure_ok(response, "complete").await?;
        Ok(())
    }

    /// Mark sync as failed. Best-effort flush of this run's buffered events
    /// first — if the flush itself fails we log and proceed, because marking
    /// the sync as failed is more important than preserving partial progress.
    /// Any events that could not be flushed are then discarded: the run is
    /// terminal and connector manager will reject them anyway, so retaining
    /// them could only wedge a later run that used to flush all runs.
    pub async fn fail(&self, sync_run_id: &str, error: &str) -> SdkResult<()> {
        debug!("SDK: Failing sync_run={}: {}", sync_run_id, error);

        // Best-effort flush of this run's buffered events first — if the
        // flush itself fails we log and proceed, because marking the sync as
        // failed is more important than preserving partial progress.
        if let Err(e) = self.flush_sync(sync_run_id).await {
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

        // Only after connector manager confirms the failure is the run
        // terminal. Discard whatever the best-effort flush retained; an
        // unsuccessful fail request leaves the run running and must keep the
        // buffer intact.
        self.discard_sync(sync_run_id).await;
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

    /// Get checkpoint for a source (latest successfully completed sync).
    pub async fn get_checkpoint(&self, source_id: &str) -> SdkResult<Option<serde_json::Value>> {
        debug!("SDK: Getting checkpoint for source_id={}", source_id);

        let response = self
            .client
            .get(format!(
                "{}/sdk/source/{}/sync-config",
                self.base_url, source_id
            ))
            .send()
            .await?;
        let response = ensure_ok(response, "get_checkpoint").await?;
        let config: SyncConfigResponse = response.json().await?;
        Ok(config.checkpoint)
    }

    /// Get credentials for a source
    pub async fn get_credentials(&self, source_id: &str) -> SdkResult<ServiceCredential> {
        debug!("SDK: Getting credentials for source_id={}", source_id);

        let response = self
            .client
            .get(format!("{}/sdk/credentials/{}", self.base_url, source_id))
            .send()
            .await?;
        let response = ensure_ok(response, "get_credentials").await?;
        let credentials: ServiceCredential = response.json().await?;
        Ok(credentials)
    }

    /// Create a new sync run for a source.
    ///
    /// Under normal circumstances, the connector-manager is responsible for
    /// creating sync runs before calling the connector's `/sync` endpoint. We
    /// allow connectors to also create sync runs when work is initiated from
    /// inside the connector itself, such as a realtime/webhook event that
    /// needs to trigger a short follow-up incremental sync.
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

    /// Cancel a sync run. Buffered events for the run are discarded: the run
    /// becomes terminal and connector manager rejects its events, so a retained
    /// batch could never be admitted and would only wedge later runs.
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

        // Only after connector manager confirms the cancellation is the run
        // terminal; an unsuccessful cancel request leaves the run running and
        // must keep its buffered events.
        self.discard_sync(sync_run_id).await;
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

    /// Save a run-scoped sync checkpoint. **Critical**: buffered events for
    /// this sync/source pair are flushed before the checkpoint is persisted.
    /// Without this, a crash after checkpointing could lose events that the
    /// connector already considers emitted (the next resume runs past them).
    pub async fn save_checkpoint(
        &self,
        sync_run_id: &str,
        source_id: &str,
        checkpoint: serde_json::Value,
    ) -> SdkResult<()> {
        debug!("SDK: Saving checkpoint for sync_run={}", sync_run_id);

        self.flush_events(sync_run_id, source_id).await?;

        let response = self
            .client
            .put(format!(
                "{}/sdk/sync/{}/checkpoint",
                self.base_url, sync_run_id
            ))
            .json(&checkpoint)
            .send()
            .await?;
        ensure_ok(response, "save_checkpoint").await?;

        Ok(())
    }

    pub async fn save_connector_state(
        &self,
        source_id: &str,
        state: serde_json::Value,
    ) -> SdkResult<()> {
        debug!("SDK: Saving connector metadata for source_id={}", source_id);

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

#[cfg(test)]
mod tests {
    use super::*;
    use axum::{Router, http::StatusCode, routing::post};
    use shared::models::{ConnectorEvent, PersonSyncRecord, SyncType};

    fn person_event() -> ConnectorEvent {
        ConnectorEvent::PersonSync {
            sync_run_id: "run-1".into(),
            source_id: "source-1".into(),
            person: PersonSyncRecord {
                external_id: "E1".into(),
                email: "ada@example.com".into(),
                display_name: None,
                given_name: None,
                middle_name: None,
                surname: None,
                job_title: None,
                department: None,
                division: None,
                company_name: None,
                office_location: None,
                work_country: None,
                employee_id: None,
                employee_type: None,
                cost_center: None,
                grade: None,
                band: None,
                confirmation_status: None,
                employment_start_date: None,
                employment_end_date: None,
                phone: None,
                is_active: None,
                top_department: None,
                manager_external_id: None,
                source_updated_at: None,
            },
        }
    }

    #[tokio::test]
    async fn failed_person_flush_is_retained_and_propagated() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        tokio::spawn(async move {
            axum::serve(
                listener,
                Router::new().route(
                    "/sdk/events/batch",
                    post(|| async { StatusCode::SERVICE_UNAVAILABLE }),
                ),
            )
            .await
            .unwrap();
        });
        let client = SdkClient::new(&format!("http://{address}"));
        client
            .sync_types
            .lock()
            .await
            .insert("run-1".into(), SyncType::Full);
        client
            .emit_event("run-1", "source-1", person_event())
            .await
            .unwrap();
        let error = client.flush_events("run-1", "source-1").await.unwrap_err();
        assert!(error.to_string().contains("flush_events"));
        let buffer = client.event_buffer.lock().await;
        let retained = &buffer
            .get(&("run-1".into(), "source-1".into()))
            .unwrap()
            .events;
        assert_eq!(retained.len(), 1);
        assert!(matches!(
            retained[0],
            ConnectorEvent::PersonSync { ref person, .. } if person.email == "ada@example.com"
        ));
    }

    #[tokio::test]
    async fn terminal_run_retained_batch_does_not_block_later_run_completion() {
        use axum::body::Body;
        use axum::http::Request as AxumRequest;

        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        tokio::spawn(async move {
            axum::serve(
                listener,
                Router::new()
                    .route(
                        "/sdk/events/batch",
                        post(|req: AxumRequest<Body>| async move {
                            let bytes = axum::body::to_bytes(req.into_body(), 64 * 1024)
                                .await
                                .unwrap();
                            let value: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
                            // Connector manager rejects events for a terminal
                            // run: run-A is treated as already failed, so its
                            // retained batch must not be re-flushed.
                            if value["sync_run_id"] == "run-A" {
                                StatusCode::SERVICE_UNAVAILABLE
                            } else {
                                StatusCode::OK
                            }
                        }),
                    )
                    .route("/sdk/sync/:id/complete", post(|| async { StatusCode::OK }))
                    .route("/sdk/sync/:id/fail", post(|| async { StatusCode::OK }))
                    .route("/sdk/sync/cancel", post(|| async { StatusCode::OK })),
            )
            .await
            .unwrap();
        });

        let client = SdkClient::new(&format!("http://{address}"));
        for (sync_run_id, _source_id) in [("run-A", "source-A"), ("run-B", "source-B")] {
            client
                .sync_types
                .lock()
                .await
                .insert(sync_run_id.into(), SyncType::Full);
        }

        // Run A: events buffered, flush rejected (run is effectively terminal),
        // then the run is failed. The retained batch must be discarded.
        client
            .emit_event("run-A", "source-A", person_event())
            .await
            .unwrap();
        assert!(client.flush_events("run-A", "source-A").await.is_err());
        // Failed flush retains the batch for a non-terminal run.
        assert!(
            client
                .event_buffer
                .lock()
                .await
                .contains_key(&("run-A".into(), "source-A".into()))
        );
        // Marking the run failed must discard its inadmissible retained batch.
        client.fail("run-A", "boom").await.unwrap();
        assert!(client.event_buffer.lock().await.is_empty());

        // Run B: must complete despite run A's earlier retained batch. The
        // old flush_all() path would have re-flushed run A's events and failed.
        client
            .emit_event("run-B", "source-B", person_event())
            .await
            .unwrap();
        client.complete("run-B").await.unwrap();
        assert!(client.event_buffer.lock().await.is_empty());
    }

    #[tokio::test]
    async fn cancel_discards_buffered_events_for_that_run() {
        use axum::body::Body;
        use axum::http::Request as AxumRequest;

        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        tokio::spawn(async move {
            axum::serve(
                listener,
                Router::new().route(
                    "/sdk/sync/cancel",
                    post(|_req: AxumRequest<Body>| async { StatusCode::OK }),
                ),
            )
            .await
            .unwrap();
        });

        let client = SdkClient::new(&format!("http://{address}"));
        client
            .sync_types
            .lock()
            .await
            .insert("run-A".into(), SyncType::Full);
        client
            .emit_event("run-A", "source-A", person_event())
            .await
            .unwrap();
        assert!(
            client
                .event_buffer
                .lock()
                .await
                .contains_key(&("run-A".into(), "source-A".into()))
        );
        client.cancel("run-A").await.unwrap();
        assert!(client.event_buffer.lock().await.is_empty());
    }

    #[tokio::test]
    async fn failed_fail_request_retains_buffered_events() {
        use axum::body::Body;
        use axum::http::Request as AxumRequest;

        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        tokio::spawn(async move {
            axum::serve(
                listener,
                Router::new()
                    .route(
                        "/sdk/events/batch",
                        post(|| async { StatusCode::SERVICE_UNAVAILABLE }),
                    )
                    .route(
                        "/sdk/sync/:id/fail",
                        post(|_req: AxumRequest<Body>| async { StatusCode::INTERNAL_SERVER_ERROR }),
                    ),
            )
            .await
            .unwrap();
        });

        let client = SdkClient::new(&format!("http://{address}"));
        client
            .sync_types
            .lock()
            .await
            .insert("run-A".into(), SyncType::Full);
        client
            .emit_event("run-A", "source-A", person_event())
            .await
            .unwrap();
        assert!(client.flush_events("run-A", "source-A").await.is_err());
        assert!(
            client
                .event_buffer
                .lock()
                .await
                .contains_key(&("run-A".into(), "source-A".into()))
        );

        // Connector manager rejects the fail request: the run is still
        // running, so the buffered events must survive.
        assert!(client.fail("run-A", "boom").await.is_err());
        assert_eq!(
            client
                .event_buffer
                .lock()
                .await
                .get(&("run-A".into(), "source-A".into()))
                .unwrap()
                .events
                .len(),
            1,
            "an unsuccessful fail request must retain the run's buffer"
        );
    }

    #[tokio::test]
    async fn successful_fail_discards_buffered_events() {
        use axum::body::Body;
        use axum::http::Request as AxumRequest;

        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        tokio::spawn(async move {
            axum::serve(
                listener,
                Router::new()
                    .route(
                        "/sdk/events/batch",
                        post(|| async { StatusCode::SERVICE_UNAVAILABLE }),
                    )
                    .route(
                        "/sdk/sync/:id/fail",
                        post(|_req: AxumRequest<Body>| async { StatusCode::OK }),
                    ),
            )
            .await
            .unwrap();
        });

        let client = SdkClient::new(&format!("http://{address}"));
        client
            .sync_types
            .lock()
            .await
            .insert("run-A".into(), SyncType::Full);
        client
            .emit_event("run-A", "source-A", person_event())
            .await
            .unwrap();
        assert!(client.flush_events("run-A", "source-A").await.is_err());
        assert!(
            client
                .event_buffer
                .lock()
                .await
                .contains_key(&("run-A".into(), "source-A".into()))
        );

        // Once connector manager confirms the failure, the run is terminal
        // and its retained batch must be discarded.
        client.fail("run-A", "boom").await.unwrap();
        assert!(
            client.event_buffer.lock().await.is_empty(),
            "a confirmed fail must discard the run's retained buffer"
        );
    }

    #[tokio::test]
    async fn failed_cancel_request_retains_buffered_events() {
        use std::sync::atomic::{AtomicUsize, Ordering};

        use axum::body::Body;
        use axum::http::Request as AxumRequest;

        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let calls = Arc::new(AtomicUsize::new(0));
        let server_calls = Arc::clone(&calls);
        tokio::spawn(async move {
            axum::serve(
                listener,
                Router::new().route(
                    "/sdk/sync/cancel",
                    post(move |_req: AxumRequest<Body>| {
                        let calls = Arc::clone(&server_calls);
                        async move {
                            // First request fails, the retry succeeds.
                            if calls.fetch_add(1, Ordering::SeqCst) == 0 {
                                StatusCode::INTERNAL_SERVER_ERROR
                            } else {
                                StatusCode::OK
                            }
                        }
                    }),
                ),
            )
            .await
            .unwrap();
        });

        let client = SdkClient::new(&format!("http://{address}"));
        client
            .sync_types
            .lock()
            .await
            .insert("run-A".into(), SyncType::Full);
        client
            .emit_event("run-A", "source-A", person_event())
            .await
            .unwrap();
        assert!(
            client
                .event_buffer
                .lock()
                .await
                .contains_key(&("run-A".into(), "source-A".into()))
        );

        // Unsuccessful cancel: the run is still running, buffer intact.
        assert!(client.cancel("run-A").await.is_err());
        assert!(
            client
                .event_buffer
                .lock()
                .await
                .contains_key(&("run-A".into(), "source-A".into()))
        );

        // Confirmed cancel: terminal, buffer discarded.
        client.cancel("run-A").await.unwrap();
        assert!(client.event_buffer.lock().await.is_empty());
    }
}

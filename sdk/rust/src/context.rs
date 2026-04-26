use anyhow::Result;
use shared::models::{ConnectorEvent, SourceType, SyncType};
use shared::SdkClient;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use tracing::warn;

#[derive(Clone)]
pub struct SyncContext {
    sdk_client: SdkClient,
    sync_run_id: String,
    source_id: String,
    source_type: SourceType,
    sync_mode: SyncType,
    cancelled: Arc<AtomicBool>,
}

impl SyncContext {
    pub fn new(
        sdk_client: SdkClient,
        sync_run_id: String,
        source_id: String,
        source_type: SourceType,
        sync_mode: SyncType,
        cancelled: Arc<AtomicBool>,
    ) -> Self {
        Self {
            sdk_client,
            sync_run_id,
            source_id,
            source_type,
            sync_mode,
            cancelled,
        }
    }

    pub fn sdk_client(&self) -> &SdkClient {
        &self.sdk_client
    }

    pub fn sync_run_id(&self) -> &str {
        &self.sync_run_id
    }

    pub fn source_id(&self) -> &str {
        &self.source_id
    }

    pub fn source_type(&self) -> SourceType {
        self.source_type
    }

    pub fn sync_mode(&self) -> SyncType {
        self.sync_mode
    }

    pub fn is_cancelled(&self) -> bool {
        self.cancelled.load(Ordering::SeqCst)
    }

    /// Emit a single event. Events are buffered in memory and auto-flushed
    /// according to the sync's mode (see [`thresholds_for`]):
    /// - Full: 500 events or 5min, whichever first
    /// - Incremental: 100 events or 60s
    /// - Realtime: flush on every emit
    ///
    /// If an auto-flush fails, the error is returned to the caller so the
    /// connector knows the event was not persisted before checkpointing.
    pub async fn emit_event(&self, event: ConnectorEvent) -> Result<()> {
        self.sdk_client
            .emit_event(&self.sync_run_id, &self.source_id, event)
            .await?;
        Ok(())
    }

    /// Flush all buffered events for this (sync_run_id, source_id) pair.
    pub async fn flush(&self) -> Result<()> {
        self.sdk_client
            .flush_events(&self.sync_run_id, &self.source_id)
            .await?;
        Ok(())
    }

    /// Flush all buffered events across all (sync_run_id, source_id) pairs.
    pub async fn flush_all(&self) -> Result<()> {
        self.sdk_client.flush_all().await?;
        Ok(())
    }

    pub async fn extract_and_store_content(
        &self,
        data: Vec<u8>,
        mime_type: &str,
        filename: Option<&str>,
    ) -> Result<String> {
        Ok(self
            .sdk_client
            .extract_and_store_content(&self.sync_run_id, data, mime_type, filename)
            .await?)
    }

    pub async fn store_content(&self, content: &str) -> Result<String> {
        Ok(self
            .sdk_client
            .store_content(&self.sync_run_id, content)
            .await?)
    }

    pub async fn increment_scanned(&self, count: i32) -> Result<()> {
        self.sdk_client
            .increment_scanned(&self.sync_run_id, count)
            .await?;
        Ok(())
    }

    pub async fn increment_updated(&self, count: i32) -> Result<()> {
        self.sdk_client
            .increment_updated(&self.sync_run_id, count)
            .await?;
        Ok(())
    }

    /// Mark sync as completed. Flushes any buffered events first so the
    /// completion never races ahead of the final events for this sync.
    /// Status flip only — counts come from `increment_scanned`/`updated`,
    /// connector state from `save_connector_state`.
    pub async fn complete(&self) -> Result<()> {
        self.sdk_client.complete(&self.sync_run_id).await?;
        Ok(())
    }

    /// Mark sync as failed. Best-effort flush of buffered events first — if
    /// the flush itself fails we log and proceed, because marking the sync as
    /// failed is more important than preserving partial progress.
    pub async fn fail(&self, error: &str) -> Result<()> {
        if let Err(e) = self.flush_all().await {
            warn!(
                "SDK: flush before fail() failed (continuing): sync_run={}: {}",
                self.sync_run_id, e
            );
        }
        self.sdk_client.fail(&self.sync_run_id, error).await?;
        Ok(())
    }

    pub async fn heartbeat(&self) -> Result<()> {
        self.sdk_client.heartbeat(&self.sync_run_id).await?;
        Ok(())
    }

    pub async fn cancel(&self) -> Result<()> {
        self.sdk_client.cancel(&self.sync_run_id).await?;
        Ok(())
    }

    /// Checkpoint state for resumability. Flushes buffered events first —
    /// without this, a crash after checkpointing would lose events that the
    /// connector considered emitted (the next run resumes past them).
    pub async fn save_connector_state(&self, state: serde_json::Value) -> Result<()> {
        self.sdk_client
            .save_connector_state(&self.source_id, state)
            .await?;
        Ok(())
    }

    pub async fn get_user_email_for_source(&self) -> Result<String> {
        Ok(self
            .sdk_client
            .get_user_email_for_source(&self.source_id)
            .await?)
    }
}

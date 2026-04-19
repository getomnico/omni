use anyhow::Result;
use shared::models::{ConnectorEvent, SourceType};
use shared::SdkClient;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

#[derive(Clone)]
pub struct SyncContext {
    sdk_client: SdkClient,
    sync_run_id: String,
    source_id: String,
    source_type: SourceType,
    cancelled: Arc<AtomicBool>,
}

impl SyncContext {
    pub fn new(
        sdk_client: SdkClient,
        sync_run_id: String,
        source_id: String,
        source_type: SourceType,
        cancelled: Arc<AtomicBool>,
    ) -> Self {
        Self {
            sdk_client,
            sync_run_id,
            source_id,
            source_type,
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

    pub fn is_cancelled(&self) -> bool {
        self.cancelled.load(Ordering::SeqCst)
    }

    pub async fn emit_event(&self, event: ConnectorEvent) -> Result<()> {
        self.sdk_client
            .emit_event(&self.sync_run_id, &self.source_id, event)
            .await?;
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

    pub async fn complete(
        &self,
        documents_scanned: i32,
        documents_updated: i32,
        new_state: Option<serde_json::Value>,
    ) -> Result<()> {
        self.sdk_client
            .complete(
                &self.sync_run_id,
                documents_scanned,
                documents_updated,
                new_state,
            )
            .await?;
        Ok(())
    }

    pub async fn fail(&self, error: &str) -> Result<()> {
        self.sdk_client.fail(&self.sync_run_id, error).await?;
        Ok(())
    }

    pub async fn cancel(&self) -> Result<()> {
        self.sdk_client.cancel(&self.sync_run_id).await?;
        Ok(())
    }

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

use anyhow::{Context, Result};
use reqwest::Client;
use serde::{Deserialize, Serialize};
use tracing::debug;

use crate::models::ConnectorEvent;

/// HTTP client for communicating with connector-manager SDK endpoints.
/// This is the standard way for connectors to interact with the connector-manager
/// for emitting events, storing content, and reporting sync status.
#[derive(Clone)]
pub struct SdkClient {
    client: Client,
    base_url: String,
}

#[derive(Debug, Serialize)]
struct EmitEventRequest {
    sync_run_id: String,
    source_id: String,
    event: ConnectorEvent,
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

#[derive(Debug, Serialize)]
struct FailRequest {
    error: String,
}

impl SdkClient {
    pub fn new(connector_manager_url: &str) -> Self {
        Self {
            client: Client::new(),
            base_url: connector_manager_url.trim_end_matches('/').to_string(),
        }
    }

    pub fn from_env() -> Result<Self> {
        let url =
            std::env::var("CONNECTOR_MANAGER_URL").context("CONNECTOR_MANAGER_URL not set")?;
        Ok(Self::new(&url))
    }

    /// Emit a document event to the queue
    pub async fn emit_event(
        &self,
        sync_run_id: &str,
        source_id: &str,
        event: ConnectorEvent,
    ) -> Result<()> {
        debug!("SDK: Emitting event for sync_run={}", sync_run_id);

        let request = EmitEventRequest {
            sync_run_id: sync_run_id.to_string(),
            source_id: source_id.to_string(),
            event,
        };

        let response = self
            .client
            .post(format!("{}/sdk/events", self.base_url))
            .json(&request)
            .send()
            .await
            .context("Failed to send emit event request")?;

        if !response.status().is_success() {
            let status = response.status();
            let body = response.text().await.unwrap_or_default();
            anyhow::bail!("Failed to emit event: {} - {}", status, body);
        }

        Ok(())
    }

    /// Store content and return content_id
    pub async fn store_content(&self, sync_run_id: &str, content: &str) -> Result<String> {
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
            .await
            .context("Failed to send store content request")?;

        if !response.status().is_success() {
            let status = response.status();
            let body = response.text().await.unwrap_or_default();
            anyhow::bail!("Failed to store content: {} - {}", status, body);
        }

        let result: StoreContentResponse = response.json().await?;
        Ok(result.content_id)
    }

    /// Send heartbeat to update last_activity_at
    pub async fn heartbeat(&self, sync_run_id: &str) -> Result<()> {
        debug!("SDK: Heartbeat for sync_run={}", sync_run_id);

        let response = self
            .client
            .post(format!(
                "{}/sdk/sync/{}/heartbeat",
                self.base_url, sync_run_id
            ))
            .send()
            .await
            .context("Failed to send heartbeat")?;

        if !response.status().is_success() {
            let status = response.status();
            let body = response.text().await.unwrap_or_default();
            anyhow::bail!("Failed to heartbeat: {} - {}", status, body);
        }

        Ok(())
    }

    /// Increment scanned count and update heartbeat
    pub async fn increment_scanned(&self, sync_run_id: &str) -> Result<()> {
        debug!("SDK: Incrementing scanned for sync_run={}", sync_run_id);

        let response = self
            .client
            .post(format!(
                "{}/sdk/sync/{}/scanned",
                self.base_url, sync_run_id
            ))
            .send()
            .await
            .context("Failed to send increment scanned")?;

        if !response.status().is_success() {
            let status = response.status();
            let body = response.text().await.unwrap_or_default();
            anyhow::bail!("Failed to increment scanned: {} - {}", status, body);
        }

        Ok(())
    }

    /// Mark sync as completed
    pub async fn complete(
        &self,
        sync_run_id: &str,
        documents_scanned: i32,
        documents_updated: i32,
        new_state: Option<serde_json::Value>,
    ) -> Result<()> {
        debug!("SDK: Completing sync_run={}", sync_run_id);

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
            .await
            .context("Failed to send complete request")?;

        if !response.status().is_success() {
            let status = response.status();
            let body = response.text().await.unwrap_or_default();
            anyhow::bail!("Failed to complete: {} - {}", status, body);
        }

        Ok(())
    }

    /// Mark sync as failed
    pub async fn fail(&self, sync_run_id: &str, error: &str) -> Result<()> {
        debug!("SDK: Failing sync_run={}: {}", sync_run_id, error);

        let request = FailRequest {
            error: error.to_string(),
        };

        let response = self
            .client
            .post(format!("{}/sdk/sync/{}/fail", self.base_url, sync_run_id))
            .json(&request)
            .send()
            .await
            .context("Failed to send fail request")?;

        if !response.status().is_success() {
            let status = response.status();
            let body = response.text().await.unwrap_or_default();
            anyhow::bail!("Failed to mark as failed: {} - {}", status, body);
        }

        Ok(())
    }
}

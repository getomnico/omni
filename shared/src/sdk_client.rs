use anyhow::{Context, Result};
use reqwest::Client;
use serde::{Deserialize, Serialize};
use tracing::debug;

use crate::models::{ConnectorEvent, ServiceCredentials, Source, SyncType};

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
    pub async fn increment_scanned(&self, sync_run_id: &str, count: i32) -> Result<()> {
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

    /// Get source configuration
    pub async fn get_source(&self, source_id: &str) -> Result<Source> {
        debug!("SDK: Getting source config for source_id={}", source_id);

        let response = self
            .client
            .get(format!("{}/sdk/source/{}", self.base_url, source_id))
            .send()
            .await
            .context("Failed to send get source request")?;

        if !response.status().is_success() {
            let status = response.status();
            let body = response.text().await.unwrap_or_default();
            anyhow::bail!("Failed to get source: {} - {}", status, body);
        }

        let source: Source = response
            .json()
            .await
            .context("Failed to parse source response")?;
        Ok(source)
    }

    /// Get connector state for a source
    pub async fn get_connector_state(&self, source_id: &str) -> Result<Option<serde_json::Value>> {
        debug!("SDK: Getting connector state for source_id={}", source_id);

        let response = self
            .client
            .get(format!(
                "{}/sdk/source/{}/sync-config",
                self.base_url, source_id
            ))
            .send()
            .await
            .context("Failed to send get sync config request")?;

        if !response.status().is_success() {
            let status = response.status();
            let body = response.text().await.unwrap_or_default();
            anyhow::bail!("Failed to get sync config: {} - {}", status, body);
        }

        let config: SyncConfigResponse = response
            .json()
            .await
            .context("Failed to parse sync config response")?;
        Ok(config.connector_state)
    }

    /// Get credentials for a source
    pub async fn get_credentials(&self, source_id: &str) -> Result<ServiceCredentials> {
        debug!("SDK: Getting credentials for source_id={}", source_id);

        let response = self
            .client
            .get(format!("{}/sdk/credentials/{}", self.base_url, source_id))
            .send()
            .await
            .context("Failed to send get credentials request")?;

        if !response.status().is_success() {
            let status = response.status();
            let body = response.text().await.unwrap_or_default();
            anyhow::bail!("Failed to get credentials: {} - {}", status, body);
        }

        let credentials: ServiceCredentials = response
            .json()
            .await
            .context("Failed to parse credentials response")?;
        Ok(credentials)
    }

    /// Create a new sync run for a source
    pub async fn create_sync_run(&self, source_id: &str, sync_type: SyncType) -> Result<String> {
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
            .await
            .context("Failed to send create sync run request")?;

        if !response.status().is_success() {
            let status = response.status();
            let body = response.text().await.unwrap_or_default();
            anyhow::bail!("Failed to create sync run: {} - {}", status, body);
        }

        let result: CreateSyncResponse = response
            .json()
            .await
            .context("Failed to parse create sync response")?;
        Ok(result.sync_run_id)
    }

    /// Cancel a sync run
    pub async fn cancel(&self, sync_run_id: &str) -> Result<()> {
        debug!("SDK: Cancelling sync_run={}", sync_run_id);

        let request = CancelSyncRequest {
            sync_run_id: sync_run_id.to_string(),
        };

        let response = self
            .client
            .post(format!("{}/sdk/sync/cancel", self.base_url))
            .json(&request)
            .send()
            .await
            .context("Failed to send cancel request")?;

        if !response.status().is_success() {
            let status = response.status();
            let body = response.text().await.unwrap_or_default();
            anyhow::bail!("Failed to cancel sync: {} - {}", status, body);
        }

        Ok(())
    }

    /// Get user email for a source
    pub async fn get_user_email_for_source(&self, source_id: &str) -> Result<String> {
        debug!("SDK: Getting user email for source_id={}", source_id);

        let response = self
            .client
            .get(format!(
                "{}/sdk/source/{}/user-email",
                self.base_url, source_id
            ))
            .send()
            .await
            .context("Failed to send get user email request")?;

        if !response.status().is_success() {
            let status = response.status();
            let body = response.text().await.unwrap_or_default();
            anyhow::bail!("Failed to get user email: {} - {}", status, body);
        }

        let result: UserEmailResponse = response
            .json()
            .await
            .context("Failed to parse user email response")?;
        Ok(result.email)
    }

    /// Notify connector-manager of a webhook event
    /// Returns the sync_run_id created for this webhook
    pub async fn notify_webhook(&self, source_id: &str, event_type: &str) -> Result<String> {
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
            .await
            .context("Failed to send webhook notification")?;

        if !response.status().is_success() {
            let status = response.status();
            let body = response.text().await.unwrap_or_default();
            anyhow::bail!("Failed to notify webhook: {} - {}", status, body);
        }

        let result: WebhookNotificationResponse = response
            .json()
            .await
            .context("Failed to parse webhook notification response")?;
        Ok(result.sync_run_id)
    }

    /// Save connector state for a source
    pub async fn save_connector_state(
        &self,
        source_id: &str,
        state: serde_json::Value,
    ) -> Result<()> {
        debug!("SDK: Saving connector state for source_id={}", source_id);

        let response = self
            .client
            .put(format!(
                "{}/sdk/source/{}/connector-state",
                self.base_url, source_id
            ))
            .json(&state)
            .send()
            .await
            .context("Failed to save connector state")?;

        if !response.status().is_success() {
            let status = response.status();
            let body = response.text().await.unwrap_or_default();
            anyhow::bail!("Failed to save connector state: {} - {}", status, body);
        }

        Ok(())
    }

    /// Get all active sources of a given type
    pub async fn get_sources_by_type(&self, source_type: &str) -> Result<Vec<Source>> {
        debug!("SDK: Getting sources by type={}", source_type);

        let response = self
            .client
            .get(format!(
                "{}/sdk/sources/by-type/{}",
                self.base_url, source_type
            ))
            .send()
            .await
            .context("Failed to get sources by type")?;

        if !response.status().is_success() {
            let status = response.status();
            let body = response.text().await.unwrap_or_default();
            anyhow::bail!("Failed to get sources by type: {} - {}", status, body);
        }

        let result: Vec<Source> = response
            .json()
            .await
            .context("Failed to parse sources response")?;
        Ok(result)
    }
}

use crate::models::{ActionRequest, ActionResponse, ConnectorManifest, SyncRequest, SyncResponse};
use reqwest::Client;
use std::time::Duration;
use tracing::{debug, error, warn};

#[derive(Clone)]
pub struct ConnectorClient {
    client: Client,
}

impl ConnectorClient {
    pub fn new() -> Self {
        let client = Client::builder()
            .timeout(Duration::from_secs(30))
            .build()
            .expect("Failed to create HTTP client");

        Self { client }
    }

    pub async fn get_manifest(
        &self,
        connector_url: &str,
    ) -> Result<ConnectorManifest, ClientError> {
        let url = format!("{}/manifest", connector_url);
        debug!("Fetching manifest from {}", url);

        let response = self
            .client
            .get(&url)
            .send()
            .await
            .map_err(|e| ClientError::RequestFailed(e.to_string()))?;

        if !response.status().is_success() {
            let status = response.status();
            let body = response.text().await.unwrap_or_default();
            error!("Failed to get manifest: {} - {}", status, body);
            return Err(ClientError::ConnectorError {
                status: status.as_u16(),
                message: body,
            });
        }

        response
            .json()
            .await
            .map_err(|e| ClientError::InvalidResponse(e.to_string()))
    }

    pub async fn trigger_sync(
        &self,
        connector_url: &str,
        request: &SyncRequest,
    ) -> Result<SyncResponse, ClientError> {
        let url = format!("{}/sync", connector_url);
        debug!(
            "Triggering sync at {} for source {}",
            url, request.source.id
        );

        let response = self
            .client
            .post(&url)
            .json(request)
            .send()
            .await
            .map_err(|e| ClientError::RequestFailed(e.to_string()))?;

        if !response.status().is_success() {
            let status = response.status();
            let body = response.text().await.unwrap_or_default();
            error!("Failed to trigger sync: {} - {}", status, body);
            return Err(ClientError::ConnectorError {
                status: status.as_u16(),
                message: body,
            });
        }

        response
            .json()
            .await
            .map_err(|e| ClientError::InvalidResponse(e.to_string()))
    }

    pub async fn cancel_sync(
        &self,
        connector_url: &str,
        sync_run_id: &str,
    ) -> Result<(), ClientError> {
        let url = format!("{}/cancel", connector_url);
        debug!("Cancelling sync {} at {}", sync_run_id, url);

        let response = self
            .client
            .post(&url)
            .json(&serde_json::json!({ "sync_run_id": sync_run_id }))
            .send()
            .await
            .map_err(|e| ClientError::RequestFailed(e.to_string()))?;

        if !response.status().is_success() {
            let status = response.status();
            let body = response.text().await.unwrap_or_default();
            warn!("Failed to cancel sync: {} - {}", status, body);
            return Err(ClientError::ConnectorError {
                status: status.as_u16(),
                message: body,
            });
        }

        Ok(())
    }

    pub async fn execute_action(
        &self,
        connector_url: &str,
        request: &ActionRequest,
    ) -> Result<ActionResponse, ClientError> {
        let url = format!("{}/action", connector_url);
        debug!("Executing action {} at {}", request.action, url);

        let response = self
            .client
            .post(&url)
            .json(request)
            .send()
            .await
            .map_err(|e| ClientError::RequestFailed(e.to_string()))?;

        if !response.status().is_success() {
            let status = response.status();
            let body = response.text().await.unwrap_or_default();
            error!("Failed to execute action: {} - {}", status, body);
            return Err(ClientError::ConnectorError {
                status: status.as_u16(),
                message: body,
            });
        }

        response
            .json()
            .await
            .map_err(|e| ClientError::InvalidResponse(e.to_string()))
    }

    pub async fn health_check(&self, connector_url: &str) -> bool {
        let url = format!("{}/health", connector_url);
        match self.client.get(&url).send().await {
            Ok(response) => response.status().is_success(),
            Err(_) => false,
        }
    }
}

impl Default for ConnectorClient {
    fn default() -> Self {
        Self::new()
    }
}

#[derive(Debug, thiserror::Error)]
pub enum ClientError {
    #[error("Request failed: {0}")]
    RequestFailed(String),

    #[error("Connector returned error: status={status}, message={message}")]
    ConnectorError { status: u16, message: String },

    #[error("Invalid response: {0}")]
    InvalidResponse(String),

    #[error("Connector not found for source type: {0}")]
    ConnectorNotFound(String),
}

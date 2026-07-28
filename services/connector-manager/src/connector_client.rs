use crate::models::{
    ActionRequest, ActionResponse, CancelRequest, ConnectorManifest, OAuthCredentialReadyRequest,
    PromptRequest, ResourceRequest, SkillRequest, SyncRequest, SyncResponse, SyncStatusResponse,
};
use reqwest::Client;
use shared::models::SyncType;
use shared::telemetry::http_client;
use shared::{RateLimiter, RetryableError};
use std::sync::atomic::{AtomicU32, Ordering};
use std::time::Duration;
use tracing::{debug, error, info, warn};

const SYNC_TRIGGER_RETRY_LIMIT: u32 = 3;
const SYNC_TRIGGER_RETRY_RPS: u32 = 1_000;

#[derive(Clone)]
pub struct ConnectorClient {
    client: Client,
    sync_trigger_retry: RateLimiter,
}

impl ConnectorClient {
    pub fn new() -> Self {
        let client = Client::builder()
            .timeout(Duration::from_secs(30))
            .build()
            .expect("Failed to create HTTP client");

        Self {
            client,
            sync_trigger_retry: RateLimiter::new(SYNC_TRIGGER_RETRY_RPS, SYNC_TRIGGER_RETRY_LIMIT),
        }
    }

    pub async fn get_manifest(
        &self,
        connector_url: &str,
    ) -> Result<ConnectorManifest, ClientError> {
        let url = format!("{}/manifest", connector_url);
        debug!("Fetching manifest from {}", url);

        let response = http_client::send_traced("GET", &url, self.client.get(&url))
            .await
            .map_err(|e| ClientError::RequestFailed(e.to_string()))?;

        if !response.status().is_success() {
            let status = response.status();
            error!("Failed to get manifest: status={}", status);
            return Err(ClientError::ConnectorError {
                status: status.as_u16(),
                message: String::new(),
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
            sync_mode = ?request.sync_mode,
            "Triggering sync"
        );

        let response = self.trigger_sync_with_retry(&url, request).await?;

        if !response.status().is_success() {
            let status = response.status();
            if status.as_u16() == 404 && request.sync_mode == SyncType::Realtime {
                debug!("Realtime sync unavailable: status={}", status);
            } else {
                error!("Failed to trigger sync: status={}", status);
            }
            return Err(ClientError::ConnectorError {
                status: status.as_u16(),
                message: String::new(),
            });
        }

        response
            .json()
            .await
            .map_err(|e| ClientError::InvalidResponse(e.to_string()))
    }

    async fn trigger_sync_with_retry(
        &self,
        url: &str,
        request: &SyncRequest,
    ) -> Result<reqwest::Response, ClientError> {
        let attempts = AtomicU32::new(0);
        self.sync_trigger_retry
            .execute_with_retry(|| async {
                let attempt = attempts.fetch_add(1, Ordering::Relaxed) + 1;
                debug!(
                    sync_mode = ?request.sync_mode,
                    attempt,
                    "Sending connector sync trigger"
                );

                let result =
                    http_client::send_traced("POST", url, self.client.post(url).json(request))
                        .await;

                match result {
                    Ok(response) => {
                        debug!(
                            sync_mode = ?request.sync_mode,
                            attempt,
                            status = response.status().as_u16(),
                            "Connector sync trigger returned"
                        );
                        Ok(response)
                    }
                    Err(e) => {
                        warn!(
                            sync_mode = ?request.sync_mode,
                            attempt,
                            "Connector sync trigger request failed"
                        );
                        Err(RetryableError::Transient(e.into()))
                    }
                }
            })
            .await
            .map_err(|e| ClientError::RequestFailed(e.to_string()))
    }

    pub async fn get_sync_status(
        &self,
        connector_url: &str,
        sync_run_id: &str,
    ) -> Result<SyncStatusResponse, ClientError> {
        let url = format!("{}/sync/{}", connector_url, sync_run_id);
        debug!("Probing sync status at {}", url);

        let response = http_client::send_traced(
            "GET",
            &url,
            self.client.get(&url).timeout(Duration::from_secs(5)),
        )
        .await
        .map_err(|e| ClientError::RequestFailed(e.to_string()))?;

        if !response.status().is_success() {
            let status = response.status();
            return Err(ClientError::ConnectorError {
                status: status.as_u16(),
                message: String::new(),
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

        let response = http_client::send_traced(
            "POST",
            &url,
            self.client.post(&url).json(&CancelRequest {
                sync_run_id: sync_run_id.to_string(),
            }),
        )
        .await
        .map_err(|e| ClientError::RequestFailed(e.to_string()))?;

        if !response.status().is_success() {
            let status = response.status();
            warn!("Failed to cancel sync: status={}", status);
            return Err(ClientError::ConnectorError {
                status: status.as_u16(),
                message: String::new(),
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

        let response = http_client::send_traced("POST", &url, self.client.post(&url).json(request))
            .await
            .map_err(|e| ClientError::RequestFailed(e.to_string()))?;

        if !response.status().is_success() {
            let status = response.status();
            error!("Failed to execute action: status={}", status);
            return Err(ClientError::ConnectorError {
                status: status.as_u16(),
                message: String::new(),
            });
        }

        response
            .json()
            .await
            .map_err(|e| ClientError::InvalidResponse(e.to_string()))
    }

    /// Execute an action and return the raw response without parsing.
    /// The connector-manager proxies the full HTTP response (status, headers, body)
    /// back to the caller, regardless of status code.
    ///
    /// Returns `reqwest::Response` (the HTTP response from the connector service)
    /// rather than `axum::response::Response` (the server-side response type).
    /// The caller converts this into an axum response for the end client.
    pub async fn execute_action_raw(
        &self,
        connector_url: &str,
        request: &ActionRequest,
    ) -> Result<reqwest::Response, ClientError> {
        let url = format!("{}/action", connector_url);
        debug!("Executing action (raw) {} at {}", request.action, url);

        let response = http_client::send_traced("POST", &url, self.client.post(&url).json(request))
            .await
            .map_err(|e| ClientError::RequestFailed(e.to_string()))?;

        // Return the raw response regardless of status code so the handler
        // can proxy status, headers, and body verbatim.
        Ok(response)
    }

    /// Notify a connector that a new OAuth credential has been stored.
    /// The connector may use the credential to refresh its authenticated MCP
    /// catalog and return an updated manifest.
    pub async fn oauth_credential_ready(
        &self,
        connector_url: &str,
        request: &OAuthCredentialReadyRequest,
    ) -> Result<Option<ConnectorManifest>, ClientError> {
        let url = format!("{}/oauth/credential-ready", connector_url);
        debug!("Notifying connector of OAuth credential-ready at {}", url);

        let response = http_client::send_traced("POST", &url, self.client.post(&url).json(request))
            .await
            .map_err(|e| ClientError::RequestFailed(e.to_string()))?;

        if !response.status().is_success() {
            // 404 means the connector doesn't implement the endpoint (old SDK).
            // 4xx/5xx means delivery failed; don't propagate, just return None.
            info!(
                "oauth_credential_ready returned {}: connector may not support the endpoint",
                response.status()
            );
            return Err(ClientError::ConnectorError {
                status: response.status().as_u16(),
                message: String::new(),
            });
        }

        // If connector returns no body (204) or empty, no manifest update.
        let body = response.text().await.unwrap_or_default();
        if body.is_empty() {
            return Ok(None);
        }
        match serde_json::from_str::<ConnectorManifest>(&body) {
            Ok(manifest) => Ok(Some(manifest)),
            Err(_) => Ok(None),
        }
    }

    pub async fn read_resource(
        &self,
        connector_url: &str,
        request: &ResourceRequest,
    ) -> Result<serde_json::Value, ClientError> {
        let url = format!("{}/resource", connector_url);
        debug!("Reading resource {} at {}", request.uri, url);

        let response = http_client::send_traced("POST", &url, self.client.post(&url).json(request))
            .await
            .map_err(|e| ClientError::RequestFailed(e.to_string()))?;

        if !response.status().is_success() {
            let status = response.status();
            error!("Failed to read resource: status={}", status);
            return Err(ClientError::ConnectorError {
                status: status.as_u16(),
                message: String::new(),
            });
        }

        response
            .json()
            .await
            .map_err(|e| ClientError::InvalidResponse(e.to_string()))
    }

    pub async fn get_prompt(
        &self,
        connector_url: &str,
        request: &PromptRequest,
    ) -> Result<serde_json::Value, ClientError> {
        let url = format!("{}/prompt", connector_url);
        debug!("Getting prompt {} at {}", request.name, url);

        let response = http_client::send_traced("POST", &url, self.client.post(&url).json(request))
            .await
            .map_err(|e| ClientError::RequestFailed(e.to_string()))?;

        if !response.status().is_success() {
            let status = response.status();
            error!("Failed to get prompt: status={}", status);
            return Err(ClientError::ConnectorError {
                status: status.as_u16(),
                message: String::new(),
            });
        }

        response
            .json()
            .await
            .map_err(|e| ClientError::InvalidResponse(e.to_string()))
    }

    pub async fn get_skill(
        &self,
        connector_url: &str,
        request: &SkillRequest,
    ) -> Result<serde_json::Value, ClientError> {
        let url = format!("{}/skill", connector_url);
        debug!("Getting skill {} at {}", request.skill_id, url);

        let response = http_client::send_traced("POST", &url, self.client.post(&url).json(request))
            .await
            .map_err(|e| ClientError::RequestFailed(e.to_string()))?;

        if !response.status().is_success() {
            let status = response.status();
            error!("Failed to get skill: status={}", status);
            return Err(ClientError::ConnectorError {
                status: status.as_u16(),
                message: String::new(),
            });
        }

        response
            .json()
            .await
            .map_err(|e| ClientError::InvalidResponse(e.to_string()))
    }

    pub async fn health_check(&self, connector_url: &str) -> bool {
        let url = format!("{}/health", connector_url);
        match http_client::send_traced("GET", &url, self.client.get(&url)).await {
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

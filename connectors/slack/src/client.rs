use anyhow::{anyhow, Result};
use omni_connector_sdk::{RateLimiter, RetryableError};
use reqwest::Client;
use serde::Deserialize;
use std::time::Duration;
use tracing::{debug, info, warn};

use crate::models::{
    ConversationInfoResponse, ConversationsHistoryResponse, ConversationsListResponse,
    ConversationsMembersResponse, SlackFile, UsersListResponse,
};

const DEFAULT_SLACK_API_BASE: &str = "https://slack.com/api";

/// Subset of every Slack response we use to detect API-level errors before
/// attempting to deserialize into the typed success shape (which would fail
/// with a misleading `missing field` for the absent success-only fields).
#[derive(Deserialize)]
struct SlackErrorEnvelope {
    ok: bool,
    error: Option<String>,
    needed: Option<String>,
    provided: Option<String>,
}

impl SlackErrorEnvelope {
    fn format_error(&self) -> String {
        let base = self.error.as_deref().unwrap_or("unknown");
        match (self.needed.as_deref(), self.provided.as_deref()) {
            (Some(needed), Some(provided)) => {
                format!("{base} (needed: {needed}; provided: {provided})")
            }
            (Some(needed), None) => format!("{base} (needed: {needed})"),
            _ => base.to_string(),
        }
    }
}

pub struct SlackClient {
    client: Client,
    rate_limiter: RateLimiter,
    base_url: String,
}

impl SlackClient {
    pub fn new() -> Self {
        Self::with_base_url(DEFAULT_SLACK_API_BASE.to_string())
    }

    pub fn with_base_url(base_url: String) -> Self {
        Self {
            client: Client::new(),
            // Slack Tier 3 allows ~50 req/min; 1 req/sec keeps us safely under.
            rate_limiter: RateLimiter::new(1, 5),
            base_url,
        }
    }

    fn extract_retry_after(response: &reqwest::Response) -> Duration {
        response
            .headers()
            .get("retry-after")
            .and_then(|v| v.to_str().ok())
            .and_then(|v| v.parse::<u64>().ok())
            .map(Duration::from_secs)
            .unwrap_or(Duration::from_secs(1))
    }

    async fn make_request<T>(&self, url: &str, token: &str) -> Result<T>
    where
        T: serde::de::DeserializeOwned,
    {
        debug!("Making request to: {}", url);

        self.rate_limiter
            .execute_with_retry(|| async {
                let response = self
                    .client
                    .get(url)
                    .header("Authorization", format!("Bearer {}", token))
                    .header("Content-Type", "application/json")
                    .send()
                    .await
                    .map_err(|e| RetryableError::Transient(e.into()))?;

                if response.status() == reqwest::StatusCode::TOO_MANY_REQUESTS {
                    return Err(RetryableError::RateLimited {
                        retry_after: Self::extract_retry_after(&response),
                        message: format!("Slack API rate limited: {}", url),
                    });
                }

                if !response.status().is_success() {
                    let error_text = response.text().await.unwrap_or_default();
                    return Err(RetryableError::Permanent(anyhow!(
                        "API request failed: {}",
                        error_text
                    )));
                }

                let response_text = response
                    .text()
                    .await
                    .map_err(|e| RetryableError::Transient(e.into()))?;
                debug!("Response: {}", response_text);

                // Slack API errors come back as 200s with `{ok:false, error:"..."}`
                // and omit the success-shape fields the typed responses require.
                // Surface the `error` cleanly here so callers see e.g.
                // "Slack API error: missing_scope (needed: mpim:read)" instead
                // of a confusing `missing field 'channels'` from serde.
                if let Ok(envelope) = serde_json::from_str::<SlackErrorEnvelope>(&response_text) {
                    if !envelope.ok {
                        return Err(RetryableError::Permanent(anyhow!(
                            "Slack API error: {}",
                            envelope.format_error()
                        )));
                    }
                }

                serde_json::from_str(&response_text).map_err(|e| {
                    RetryableError::Permanent(anyhow!("Failed to parse response: {}", e))
                })
            })
            .await
    }

    pub async fn list_conversations(
        &self,
        token: &str,
        cursor: Option<&str>,
    ) -> Result<ConversationsListResponse> {
        let mut url = format!(
            "{}/conversations.list?types=public_channel,private_channel,mpim,im&limit=200",
            self.base_url
        );

        if let Some(cursor) = cursor {
            url.push_str(&format!("&cursor={}", cursor));
        }

        let response: ConversationsListResponse = self.make_request(&url, token).await?;

        if !response.ok {
            return Err(anyhow!(
                "conversations.list failed: {}",
                response.error.unwrap_or("Unknown error".to_string())
            ));
        }

        info!("Found {} channels", response.channels.len());
        Ok(response)
    }

    pub async fn get_conversation_history(
        &self,
        token: &str,
        channel_id: &str,
        cursor: Option<&str>,
        oldest: Option<&str>,
        latest: Option<&str>,
    ) -> Result<ConversationsHistoryResponse> {
        let mut url = format!(
            "{}/conversations.history?channel={}&limit=200",
            self.base_url, channel_id
        );

        if let Some(cursor) = cursor {
            url.push_str(&format!("&cursor={}", cursor));
        }
        if let Some(oldest) = oldest {
            url.push_str(&format!("&oldest={}", oldest));
        }
        if let Some(latest) = latest {
            url.push_str(&format!("&latest={}", latest));
        }

        let response: ConversationsHistoryResponse = self.make_request(&url, token).await?;

        if !response.ok {
            return Err(anyhow!(
                "conversations.history failed: {}",
                response.error.unwrap_or("Unknown error".to_string())
            ));
        }

        debug!(
            "Retrieved {} messages from channel {}",
            response.messages.len(),
            channel_id
        );
        Ok(response)
    }

    pub async fn get_thread_replies(
        &self,
        token: &str,
        channel_id: &str,
        thread_ts: &str,
        cursor: Option<&str>,
    ) -> Result<ConversationsHistoryResponse> {
        let mut url = format!(
            "{}/conversations.replies?channel={}&ts={}&limit=200",
            self.base_url, channel_id, thread_ts
        );

        if let Some(cursor) = cursor {
            url.push_str(&format!("&cursor={}", cursor));
        }

        let response: ConversationsHistoryResponse = self.make_request(&url, token).await?;

        if !response.ok {
            return Err(anyhow!(
                "conversations.replies failed: {}",
                response.error.unwrap_or("Unknown error".to_string())
            ));
        }

        debug!(
            "Retrieved {} thread replies for ts {}",
            response.messages.len(),
            thread_ts
        );
        Ok(response)
    }

    pub async fn join_conversation(&self, token: &str, channel_id: &str) -> Result<()> {
        let url = format!("{}/conversations.join", self.base_url);
        let payload = serde_json::json!({ "channel": channel_id });

        self.rate_limiter
            .execute_with_retry(|| async {
                let response = self
                    .client
                    .post(&url)
                    .header("Authorization", format!("Bearer {}", token))
                    .header("Content-Type", "application/json")
                    .json(&payload)
                    .send()
                    .await
                    .map_err(|e| RetryableError::Transient(e.into()))?;

                if response.status() == reqwest::StatusCode::TOO_MANY_REQUESTS {
                    return Err(RetryableError::RateLimited {
                        retry_after: Self::extract_retry_after(&response),
                        message: "Slack API rate limited: conversations.join".to_string(),
                    });
                }

                let body: serde_json::Value = response
                    .json()
                    .await
                    .map_err(|e| RetryableError::Transient(e.into()))?;
                if body.get("ok") == Some(&serde_json::Value::Bool(true)) {
                    Ok(())
                } else {
                    let err = body["error"].as_str().unwrap_or("Unknown error");
                    Err(RetryableError::Permanent(anyhow!(
                        "conversations.join failed: {}",
                        err
                    )))
                }
            })
            .await
    }

    pub async fn list_users(&self, token: &str, cursor: Option<&str>) -> Result<UsersListResponse> {
        let mut url = format!("{}/users.list?limit=200", self.base_url);

        if let Some(cursor) = cursor {
            url.push_str(&format!("&cursor={}", cursor));
        }

        let response: UsersListResponse = self.make_request(&url, token).await?;

        if !response.ok {
            return Err(anyhow!(
                "users.list failed: {}",
                response.error.unwrap_or("Unknown error".to_string())
            ));
        }

        info!("Found {} users", response.members.len());
        Ok(response)
    }

    pub async fn get_conversation_info(
        &self,
        token: &str,
        channel_id: &str,
    ) -> Result<crate::models::SlackChannel> {
        let url = format!(
            "{}/conversations.info?channel={}",
            self.base_url, channel_id
        );

        let response: ConversationInfoResponse = self.make_request(&url, token).await?;

        if !response.ok {
            return Err(anyhow!(
                "conversations.info failed: {}",
                response.error.unwrap_or("Unknown error".to_string())
            ));
        }

        Ok(response.channel)
    }

    pub async fn get_conversation_members(
        &self,
        token: &str,
        channel_id: &str,
        cursor: Option<&str>,
    ) -> Result<ConversationsMembersResponse> {
        let mut url = format!(
            "{}/conversations.members?channel={}&limit=200",
            self.base_url, channel_id
        );

        if let Some(cursor) = cursor {
            url.push_str(&format!("&cursor={}", cursor));
        }

        let response: ConversationsMembersResponse = self.make_request(&url, token).await?;

        if !response.ok {
            return Err(anyhow!(
                "conversations.members failed: {}",
                response.error.unwrap_or("Unknown error".to_string())
            ));
        }

        debug!(
            "Retrieved {} members from channel {}",
            response.members.len(),
            channel_id
        );
        Ok(response)
    }

    /// Download a file's bytes via its `url_private_download`. Returns
    /// `Some((bytes, content_type))` on success, `None` if the file has no
    /// download URL or the download returned a non-success HTTP status (e.g.
    /// the file was deleted, or the bot lost access). The caller decides what
    /// to do with the bytes — typically `ctx.extract_and_store_content` to
    /// route through the connector-manager's extractor (Docling for binary,
    /// utf8 decode for text).
    pub async fn download_file(
        &self,
        token: &str,
        file: &SlackFile,
    ) -> Result<Option<(Vec<u8>, String)>> {
        let Some(download_url) = &file.url_private_download else {
            return Ok(None);
        };

        debug!("Downloading file: {} ({})", file.name, file.id);

        self.rate_limiter
            .execute_with_retry(|| async {
                let response = self
                    .client
                    .get(download_url)
                    .header("Authorization", format!("Bearer {}", token))
                    .send()
                    .await
                    .map_err(|e| RetryableError::Transient(e.into()))?;

                if response.status() == reqwest::StatusCode::TOO_MANY_REQUESTS {
                    return Err(RetryableError::RateLimited {
                        retry_after: Self::extract_retry_after(&response),
                        message: format!("Slack API rate limited downloading file: {}", file.name),
                    });
                }

                if !response.status().is_success() {
                    warn!(
                        "Failed to download file {}: HTTP {}",
                        file.name,
                        response.status()
                    );
                    return Ok(None);
                }

                let content_type = response
                    .headers()
                    .get("content-type")
                    .and_then(|ct| ct.to_str().ok())
                    .unwrap_or("application/octet-stream")
                    .to_string();

                let bytes = response
                    .bytes()
                    .await
                    .map_err(|e| RetryableError::Transient(e.into()))?;

                Ok(Some((bytes.to_vec(), content_type)))
            })
            .await
    }
}

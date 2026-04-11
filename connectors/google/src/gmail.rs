use anyhow::{anyhow, Context, Result};
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use base64::Engine;
use reqwest::Client;
use serde::Deserialize;
use std::collections::HashMap;
use std::sync::{Arc, RwLock};
use std::time::Duration;
use tracing::{debug, warn};

use crate::auth::{execute_with_auth_retry, is_auth_error, ApiResult, GoogleAuth};
use shared::RateLimiter;

const GMAIL_API_BASE: &str = "https://gmail.googleapis.com/gmail/v1";

#[derive(Clone)]
pub struct GmailClient {
    client: Client,
    rate_limiter: Arc<RateLimiter>,
    user_rate_limiters: Arc<RwLock<HashMap<String, Arc<RateLimiter>>>>,
}

impl GmailClient {
    pub fn new() -> Self {
        let client = Client::builder()
            .timeout(Duration::from_secs(60))
            .connect_timeout(Duration::from_secs(10))
            .build()
            .expect("Failed to build HTTP client");

        let rate_limiter = Arc::new(RateLimiter::new(200, 5));
        let user_rate_limiters = Arc::new(RwLock::new(HashMap::new()));
        Self {
            client,
            rate_limiter,
            user_rate_limiters,
        }
    }

    pub fn with_rate_limiter(rate_limiter: Arc<RateLimiter>) -> Self {
        let client = Client::builder()
            .timeout(Duration::from_secs(60))
            .connect_timeout(Duration::from_secs(10))
            .pool_max_idle_per_host(10)
            .pool_idle_timeout(Duration::from_secs(90))
            .tcp_keepalive(Duration::from_secs(60))
            .build()
            .expect("Failed to build HTTP client");

        let user_rate_limiters = Arc::new(RwLock::new(HashMap::new()));
        Self {
            client,
            rate_limiter,
            user_rate_limiters,
        }
    }

    fn get_or_create_user_rate_limiter(&self, user_email: &str) -> Result<Arc<RateLimiter>> {
        {
            let rate_limiters = self.user_rate_limiters.read().map_err(|e| {
                anyhow!("Failed to acquire read lock on user rate limiters: {:?}", e)
            })?;
            if let Some(limiter) = rate_limiters.get(user_email) {
                return Ok(Arc::clone(limiter));
            }
        }

        let mut rate_limiters = self.user_rate_limiters.write().map_err(|e| {
            anyhow!(
                "Failed to acquire write lock on user rate limiters: {:?}",
                e
            )
        })?;

        let limiter = rate_limiters
            .entry(user_email.to_string())
            .or_insert_with(|| Arc::new(RateLimiter::new(25, 5))) // 1500 req/min for each user, 5 retry attempts
            .clone();

        Ok(limiter)
    }

    fn delete_user_rate_limiter(&self, user_email: &str) -> Result<()> {
        let mut rate_limiters = self.user_rate_limiters.write().map_err(|e| {
            anyhow!(
                "Failed to acquire write lock on user rate limiters: {:?}",
                e
            )
        })?;
        rate_limiters.remove(user_email);
        Ok(())
    }

    pub async fn list_messages(
        &self,
        auth: &GoogleAuth,
        user_email: &str,
        query: Option<&str>,
        max_results: Option<u32>,
        page_token: Option<&str>,
        created_after: Option<&str>,
    ) -> Result<MessagesListResponse> {
        let base_query = query.map(|s| s.to_string());
        let page_token = page_token.map(|s| s.to_string());
        let created_after = created_after.map(|s| s.to_string());

        let rate_limiter = self.get_or_create_user_rate_limiter(user_email)?;
        execute_with_auth_retry(auth, user_email, rate_limiter.clone(), |token| {
            let base_query = base_query.clone();
            let page_token = page_token.clone();
            let created_after = created_after.clone();
            async move {
                let url = format!("{}/users/{}/messages", GMAIL_API_BASE, user_email);

                let mut params = vec![("maxResults", max_results.unwrap_or(100).to_string())];

                // Build the complete query with date filter
                let mut query_parts = Vec::new();
                if let Some(ref q) = base_query {
                    query_parts.push(q.clone());
                }
                if let Some(ref date) = created_after {
                    query_parts.push(format!("after:{}", date));
                }

                if !query_parts.is_empty() {
                    let final_query = query_parts.join(" ");
                    params.push(("q", final_query));
                }

                if let Some(ref page_token) = page_token {
                    params.push(("pageToken", page_token.clone()));
                }

                let response = self
                    .client
                    .get(&url)
                    .bearer_auth(&token)
                    .query(&params)
                    .send()
                    .await?;

                let status = response.status();
                if is_auth_error(status) {
                    return Ok(ApiResult::AuthError);
                } else if !status.is_success() {
                    let error_text = response.text().await?;
                    return Ok(ApiResult::OtherError(anyhow!(
                        "Failed to list messages: HTTP {} - {}",
                        status,
                        error_text
                    )));
                }

                debug!("Gmail API list messages response status: {}", status);
                let response_text = response.text().await?;
                debug!("Gmail API raw response: {}", response_text);

                let parsed_response = serde_json::from_str(&response_text).map_err(|e| {
                    anyhow!(
                        "Failed to parse Gmail API response: {}. Raw response: {}",
                        e,
                        response_text
                    )
                })?;

                Ok(ApiResult::Success(parsed_response))
            }
        })
        .await
    }

    pub async fn list_threads(
        &self,
        auth: &GoogleAuth,
        user_email: &str,
        query: Option<&str>,
        max_results: Option<u32>,
        page_token: Option<&str>,
        created_after: Option<&str>,
    ) -> Result<ThreadsListResponse> {
        let base_query = query.map(|s| s.to_string());
        let page_token = page_token.map(|s| s.to_string());
        let created_after = created_after.map(|s| s.to_string());

        let rate_limiter = self.get_or_create_user_rate_limiter(user_email)?;
        execute_with_auth_retry(auth, user_email, rate_limiter.clone(), |token| {
            let base_query = base_query.clone();
            let page_token = page_token.clone();
            let created_after = created_after.clone();
            async move {
                let url = format!("{}/users/{}/threads", GMAIL_API_BASE, user_email);

                let mut params = vec![("maxResults", max_results.unwrap_or(100).to_string())];

                // Build the complete query with date filter
                let mut query_parts = Vec::new();
                if let Some(ref q) = base_query {
                    query_parts.push(q.clone());
                }
                if let Some(ref date) = created_after {
                    query_parts.push(format!("after:{}", date));
                }

                if !query_parts.is_empty() {
                    let final_query = query_parts.join(" ");
                    params.push(("q", final_query));
                }

                if let Some(ref page_token) = page_token {
                    params.push(("pageToken", page_token.clone()));
                }

                let response = self
                    .client
                    .get(&url)
                    .bearer_auth(&token)
                    .query(&params)
                    .send()
                    .await?;

                let status = response.status();
                if is_auth_error(status) {
                    return Ok(ApiResult::AuthError);
                } else if !status.is_success() {
                    let error_text = response.text().await?;
                    return Ok(ApiResult::OtherError(anyhow!(
                        "Failed to list threads: HTTP {} - {}",
                        status,
                        error_text
                    )));
                }

                debug!("Gmail API list threads response status: {}", status);
                let response_text = response.text().await?;
                debug!("Gmail API raw response: {}", response_text);

                let parsed_response = serde_json::from_str(&response_text).map_err(|e| {
                    anyhow!(
                        "Failed to parse Gmail threads API response: {}. Raw response: {}",
                        e,
                        response_text
                    )
                })?;

                Ok(ApiResult::Success(parsed_response))
            }
        })
        .await
    }

    pub async fn get_message(
        &self,
        auth: &GoogleAuth,
        user_email: &str,
        message_id: &str,
        format: MessageFormat,
    ) -> Result<GmailMessage> {
        let message_id = message_id.to_string();

        let rate_limiter = self.get_or_create_user_rate_limiter(user_email)?;
        execute_with_auth_retry(auth, user_email, rate_limiter.clone(), |token| {
            let message_id = message_id.clone();
            async move {
                let url = format!(
                    "{}/users/{}/messages/{}",
                    GMAIL_API_BASE, user_email, message_id
                );

                let format_str = match format {
                    MessageFormat::Full => "full",
                    MessageFormat::Metadata => "metadata",
                    MessageFormat::Minimal => "minimal",
                    MessageFormat::Raw => "raw",
                };

                let params = vec![("format", format_str)];

                let response = self
                    .client
                    .get(&url)
                    .bearer_auth(&token)
                    .query(&params)
                    .send()
                    .await
                    .with_context(|| {
                        format!(
                            "Failed to send request to Gmail API for message {}",
                            message_id
                        )
                    })?;

                let status = response.status();
                if is_auth_error(status) {
                    return Ok(ApiResult::AuthError);
                } else if !status.is_success() {
                    let error_text = response.text().await?;
                    return Ok(ApiResult::OtherError(anyhow!(
                        "Gmail API returned error for message {}: HTTP {} - {}",
                        message_id,
                        status,
                        error_text
                    )));
                }

                debug!("Gmail API get message response status: {}", status);
                let response_text = response
                    .text()
                    .await
                    .context("Failed to read response body from Gmail API")?;

                let message: GmailMessage =
                    serde_json::from_str(&response_text).with_context(|| {
                        format!(
                            "Failed to parse Gmail API response for message {}. Raw response: {}",
                            message_id, response_text
                        )
                    })?;

                Ok(ApiResult::Success(message))
            }
        })
        .await
    }

    pub async fn get_thread(
        &self,
        auth: &GoogleAuth,
        user_email: &str,
        thread_id: &str,
        format: MessageFormat,
    ) -> Result<GmailThreadResponse> {
        let thread_id = thread_id.to_string();

        let rate_limiter = self.get_or_create_user_rate_limiter(user_email)?;
        execute_with_auth_retry(auth, user_email, rate_limiter.clone(), |token| {
            let thread_id = thread_id.clone();
            async move {
                let url = format!(
                    "{}/users/{}/threads/{}",
                    GMAIL_API_BASE, user_email, thread_id
                );

                let format_str = match format {
                    MessageFormat::Full => "full",
                    MessageFormat::Metadata => "metadata",
                    MessageFormat::Minimal => "minimal",
                    MessageFormat::Raw => "raw",
                };

                let params = vec![("format", format_str)];

                let response = self
                    .client
                    .get(&url)
                    .bearer_auth(&token)
                    .query(&params)
                    .send()
                    .await
                    .with_context(|| {
                        format!(
                            "Failed to send request to Gmail API for thread {}",
                            thread_id
                        )
                    })?;

                let status = response.status();
                if is_auth_error(status) {
                    return Ok(ApiResult::AuthError);
                } else if !status.is_success() {
                    let error_text = response.text().await?;
                    return Ok(ApiResult::OtherError(anyhow!(
                        "Gmail API returned error for thread {}: HTTP {} - {}",
                        thread_id,
                        status,
                        error_text
                    )));
                }

                debug!("Gmail API get thread response status: {}", status);
                let response_text = response
                    .text()
                    .await
                    .context("Failed to read response body from Gmail API")?;

                let thread: GmailThreadResponse = serde_json::from_str(&response_text)
                    .with_context(|| {
                        format!(
                            "Failed to parse Gmail API response for thread {}. Raw response: {}",
                            thread_id, response_text
                        )
                    })?;

                Ok(ApiResult::Success(thread))
            }
        })
        .await
    }

    pub async fn batch_get_threads(
        &self,
        auth: &GoogleAuth,
        user_email: &str,
        thread_ids: &[String],
        format: MessageFormat,
    ) -> Result<Vec<BatchThreadResult>> {
        if thread_ids.is_empty() {
            return Ok(Vec::new());
        }

        // Gmail batch API allows up to 100 requests per batch
        let chunk_size = std::cmp::min(100, thread_ids.len());
        let thread_chunk = &thread_ids[..chunk_size];

        let rate_limiter = self.get_or_create_user_rate_limiter(user_email)?;
        execute_with_auth_retry(auth, user_email, rate_limiter.clone(), |token| {
            let thread_chunk = thread_chunk.to_vec();
            async move {
                let batch_url = "https://www.googleapis.com/batch/gmail/v1";

                let format_str = match format {
                    MessageFormat::Full => "full",
                    MessageFormat::Metadata => "metadata",
                    MessageFormat::Minimal => "minimal",
                    MessageFormat::Raw => "raw",
                };

                // Construct multipart batch request body
                let boundary = "batch_boundary_123456789";
                let mut batch_body = String::new();

                for (i, thread_id) in thread_chunk.iter().enumerate() {
                    batch_body.push_str(&format!("--{}\r\n", boundary));
                    batch_body.push_str("Content-Type: application/http\r\n");
                    batch_body.push_str(&format!("Content-ID: <item{}>\r\n\r\n", i + 1));

                    let thread_url = format!(
                        "GET /gmail/v1/users/{}/threads/{}?format={} HTTP/1.1\r\n",
                        user_email, thread_id, format_str
                    );
                    batch_body.push_str(&thread_url);
                    batch_body.push_str("Host: gmail.googleapis.com\r\n\r\n");
                }

                batch_body.push_str(&format!("--{}--\r\n", boundary));

                let response = self
                    .client
                    .post(batch_url)
                    .bearer_auth(&token)
                    .header(
                        "Content-Type",
                        format!("multipart/mixed; boundary={}", boundary),
                    )
                    .body(batch_body)
                    .send()
                    .await
                    .context("Failed to send batch request to Gmail API")?;

                let status = response.status();
                if is_auth_error(status) {
                    return Ok(ApiResult::AuthError);
                } else if !status.is_success() {
                    let error_text = response.text().await?;
                    return Ok(ApiResult::OtherError(anyhow!(
                        "Gmail batch API returned error: HTTP {} - {}",
                        status,
                        error_text
                    )));
                }

                let response_text = response
                    .text()
                    .await
                    .context("Failed to read batch response body from Gmail API")?;

                // Parse multipart response
                let results = self.parse_batch_response(&response_text, &thread_chunk)?;

                Ok(ApiResult::Success(results))
            }
        })
        .await
    }

    fn parse_batch_response(
        &self,
        response_text: &str,
        thread_ids: &[String],
    ) -> Result<Vec<BatchThreadResult>> {
        let mut results = Vec::with_capacity(thread_ids.len());

        // Split response by boundary markers
        let parts: Vec<&str> = response_text
            .split("--batch_")
            .filter(|part| !part.trim().is_empty() && !part.starts_with('-'))
            .collect();

        for (i, part) in parts.iter().enumerate() {
            if i >= thread_ids.len() {
                break;
            }

            // Extract JSON from the HTTP response part
            if let Some(json_start) = part.find('{') {
                let json_part = &part[json_start..];
                if let Some(json_end) = json_part.rfind('}') {
                    let json_str = &json_part[..=json_end];

                    // Check if response is an API error (not a thread)
                    if let Ok(error_obj) = serde_json::from_str::<serde_json::Value>(json_str) {
                        if error_obj.get("error").is_some() {
                            let code = error_obj["error"]["code"].as_u64().unwrap_or(0);
                            let msg = error_obj["error"]["message"].as_str().unwrap_or("unknown");

                            if code == 429 {
                                debug!("Rate limited on thread {}: {}", thread_ids[i], msg);
                                results.push(BatchThreadResult::RateLimited);
                            } else {
                                debug!(
                                    "Gmail API error for thread {}: {} - {}",
                                    thread_ids[i], code, msg
                                );
                                results.push(BatchThreadResult::Failed(anyhow!(
                                    "Gmail API error for thread {}: {} - {}",
                                    thread_ids[i], code, msg
                                )));
                            }
                            continue;
                        }
                    }

                    match serde_json::from_str::<GmailThreadResponse>(json_str) {
                        Ok(thread) => results.push(BatchThreadResult::Success(thread)),
                        Err(e) => {
                            debug!(
                                "Failed to parse thread {} — first 200 chars: {}",
                                thread_ids[i],
                                &json_str[..json_str.len().min(200)]
                            );
                            results.push(BatchThreadResult::Failed(anyhow!(
                                "Failed to parse thread {} response: {}",
                                thread_ids[i],
                                e
                            )));
                        }
                    }
                } else {
                    results.push(BatchThreadResult::Failed(anyhow!(
                        "Invalid JSON response for thread {}",
                        thread_ids[i]
                    )));
                }
            } else {
                if part.contains("HTTP/1.1 429") {
                    results.push(BatchThreadResult::RateLimited);
                } else if part.contains("HTTP/1.1 4") || part.contains("HTTP/1.1 5") {
                    results.push(BatchThreadResult::Failed(anyhow!(
                        "HTTP error for thread {}",
                        thread_ids[i],
                    )));
                } else {
                    results.push(BatchThreadResult::Failed(anyhow!(
                        "No JSON found in response for thread {}",
                        thread_ids[i]
                    )));
                }
            }
        }

        // Threads missing from batch response — treat as rate-limited so they get retried
        while results.len() < thread_ids.len() {
            let missing_idx = results.len();
            debug!(
                "No response in batch for thread {} — marking for retry",
                thread_ids[missing_idx]
            );
            results.push(BatchThreadResult::RateLimited);
        }

        Ok(results)
    }

    pub async fn list_history(
        &self,
        auth: &GoogleAuth,
        user_email: &str,
        start_history_id: &str,
        max_results: Option<u32>,
        page_token: Option<&str>,
    ) -> Result<HistoryListResponse> {
        let start_history_id = start_history_id.to_string();
        let page_token = page_token.map(|s| s.to_string());

        let rate_limiter = self.get_or_create_user_rate_limiter(user_email)?;
        execute_with_auth_retry(auth, user_email, rate_limiter.clone(), |token| {
            let start_history_id = start_history_id.clone();
            let page_token = page_token.clone();
            async move {
                let url = format!("{}/users/{}/history", GMAIL_API_BASE, user_email);

                let mut params = vec![
                    ("startHistoryId", start_history_id),
                    ("maxResults", max_results.unwrap_or(100).to_string()),
                ];

                if let Some(ref page_token) = page_token {
                    params.push(("pageToken", page_token.clone()));
                }

                let response = self
                    .client
                    .get(&url)
                    .bearer_auth(&token)
                    .query(&params)
                    .send()
                    .await?;

                let status = response.status();
                if is_auth_error(status) {
                    return Ok(ApiResult::AuthError);
                } else if !status.is_success() {
                    let error_text = response.text().await?;
                    return Ok(ApiResult::OtherError(anyhow!(
                        "Failed to list history: HTTP {} - {}",
                        status,
                        error_text
                    )));
                }

                debug!("Gmail API list history response status: {}", status);
                let response_text = response.text().await?;

                let parsed_response = serde_json::from_str(&response_text).map_err(|e| {
                    anyhow!(
                        "Failed to parse Gmail history API response: {}. Raw response: {}",
                        e,
                        response_text
                    )
                })?;

                Ok(ApiResult::Success(parsed_response))
            }
        })
        .await
    }

    pub async fn get_profile(&self, auth: &GoogleAuth, user_email: &str) -> Result<GmailProfile> {
        let rate_limiter = self.get_or_create_user_rate_limiter(user_email)?;
        execute_with_auth_retry(auth, user_email, rate_limiter.clone(), |token| async move {
            let url = format!("{}/users/{}/profile", GMAIL_API_BASE, user_email);

            let response = self.client.get(&url).bearer_auth(&token).send().await?;

            let status = response.status();
            if is_auth_error(status) {
                return Ok(ApiResult::AuthError);
            } else if !status.is_success() {
                let error_text = response.text().await?;
                return Ok(ApiResult::OtherError(anyhow!(
                    "Failed to get profile: HTTP {} - {}",
                    status,
                    error_text
                )));
            }

            let response_text = response.text().await?;
            let profile: GmailProfile = serde_json::from_str(&response_text)?;

            Ok(ApiResult::Success(profile))
        })
        .await
    }

    pub fn extract_message_content(&self, message: &GmailMessage) -> Result<String> {
        if let Some(ref payload) = message.payload {
            let mut plain_parts: Vec<String> = Vec::new();
            let mut html_parts: Vec<String> = Vec::new();
            Self::collect_text_parts(payload, &mut plain_parts, &mut html_parts);

            let body = if !plain_parts.is_empty() {
                plain_parts.join("\n\n")
            } else if !html_parts.is_empty() {
                let combined = html_parts.join("\n\n");
                html_to_text(&combined)
            } else {
                String::new()
            };

            Ok(body)
        } else {
            Ok(String::new())
        }
    }

    /// Download and extract text from all supported attachments in a message.
    /// Returns structured attachment data for separate document indexing.
    /// Extraction is delegated to the connector manager (supports Docling).
    pub async fn extract_attachments(
        &self,
        message: &GmailMessage,
        auth: &GoogleAuth,
        user_email: &str,
        sdk_client: &shared::SdkClient,
        sync_run_id: &str,
    ) -> Vec<ExtractedAttachment> {
        let mut results = Vec::new();

        let Some(ref payload) = message.payload else {
            return results;
        };

        let mut attachment_parts: Vec<AttachmentInfo> = Vec::new();
        Self::collect_attachment_parts(payload, &mut attachment_parts);

        for att in attachment_parts {
            match self
                .download_attachment(auth, user_email, &message.id, &att.attachment_id)
                .await
            {
                Ok(data) => {
                    let size = data.len() as u64;
                    let extracted_text = sdk_client
                        .extract_text(
                            sync_run_id,
                            data,
                            &att.mime_type,
                            Some(&att.filename),
                        )
                        .await
                        .unwrap_or_default();

                    results.push(ExtractedAttachment {
                        message_id: message.id.clone(),
                        attachment_id: att.attachment_id,
                        filename: att.filename,
                        mime_type: att.mime_type,
                        size,
                        extracted_text,
                    });
                }
                Err(e) => {
                    debug!(
                        "Failed to download attachment {} ({}): {}",
                        att.filename, att.attachment_id, e
                    );
                }
            }
        }

        results
    }

    /// Recursively collect text/plain and text/html parts separately,
    /// skipping parts that are file attachments.
    fn collect_text_parts(part: &MessagePart, plain: &mut Vec<String>, html: &mut Vec<String>) {
        // Skip file attachments
        if is_file_attachment(part) {
            return;
        }

        if let Some(ref body) = part.body {
            if let Some(ref data) = body.data {
                if let Some(ref mime_type) = part.mime_type {
                    if let Ok(decoded) = URL_SAFE_NO_PAD.decode(data) {
                        if let Ok(text) = String::from_utf8(decoded) {
                            if !text.trim().is_empty() {
                                if mime_type == "text/plain" {
                                    plain.push(text);
                                } else if mime_type == "text/html" {
                                    html.push(text);
                                }
                            }
                        }
                    }
                }
            }
        }

        if let Some(ref parts) = part.parts {
            for sub in parts {
                Self::collect_text_parts(sub, plain, html);
            }
        }
    }

    /// Recursively collect attachment parts that have a supported MIME type.
    fn collect_attachment_parts(part: &MessagePart, attachments: &mut Vec<AttachmentInfo>) {
        if is_file_attachment(part) {
            if let Some(ref body) = part.body {
                if let Some(ref attachment_id) = body.attachment_id {
                    let mime_type = part
                        .mime_type
                        .as_deref()
                        .unwrap_or("application/octet-stream");
                    let filename = part.filename.as_deref().unwrap_or("attachment");

                    // Infer MIME type from extension if declared as octet-stream
                    let effective_mime = if mime_type == "application/octet-stream" {
                        mime_type_from_extension(filename).unwrap_or(mime_type)
                    } else {
                        mime_type
                    };

                    if is_supported_attachment_type(effective_mime) {
                        // Skip very large attachments (>10MB)
                        let too_large = body.size.map_or(false, |s| s > 10 * 1024 * 1024);
                        if !too_large {
                            attachments.push(AttachmentInfo {
                                attachment_id: attachment_id.clone(),
                                filename: filename.to_string(),
                                mime_type: effective_mime.to_string(),
                            });
                        }
                    }
                }
            }
        }

        if let Some(ref parts) = part.parts {
            for sub in parts {
                Self::collect_attachment_parts(sub, attachments);
            }
        }
    }

    pub async fn download_attachment(
        &self,
        auth: &GoogleAuth,
        user_email: &str,
        message_id: &str,
        attachment_id: &str,
    ) -> Result<Vec<u8>> {
        let message_id = message_id.to_string();
        let attachment_id = attachment_id.to_string();

        let rate_limiter = self.get_or_create_user_rate_limiter(user_email)?;
        execute_with_auth_retry(auth, user_email, rate_limiter.clone(), |token| {
            let message_id = message_id.clone();
            let attachment_id = attachment_id.clone();
            async move {
                let url = format!(
                    "{}/users/{}/messages/{}/attachments/{}",
                    GMAIL_API_BASE, user_email, message_id, attachment_id
                );

                let response = self
                    .client
                    .get(&url)
                    .bearer_auth(&token)
                    .send()
                    .await
                    .with_context(|| {
                        format!(
                            "Failed to download attachment {} for message {}",
                            attachment_id, message_id
                        )
                    })?;

                let status = response.status();
                if is_auth_error(status) {
                    return Ok(ApiResult::AuthError);
                } else if !status.is_success() {
                    let error_text = response.text().await?;
                    return Ok(ApiResult::OtherError(anyhow!(
                        "Gmail API attachment download error: HTTP {} - {}",
                        status,
                        error_text
                    )));
                }

                let body: AttachmentResponse = match response.json().await {
                    Ok(b) => b,
                    Err(e) => {
                        return Ok(ApiResult::OtherError(anyhow!(
                            "Failed to parse attachment response for {} in message {}: {}",
                            attachment_id,
                            message_id,
                            e
                        )));
                    }
                };

                let decoded = match URL_SAFE_NO_PAD.decode(&body.data) {
                    Ok(d) => d,
                    Err(e) => {
                        return Ok(ApiResult::OtherError(anyhow!(
                            "Failed to decode attachment data: {}",
                            e
                        )));
                    }
                };

                Ok(ApiResult::Success(decoded))
            }
        })
        .await
    }

    pub fn get_header_value(&self, message: &GmailMessage, header_name: &str) -> Option<String> {
        message
            .payload
            .as_ref()?
            .headers
            .as_ref()?
            .iter()
            .find(|h| h.name.eq_ignore_ascii_case(header_name))
            .map(|h| h.value.clone())
    }
}

#[derive(Debug, Clone, Copy)]
pub enum MessageFormat {
    Full,
    Metadata,
    Minimal,
    Raw,
}

#[derive(Debug, Deserialize)]
pub struct MessagesListResponse {
    pub messages: Option<Vec<MessageInfo>>,
    #[serde(rename = "nextPageToken")]
    pub next_page_token: Option<String>,
    #[serde(rename = "resultSizeEstimate")]
    pub result_size_estimate: Option<u64>,
}

#[derive(Debug, Deserialize)]
pub struct ThreadsListResponse {
    pub threads: Option<Vec<ThreadInfo>>,
    #[serde(rename = "nextPageToken")]
    pub next_page_token: Option<String>,
    #[serde(rename = "resultSizeEstimate")]
    pub result_size_estimate: Option<u64>,
}

#[derive(Debug, Deserialize)]
pub struct MessageInfo {
    pub id: String,
    #[serde(rename = "threadId")]
    pub thread_id: String,
}

#[derive(Debug, Deserialize)]
pub struct ThreadInfo {
    pub id: String,
    #[serde(rename = "historyId")]
    pub history_id: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct GmailMessage {
    pub id: String,
    #[serde(rename = "threadId")]
    pub thread_id: String,
    #[serde(rename = "labelIds")]
    pub label_ids: Option<Vec<String>>,
    pub snippet: Option<String>,
    #[serde(rename = "historyId")]
    pub history_id: Option<String>,
    #[serde(rename = "internalDate")]
    pub internal_date: Option<String>,
    pub payload: Option<MessagePart>,
    #[serde(rename = "sizeEstimate")]
    pub size_estimate: Option<u64>,
    pub raw: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct MessagePart {
    #[serde(rename = "partId")]
    pub part_id: Option<String>,
    #[serde(rename = "mimeType")]
    pub mime_type: Option<String>,
    pub filename: Option<String>,
    pub headers: Option<Vec<Header>>,
    pub body: Option<MessagePartBody>,
    pub parts: Option<Vec<MessagePart>>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct Header {
    pub name: String,
    pub value: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct MessagePartBody {
    #[serde(rename = "attachmentId")]
    pub attachment_id: Option<String>,
    pub size: Option<u64>,
    pub data: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct HistoryListResponse {
    pub history: Option<Vec<History>>,
    #[serde(rename = "nextPageToken")]
    pub next_page_token: Option<String>,
    #[serde(rename = "historyId")]
    pub history_id: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct History {
    pub id: String,
    pub messages: Option<Vec<HistoryMessage>>,
    #[serde(rename = "messagesAdded")]
    pub messages_added: Option<Vec<HistoryMessageAdded>>,
    #[serde(rename = "messagesDeleted")]
    pub messages_deleted: Option<Vec<HistoryMessageDeleted>>,
    #[serde(rename = "labelsAdded")]
    pub labels_added: Option<Vec<HistoryLabelAdded>>,
    #[serde(rename = "labelsRemoved")]
    pub labels_removed: Option<Vec<HistoryLabelRemoved>>,
}

#[derive(Debug, Deserialize)]
pub struct HistoryMessage {
    pub id: String,
    #[serde(rename = "threadId")]
    pub thread_id: String,
}

#[derive(Debug, Deserialize)]
pub struct HistoryMessageAdded {
    pub message: HistoryMessage,
}

#[derive(Debug, Deserialize)]
pub struct HistoryMessageDeleted {
    pub message: HistoryMessage,
}

#[derive(Debug, Deserialize)]
pub struct HistoryLabelAdded {
    pub message: HistoryMessage,
    #[serde(rename = "labelIds")]
    pub label_ids: Vec<String>,
}

#[derive(Debug, Deserialize)]
pub struct HistoryLabelRemoved {
    pub message: HistoryMessage,
    #[serde(rename = "labelIds")]
    pub label_ids: Vec<String>,
}

#[derive(Debug, Deserialize)]
pub struct GmailProfile {
    #[serde(rename = "emailAddress")]
    pub email_address: String,
    #[serde(rename = "messagesTotal")]
    pub messages_total: Option<u64>,
    #[serde(rename = "threadsTotal")]
    pub threads_total: Option<u64>,
    #[serde(rename = "historyId")]
    pub history_id: String,
}

#[derive(Debug)]
pub enum BatchThreadResult {
    Success(GmailThreadResponse),
    RateLimited,
    Failed(anyhow::Error),
}

#[derive(Debug, Clone, Deserialize)]
pub struct GmailThreadResponse {
    pub id: String,
    #[serde(rename = "historyId")]
    pub history_id: Option<String>,
    pub messages: Vec<GmailMessage>,
}

#[derive(Debug, Deserialize)]
struct AttachmentResponse {
    data: String,
}

#[derive(Debug)]
pub struct AttachmentInfo {
    pub attachment_id: String,
    pub filename: String,
    pub mime_type: String,
}

#[derive(Debug, Clone)]
pub struct ExtractedAttachment {
    pub message_id: String,
    pub attachment_id: String,
    pub filename: String,
    pub mime_type: String,
    pub size: u64,
    pub extracted_text: String,
}

const SUPPORTED_ATTACHMENT_TYPES: &[&str] = &[
    "application/pdf",
    "application/vnd.openxmlformats-officedocument.wordprocessingml.document",
    "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet",
    "application/vnd.openxmlformats-officedocument.presentationml.presentation",
    "application/vnd.ms-excel",
    "text/plain",
    "text/html",
    "text/csv",
    "text/markdown",
];

fn is_supported_attachment_type(mime_type: &str) -> bool {
    SUPPORTED_ATTACHMENT_TYPES.iter().any(|&t| t == mime_type)
}

fn is_file_attachment(part: &MessagePart) -> bool {
    part.filename.as_ref().is_some_and(|f| !f.is_empty())
        || part
            .body
            .as_ref()
            .is_some_and(|b| b.attachment_id.is_some())
}

const HTML_TEXT_WIDTH: usize = 100;

fn html_to_text(html: &str) -> String {
    html2text::from_read(html.as_bytes(), HTML_TEXT_WIDTH).unwrap_or_default()
}

fn mime_type_from_extension(filename: &str) -> Option<&'static str> {
    let ext = filename.rsplit('.').next()?.to_ascii_lowercase();
    match ext.as_str() {
        "pdf" => Some("application/pdf"),
        "docx" => Some("application/vnd.openxmlformats-officedocument.wordprocessingml.document"),
        "xlsx" => Some("application/vnd.openxmlformats-officedocument.spreadsheetml.sheet"),
        "pptx" => Some("application/vnd.openxmlformats-officedocument.presentationml.presentation"),
        "xls" => Some("application/vnd.ms-excel"),
        "txt" => Some("text/plain"),
        "html" | "htm" => Some("text/html"),
        "csv" => Some("text/csv"),
        "md" | "markdown" => Some("text/markdown"),
        _ => None,
    }
}

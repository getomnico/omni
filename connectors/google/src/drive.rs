use anyhow::{anyhow, Context, Result};
use reqwest::Client;
use serde::Deserialize;
use std::sync::{Arc, RwLock};
use std::time::Duration;
use tracing::{debug, warn};

use std::collections::HashMap;

use crate::auth::{execute_with_auth_retry, is_auth_error, ApiResult, GoogleAuth};
use crate::models::{
    DriveChangesResponse, GoogleDriveFile, GooglePresentation, WebhookChannel,
    WebhookChannelResponse,
};
use shared::RateLimiter;

/// Content returned by `get_file_content`. Text formats are already extracted;
/// binary formats carry raw bytes for extraction via the SDK.
pub enum FileContent {
    Text(String),
    Binary {
        data: Vec<u8>,
        mime_type: String,
        filename: String,
    },
}

const DRIVE_API_BASE: &str = "https://www.googleapis.com/drive/v3";
const DOCS_API_BASE: &str = "https://docs.googleapis.com/v1";
const SHEETS_API_BASE: &str = "https://sheets.googleapis.com/v4";
const SLIDES_API_BASE: &str = "https://slides.googleapis.com/v1";

#[derive(Clone)]
pub struct DriveClient {
    client: Client,
    // This rate limiter is for Drive APIs (rate limit: 12k req/min)
    rate_limiter: Arc<RateLimiter>,
    // These rate limiters, one per user, are for Docs/Sheets etc. APIs,
    // which have a rate limit per user of 300 req/min
    user_rate_limiters: Arc<RwLock<HashMap<String, Arc<RateLimiter>>>>,
}

impl DriveClient {
    pub fn new() -> Self {
        let client = Client::builder()
            .timeout(Duration::from_secs(60)) // 60 second timeout for all requests
            .connect_timeout(Duration::from_secs(10)) // 10 second connection timeout
            // .pool_max_idle_per_host(10) // Reuse connections to reduce SSL handshakes
            // .pool_idle_timeout(Duration::from_secs(90)) // Keep connections alive longer
            // .tcp_keepalive(Duration::from_secs(60)) // Enable TCP keepalive
            .build()
            .expect("Failed to build HTTP client");

        let rate_limiter = Arc::new(RateLimiter::new(200, 5)); // 12000 req/min
        let user_rate_limiters = Arc::new(RwLock::new(HashMap::new()));

        Self {
            client,
            rate_limiter,
            user_rate_limiters,
        }
    }

    pub fn with_rate_limiter(rate_limiter: Arc<RateLimiter>) -> Self {
        let client = Client::builder()
            .timeout(Duration::from_secs(60)) // 60 second timeout for all requests
            .connect_timeout(Duration::from_secs(10)) // 10 second connection timeout
            .pool_max_idle_per_host(10) // Reuse connections to reduce SSL handshakes
            .pool_idle_timeout(Duration::from_secs(90)) // Keep connections alive longer
            .tcp_keepalive(Duration::from_secs(60)) // Enable TCP keepalive
            .build()
            .expect("Failed to build HTTP client");

        Self {
            client,
            rate_limiter,
            user_rate_limiters: Arc::new(RwLock::new(HashMap::new())),
        }
    }

    pub async fn list_files(
        &self,
        auth: &GoogleAuth,
        user_email: &str,
        page_token: Option<&str>,
        created_after: Option<&str>,
    ) -> Result<FilesListResponse> {
        let page_token = page_token.map(|s| s.to_string());
        let created_after = created_after.map(|s| s.to_string());

        execute_with_auth_retry(auth, user_email, self.rate_limiter.clone(), |token| {
            let page_token = page_token.clone();
            let created_after = created_after.clone();
            async move {
            let url = format!("{}/files", DRIVE_API_BASE);

            // Build the query filter
            let mut query_parts = vec!["trashed=false".to_string()];
            if let Some(ref date) = created_after {
                query_parts.push(format!("createdTime > '{}'", date));
            }
            let query = query_parts.join(" and ");

            let mut params = vec![
                ("pageSize", "100"),
                ("fields", "nextPageToken,files(id,name,mimeType,webViewLink,createdTime,modifiedTime,size,parents,shared,permissions(id,type,emailAddress,role))"),
                ("q", query.as_str()),
                ("orderBy", "modifiedTime desc"),
                ("includeItemsFromAllDrives", "true"),
                ("supportsAllDrives", "true"),
            ];

            if let Some(ref page_token) = page_token {
                params.push(("pageToken", page_token));
            }

            debug!("[GOOGLE API CALL] list_files for user {}, page_token {:?}", user_email, page_token);
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
                return Ok(ApiResult::OtherError(anyhow!("Failed to list files: HTTP {} - {}", status, error_text)));
            }

            debug!("Drive API response status: {}", status);
            let response_text = response.text().await?;
            debug!("Drive API raw response: {}", response_text);

            let parsed_response = serde_json::from_str(&response_text).map_err(|e| {
                anyhow!(
                    "Failed to parse Drive API response: {}. Raw response: {}",
                    e,
                    response_text
                )
            })?;

            Ok(ApiResult::Success(parsed_response))
            }
        }).await
    }

    pub async fn get_file_content(
        &self,
        auth: &GoogleAuth,
        user_email: &str,
        file: &GoogleDriveFile,
    ) -> Result<FileContent> {
        match file.mime_type.as_str() {
            "application/vnd.google-apps.document" => self
                .get_google_doc_content(auth, user_email, &file.id)
                .await
                .map(FileContent::Text),
            "application/vnd.google-apps.spreadsheet" => self
                .get_google_sheet_content(auth, user_email, &file.id)
                .await
                .map(FileContent::Text),
            "application/vnd.google-apps.presentation" => self
                .get_google_slides_content(auth, user_email, &file.id)
                .await
                .map(FileContent::Text),
            "text/plain" | "text/html" | "text/csv" => self
                .download_file_content(auth, user_email, &file.id)
                .await
                .map(FileContent::Text),
            // Binary document formats — return raw bytes for extraction via SDK
            "application/pdf"
            | "application/vnd.openxmlformats-officedocument.wordprocessingml.document"
            | "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet"
            | "application/vnd.openxmlformats-officedocument.presentationml.presentation"
            | "application/msword"
            | "application/vnd.ms-excel"
            | "application/vnd.ms-powerpoint" => {
                let data = self
                    .download_file_binary(auth, user_email, &file.id)
                    .await?;
                Ok(FileContent::Binary {
                    data,
                    mime_type: file.mime_type.clone(),
                    filename: file.name.clone(),
                })
            }
            _ => {
                debug!("Unsupported file type: {}", file.mime_type);
                Ok(FileContent::Text(String::new()))
            }
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
            .or_insert_with(|| Arc::new(RateLimiter::new(5, 5))) // 300 req/min for each user, 5 retry attempts
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

    async fn get_google_doc_content(
        &self,
        auth: &GoogleAuth,
        user_email: &str,
        file_id: &str,
    ) -> Result<String> {
        let file_id = file_id.to_string();

        let rate_limiter = self.get_or_create_user_rate_limiter(user_email)?;
        execute_with_auth_retry(auth, user_email, rate_limiter.clone(), |token| {
            let file_id = file_id.clone();
            async move {
                let url = format!("{}/documents/{}", DOCS_API_BASE, &file_id);

                debug!("[GOOGLE API CALL] get_google_doc_content for user {}, file_id {}", user_email, file_id);
                let response = self
                    .client
                    .get(&url)
                    .bearer_auth(&token)
                    .send()
                    .await
                    .with_context(|| {
                        format!(
                            "Failed to send request to Google Docs API for file {}",
                            file_id
                        )
                    })?;

                let status = response.status();
                if is_auth_error(status) {
                    return Ok(ApiResult::AuthError);
                } else if !status.is_success() {
                    let error_text = response.text().await?;
                    return Ok(ApiResult::OtherError(anyhow!(
                        "Google Docs API returned error for file {}: HTTP {} - {}",
                        file_id,
                        status,
                        error_text
                    )));
                }

                debug!("Google Docs API response status: {}", status);
                let response_text = response
                    .text()
                    .await
                    .context("Failed to read response body from Google Docs API")?;

                let doc: GoogleDocument =
                    serde_json::from_str(&response_text).with_context(|| {
                        format!(
                            "Failed to parse Google Docs API response for file {}. Raw response: {}",
                            file_id, response_text
                        )
                    })?;

                Ok(ApiResult::Success(extract_text_from_document(&doc)))
            }
        })
        .await
    }

    async fn get_google_sheet_content(
        &self,
        auth: &GoogleAuth,
        user_email: &str,
        file_id: &str,
    ) -> Result<String> {
        let file_id = file_id.to_string();

        let rate_limiter = self.get_or_create_user_rate_limiter(user_email)?;
        execute_with_auth_retry(auth, user_email, rate_limiter.clone(), |token| {
            let file_id = file_id.clone();
            async move {
                let url = format!("{}/spreadsheets/{}", SHEETS_API_BASE, &file_id);

                let response = self.client.get(&url).bearer_auth(&token).send().await?;

                let status = response.status();
                if is_auth_error(status) {
                    return Ok(ApiResult::AuthError);
                } else if !status.is_success() {
                    let error_text = response.text().await?;
                    return Ok(ApiResult::OtherError(anyhow!(
                        "Failed to get spreadsheet metadata: {}",
                        error_text
                    )));
                }

                let sheet: GoogleSpreadsheet = response.json().await?;
                let mut content = String::new();

                for sheet_info in &sheet.sheets {
                    let sheet_name = &sheet_info.properties.title;
                    let range = format!("'{}'", sheet_name);

                    let values_url = format!(
                        "{}/spreadsheets/{}/values/{}",
                        SHEETS_API_BASE, &file_id, range
                    );

                    let values_response = self
                        .client
                        .get(&values_url)
                        .bearer_auth(&token)
                        .send()
                        .await?;

                    if values_response.status().is_success() {
                        if let Ok(values) = values_response.json::<ValueRange>().await {
                            content.push_str(&format!("Sheet: {}\n", sheet_name));
                            for row in values.values.unwrap_or_default() {
                                content.push_str(&row.join("\t"));
                                content.push('\n');
                            }
                            content.push('\n');
                        }
                    }
                }

                Ok(ApiResult::Success(content))
            }
        })
        .await
    }

    async fn get_google_slides_content(
        &self,
        auth: &GoogleAuth,
        user_email: &str,
        file_id: &str,
    ) -> Result<String> {
        let file_id = file_id.to_string();

        let rate_limiter = self.get_or_create_user_rate_limiter(user_email)?;
        execute_with_auth_retry(auth, user_email, rate_limiter.clone(), |token| {
            let file_id = file_id.clone();
            async move {
                let url = format!("{}/presentations/{}", SLIDES_API_BASE, &file_id);

                let response = self.client.get(&url).bearer_auth(&token).send().await?;

                let status = response.status();
                if is_auth_error(status) {
                    return Ok(ApiResult::AuthError);
                } else if !status.is_success() {
                    let error_text = response.text().await?;
                    return Ok(ApiResult::OtherError(anyhow!(
                        "Failed to get presentation content: {}",
                        error_text
                    )));
                }

                debug!("Google Slides API response status: {}", status);
                let response_text = response.text().await?;

                let presentation: GooglePresentation = serde_json::from_str(&response_text)
                    .map_err(|e| {
                        anyhow!(
                            "Failed to parse Google Slides API response: {}. Raw response: {}",
                            e,
                            response_text
                        )
                    })?;

                Ok(ApiResult::Success(extract_text_from_presentation(
                    &presentation,
                )))
            }
        })
        .await
    }

    async fn download_file_content(
        &self,
        auth: &GoogleAuth,
        user_email: &str,
        file_id: &str,
    ) -> Result<String> {
        let file_id = file_id.to_string();

        let rate_limiter = self.get_or_create_user_rate_limiter(user_email)?;
        execute_with_auth_retry(auth, user_email, rate_limiter.clone(), |token| {
            let file_id = file_id.clone();
            async move {
                let url = format!("{}/files/{}?alt=media", DRIVE_API_BASE, &file_id);

                debug!("Downloading file: {}", file_id);
                let response = self
                    .client
                    .get(&url)
                    .bearer_auth(&token)
                    .send()
                    .await
                    .with_context(|| format!("Failed to send request for file {}", file_id))?;

                let status = response.status();
                if is_auth_error(status) {
                    return Ok(ApiResult::AuthError);
                } else if !status.is_success() {
                    let error_text = response.text().await?;
                    return Ok(ApiResult::OtherError(anyhow!(
                        "Failed to download file {}: HTTP {} - {}",
                        file_id,
                        status,
                        error_text
                    )));
                }

                let content = response
                    .text()
                    .await
                    .with_context(|| format!("Failed to read file content for {}", file_id))?;

                Ok(ApiResult::Success(content))
            }
        })
        .await
    }

    pub async fn get_file_metadata(
        &self,
        auth: &GoogleAuth,
        user_email: &str,
        file_id: &str,
    ) -> Result<GoogleDriveFile> {
        let file_id = file_id.to_string();

        let rate_limiter = self.get_or_create_user_rate_limiter(user_email)?;
        execute_with_auth_retry(auth, user_email, rate_limiter.clone(), |token| {
            let file_id = file_id.clone();
            async move {
                let url = format!(
                    "{}/files/{}?fields=id,name,mimeType,webViewLink,createdTime,modifiedTime,size,parents",
                    DRIVE_API_BASE, &file_id
                );

                debug!("Getting file metadata: {}", file_id);
                let response = self
                    .client
                    .get(&url)
                    .bearer_auth(&token)
                    .send()
                    .await
                    .with_context(|| format!("Failed to get metadata for file {}", file_id))?;

                let status = response.status();
                if is_auth_error(status) {
                    return Ok(ApiResult::AuthError);
                } else if !status.is_success() {
                    let error_text = response.text().await?;
                    return Ok(ApiResult::OtherError(anyhow!(
                        "Failed to get file metadata {}: HTTP {} - {}",
                        file_id,
                        status,
                        error_text
                    )));
                }

                let file: GoogleDriveFile = response.json().await.with_context(|| {
                    format!("Failed to parse metadata for file {}", file_id)
                })?;

                Ok(ApiResult::Success(file))
            }
        })
        .await
    }

    pub async fn export_file(
        &self,
        auth: &GoogleAuth,
        user_email: &str,
        file_id: &str,
        export_mime_type: &str,
    ) -> Result<Vec<u8>> {
        let file_id = file_id.to_string();
        let export_mime_type = export_mime_type.to_string();

        let rate_limiter = self.get_or_create_user_rate_limiter(user_email)?;
        execute_with_auth_retry(auth, user_email, rate_limiter.clone(), |token| {
            let file_id = file_id.clone();
            let export_mime_type = export_mime_type.clone();
            async move {
                let url = format!(
                    "{}/files/{}/export?mimeType={}",
                    DRIVE_API_BASE, &file_id, &export_mime_type
                );

                debug!("Exporting file {} as {}", file_id, export_mime_type);
                let response = self
                    .client
                    .get(&url)
                    .bearer_auth(&token)
                    .send()
                    .await
                    .with_context(|| format!("Failed to export file {}", file_id))?;

                let status = response.status();
                if is_auth_error(status) {
                    return Ok(ApiResult::AuthError);
                } else if !status.is_success() {
                    let error_text = response.text().await?;
                    return Ok(ApiResult::OtherError(anyhow!(
                        "Failed to export file {}: HTTP {} - {}",
                        file_id,
                        status,
                        error_text
                    )));
                }

                let bytes = response.bytes().await.with_context(|| {
                    format!("Failed to read export content for file {}", file_id)
                })?;

                Ok(ApiResult::Success(bytes.to_vec()))
            }
        })
        .await
    }

    pub async fn download_file_binary(
        &self,
        auth: &GoogleAuth,
        user_email: &str,
        file_id: &str,
    ) -> Result<Vec<u8>> {
        let file_id = file_id.to_string();

        let rate_limiter = self.get_or_create_user_rate_limiter(user_email)?;
        execute_with_auth_retry(auth, user_email, rate_limiter.clone(), |token| {
            let file_id = file_id.clone();
            async move {
                let url = format!("{}/files/{}?alt=media", DRIVE_API_BASE, &file_id);

                debug!("Downloading binary file: {}", file_id);
                let response = self
                    .client
                    .get(&url)
                    .bearer_auth(&token)
                    .send()
                    .await
                    .with_context(|| format!("Failed to send request for file {}", file_id))?;

                let status = response.status();
                if is_auth_error(status) {
                    return Ok(ApiResult::AuthError);
                } else if !status.is_success() {
                    let error_text = response.text().await?;
                    return Ok(ApiResult::OtherError(anyhow!(
                        "Failed to download file {}: HTTP {} - {}",
                        file_id,
                        status,
                        error_text
                    )));
                }

                // Skip files over 100 MB to prevent OOM
                const MAX_FILE_SIZE_MB: f64 = 100.0;
                if let Some(content_length) =
                    response.headers().get(reqwest::header::CONTENT_LENGTH)
                {
                    if let Ok(length_str) = content_length.to_str() {
                        if let Ok(length) = length_str.parse::<u64>() {
                            let mb = length as f64 / (1024.0 * 1024.0);
                            if mb > MAX_FILE_SIZE_MB {
                                warn!(
                                    "Skipping oversized file {} ({:.1} MB > {:.0} MB limit)",
                                    file_id, mb, MAX_FILE_SIZE_MB
                                );
                                return Ok(ApiResult::OtherError(anyhow!(
                                    "File too large ({:.1} MB), skipping",
                                    mb
                                )));
                            }
                            if mb > 50.0 {
                                warn!("Large office document detected ({}): {:.1} MB", file_id, mb);
                            }
                        }
                    }
                }

                let binary_content = response.bytes().await.with_context(|| {
                    format!("Failed to read binary content for file {}", file_id)
                })?;

                Ok(ApiResult::Success(binary_content.to_vec()))
            }
        })
        .await
    }

    pub async fn register_changes_webhook(
        &self,
        token: &str,
        webhook_channel: &WebhookChannel,
        page_token: &str,
    ) -> Result<WebhookChannelResponse> {
        let url = format!("{}/changes/watch", DRIVE_API_BASE);

        let params = vec![
            ("pageToken", page_token),
            ("includeItemsFromAllDrives", "true"),
            ("supportsAllDrives", "true"),
            ("includeRemoved", "true"),
        ];

        let response = self
            .client
            .post(&url)
            .bearer_auth(token)
            .query(&params)
            .json(webhook_channel)
            .send()
            .await?;

        if !response.status().is_success() {
            let error_text = response.text().await?;
            return Err(anyhow!("Failed to register webhook: {}", error_text));
        }

        let response_text = response.text().await?;
        debug!("Webhook registration response: {}", response_text);

        serde_json::from_str(&response_text).map_err(|e| {
            anyhow!(
                "Failed to parse webhook response: {}. Raw response: {}",
                e,
                response_text
            )
        })
    }

    pub async fn stop_webhook_channel(
        &self,
        token: &str,
        channel_id: &str,
        resource_id: &str,
    ) -> Result<()> {
        let url = format!("{}/channels/stop", DRIVE_API_BASE);

        let stop_request = serde_json::json!({
            "id": channel_id,
            "resourceId": resource_id
        });

        let response = self
            .client
            .post(&url)
            .bearer_auth(token)
            .json(&stop_request)
            .send()
            .await?;

        if !response.status().is_success() {
            let error_text = response.text().await?;
            return Err(anyhow!("Failed to stop webhook channel: {}", error_text));
        }

        debug!("Successfully stopped webhook channel: {}", channel_id);
        Ok(())
    }

    pub async fn get_start_page_token(&self, token: &str) -> Result<String> {
        let url = format!("{}/changes/startPageToken", DRIVE_API_BASE);

        let params = vec![("supportsAllDrives", "true")];

        let response = self
            .client
            .get(&url)
            .bearer_auth(token)
            .query(&params)
            .send()
            .await?;

        if !response.status().is_success() {
            let error_text = response.text().await?;
            return Err(anyhow!("Failed to get start page token: {}", error_text));
        }

        let response_json: serde_json::Value = response.json().await?;
        let start_page_token = response_json["startPageToken"]
            .as_str()
            .ok_or_else(|| anyhow!("Missing startPageToken in response"))?;

        Ok(start_page_token.to_string())
    }

    pub async fn get_folder_metadata(
        &self,
        auth: &GoogleAuth,
        user_email: &str,
        folder_id: &str,
    ) -> Result<GoogleDriveFile> {
        let folder_id = folder_id.to_string();

        execute_with_auth_retry(auth, user_email, self.rate_limiter.clone(), |token| {
            let folder_id = folder_id.clone();
            async move {
                let url = format!("{}/files/{}", DRIVE_API_BASE, folder_id);

                let params = vec![
                    ("fields", "id,name,parents,mimeType"),
                    ("supportsAllDrives", "true"),
                ];

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
                        "Failed to get folder metadata for {}: {}",
                        folder_id,
                        error_text
                    )));
                }

                let response_text = response.text().await?;
                debug!("Folder metadata response: {}", response_text);

                let folder_metadata = serde_json::from_str(&response_text).map_err(|e| {
                    anyhow!(
                        "Failed to parse folder metadata response for {}: {}. Raw response: {}",
                        folder_id,
                        e,
                        response_text
                    )
                })?;

                Ok(ApiResult::Success(folder_metadata))
            }
        })
        .await
    }

    pub async fn list_changes(
        &self,
        token: &str,
        page_token: &str,
    ) -> Result<DriveChangesResponse> {
        let url = format!("{}/changes", DRIVE_API_BASE);

        let params = vec![
            ("pageToken", page_token),
            ("includeItemsFromAllDrives", "true"),
            ("supportsAllDrives", "true"),
            ("includeRemoved", "true"),
            ("fields", "nextPageToken,changes(changeType,removed,file(id,name,mimeType,webViewLink,createdTime,modifiedTime,size,parents,shared,permissions(id,type,emailAddress,role)),fileId,time)"),
        ];

        let response = self
            .client
            .get(&url)
            .bearer_auth(token)
            .query(&params)
            .send()
            .await?;

        if !response.status().is_success() {
            let error_text = response.text().await?;
            return Err(anyhow!("Failed to list changes: {}", error_text));
        }

        let response_text = response.text().await?;
        debug!("Drive changes response: {}", response_text);

        serde_json::from_str(&response_text).map_err(|e| {
            anyhow!(
                "Failed to parse changes response: {}. Raw response: {}",
                e,
                response_text
            )
        })
    }
}

#[derive(Debug, Deserialize)]
pub struct FilesListResponse {
    pub files: Vec<GoogleDriveFile>,
    #[serde(rename = "nextPageToken")]
    pub next_page_token: Option<String>,
}

#[derive(Debug, Deserialize)]
struct GoogleDocument {
    body: DocumentBody,
}

#[derive(Debug, Deserialize)]
struct DocumentBody {
    content: Vec<StructuralElement>,
}

#[derive(Debug, Deserialize)]
struct StructuralElement {
    paragraph: Option<Paragraph>,
    table: Option<Table>,
}

#[derive(Debug, Deserialize)]
struct Table {
    #[serde(rename = "tableRows")]
    table_rows: Vec<TableRow>,
}

#[derive(Debug, Deserialize)]
struct TableRow {
    #[serde(rename = "tableCells")]
    table_cells: Vec<TableCell>,
}

#[derive(Debug, Deserialize)]
struct TableCell {
    content: Vec<StructuralElement>,
}

#[derive(Debug, Deserialize)]
struct Paragraph {
    elements: Vec<ParagraphElement>,
}

#[derive(Debug, Deserialize)]
struct ParagraphElement {
    #[serde(rename = "textRun")]
    text_run: Option<TextRun>,
}

#[derive(Debug, Deserialize)]
struct TextRun {
    content: String,
}

fn stringify_para(para: &Paragraph) -> String {
    let mut text = String::new();
    for elem in &para.elements {
        if let Some(text_run) = &elem.text_run {
            text.push_str(&text_run.content);
        }
    }
    text
}

fn stringify_table(table: &Table) -> String {
    let mut text = String::new();

    for (row_idx, row) in table.table_rows.iter().enumerate() {
        let mut cell_texts = Vec::new();

        for cell in &row.table_cells {
            let mut cell_text = String::new();

            for element in &cell.content {
                if let Some(para) = &element.paragraph {
                    cell_text.push_str(&stringify_para(para));
                } else if let Some(nested_table) = &element.table {
                    cell_text.push_str(&stringify_table(nested_table));
                }
            }

            // Remove newlines within cells and trim
            let cleaned = cell_text.replace('\n', " ").trim().to_string();
            cell_texts.push(cleaned);
        }

        // Format as markdown table row
        text.push_str("| ");
        text.push_str(&cell_texts.join(" | "));
        text.push_str(" |\n");

        // Add separator after first row (header row)
        if row_idx == 0 {
            text.push_str("|");
            for _ in 0..cell_texts.len() {
                text.push_str(" --- |");
            }
            text.push('\n');
        }
    }

    text
}

fn extract_text_from_document(doc: &GoogleDocument) -> String {
    let mut text = String::new();

    for element in &doc.body.content {
        if let Some(para) = &element.paragraph {
            text.push_str(&stringify_para(para));
        } else if let Some(table) = &element.table {
            text.push_str(&stringify_table(table));
        }
    }

    text
}

#[derive(Debug, Deserialize)]
struct GoogleSpreadsheet {
    sheets: Vec<Sheet>,
}

#[derive(Debug, Deserialize)]
struct Sheet {
    properties: SheetProperties,
}

#[derive(Debug, Deserialize)]
struct SheetProperties {
    title: String,
}

#[derive(Debug, Deserialize)]
struct ValueRange {
    values: Option<Vec<Vec<String>>>,
}

fn extract_text_from_presentation(presentation: &GooglePresentation) -> String {
    let mut text = String::new();

    // Add presentation title
    text.push_str(&format!("Title: {}\n\n", presentation.title));

    // Extract text from each slide
    for (slide_index, slide) in presentation.slides.iter().enumerate() {
        text.push_str(&format!("Slide {}: \n", slide_index + 1));

        // Extract text from all page elements in the slide
        for page_element in &slide.page_elements {
            // Extract text from shapes
            if let Some(shape) = &page_element.shape {
                if let Some(text_content) = &shape.text {
                    for text_element in &text_content.text_elements {
                        if let Some(text_run) = &text_element.text_run {
                            text.push_str(&text_run.content);
                        }
                    }
                }
            }

            // Extract text from tables
            if let Some(table) = &page_element.table {
                for table_row in &table.table_rows {
                    for table_cell in &table_row.table_cells {
                        if let Some(text_content) = &table_cell.text {
                            for text_element in &text_content.text_elements {
                                if let Some(text_run) = &text_element.text_run {
                                    text.push_str(&text_run.content);
                                    text.push('\t'); // Separate table cells with tab
                                }
                            }
                        }
                    }
                    text.push('\n'); // New line for each table row
                }
            }
        }

        text.push_str("\n\n"); // Separate slides with double newline
    }

    text.trim().to_string()
}

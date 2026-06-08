use anyhow::{anyhow, Context, Result};
use reqwest::Client;
use serde::Deserialize;
use std::env;
use std::sync::{Arc, RwLock};
use std::time::Duration;
use tracing::{debug, warn};

use std::collections::HashMap;

use crate::auth::{
    classify_google_api_error, execute_with_auth_retry, google_max_retries, ApiResult, GoogleAuth,
};
use crate::models::{
    DriveChangesResponse, GoogleDriveFile, GooglePresentation, WebhookChannel,
    WebhookChannelResponse,
};
use omni_connector_sdk::{RateLimiter, RetryableError};

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
const DEFAULT_GOOGLE_SHEETS_MAX_INDEXED_ROWS: usize = 1000;
const DEFAULT_GOOGLE_DRIVE_MAX_DOWNLOAD_BYTES: usize = 50 * 1024 * 1024;

fn drive_api_base() -> String {
    env::var("GOOGLE_DRIVE_API_BASE").unwrap_or_else(|_| DRIVE_API_BASE.to_string())
}

fn google_sheets_max_indexed_rows() -> usize {
    env::var("GOOGLE_SHEETS_MAX_INDEXED_ROWS")
        .ok()
        .and_then(|value| value.parse::<usize>().ok())
        .filter(|rows| *rows > 0)
        .unwrap_or(DEFAULT_GOOGLE_SHEETS_MAX_INDEXED_ROWS)
}

fn google_drive_max_download_bytes() -> usize {
    env::var("GOOGLE_DRIVE_MAX_DOWNLOAD_BYTES")
        .ok()
        .and_then(|value| value.parse::<usize>().ok())
        .filter(|bytes| *bytes > 0)
        .unwrap_or(DEFAULT_GOOGLE_DRIVE_MAX_DOWNLOAD_BYTES)
}

async fn read_response_bytes_limited(
    mut response: reqwest::Response,
    file_id: &str,
    max_bytes: usize,
) -> Result<ApiResult<Vec<u8>>> {
    if let Some(content_length) = response.headers().get(reqwest::header::CONTENT_LENGTH) {
        if let Ok(length_str) = content_length.to_str() {
            if let Ok(length) = length_str.parse::<usize>() {
                if length > max_bytes {
                    warn!(
                        "Skipping oversized Drive file {} ({} bytes > {} byte download limit)",
                        file_id, length, max_bytes
                    );
                    return Ok(ApiResult::OtherError(anyhow!(
                        "File too large ({} bytes), skipping content download",
                        length
                    )));
                }
            }
        }
    }

    let mut data = Vec::new();
    while let Some(chunk) = response
        .chunk()
        .await
        .with_context(|| format!("Failed to read content for file {}", file_id))?
    {
        if data.len().saturating_add(chunk.len()) > max_bytes {
            warn!(
                "Skipping oversized Drive file {} while streaming (exceeded {} byte download limit)",
                file_id, max_bytes
            );
            return Ok(ApiResult::OtherError(anyhow!(
                "File too large (exceeded {} bytes), skipping content download",
                max_bytes
            )));
        }
        data.extend_from_slice(&chunk);
    }

    Ok(ApiResult::Success(data))
}

fn escape_sheet_name_for_a1(sheet_name: &str) -> String {
    sheet_name.replace('\'', "''")
}

fn encode_a1_range_for_url(range: &str) -> String {
    urlencoding::encode(range).into_owned()
}

#[derive(Clone)]
pub struct DriveClient {
    client: Client,
    // This rate limiter is for Drive APIs (rate limit: 12k req/min)
    rate_limiter: Arc<RateLimiter>,
    // These rate limiters, one per user, are for Docs/Slides APIs,
    // which have a rate limit per user of 300 req/min.
    user_rate_limiters: Arc<RwLock<HashMap<String, Arc<RateLimiter>>>>,
    // Sheets has a lower per-user read quota (60 req/min). Keep it separate
    // from Docs/Slides so spreadsheet crawls cannot overrun the Sheets quota.
    user_sheets_rate_limiters: Arc<RwLock<HashMap<String, Arc<RateLimiter>>>>,
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

        let rate_limiter = Arc::new(RateLimiter::new(200, google_max_retries())); // 12000 req/min
        let user_rate_limiters = Arc::new(RwLock::new(HashMap::new()));
        let user_sheets_rate_limiters = Arc::new(RwLock::new(HashMap::new()));

        Self {
            client,
            rate_limiter,
            user_rate_limiters,
            user_sheets_rate_limiters,
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
            user_sheets_rate_limiters: Arc::new(RwLock::new(HashMap::new())),
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
            let url = format!("{}/files", drive_api_base().as_str());

            // Build the query filter
            let mut query_parts = vec!["trashed=false".to_string()];
            if let Some(ref date) = created_after {
                query_parts.push(format!("createdTime > '{}'", date));
            }
            let query = query_parts.join(" and ");

            let mut params = vec![
                ("pageSize", "100"),
                ("fields", "nextPageToken,files(id,name,mimeType,webViewLink,createdTime,modifiedTime,size,parents,shared,permissions(id,type,emailAddress,role),owners(emailAddress))"),
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
                .await
                .with_context(|| format!("Failed to send list_files request for user {}", user_email))?;

            let status = response.status();
            debug!("Drive list_files response status: {}", status);

            if !status.is_success() {
                return classify_google_api_error(response, "Failed to list files").await;
            }

            let response_text = response.text().await?;
            debug!("Drive API raw response: {}", response_text);

            let parsed_response = match serde_json::from_str(&response_text) {
                Ok(parsed_response) => parsed_response,
                Err(e) => {
                    return Ok(ApiResult::OtherError(anyhow!(
                        "Failed to parse Drive API response: {}. Raw response: {}",
                        e,
                        response_text
                    )));
                }
            };

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
            "text/plain" | "text/html" => self
                .download_file_content(auth, user_email, &file.id)
                .await
                .map(FileContent::Text),
            // Binary and structured document formats — return raw bytes for extraction via SDK.
            // CSV goes through connector-manager extraction so spreadsheet filtering/truncation
            // is centralized with XLS/XLSX handling.
            "application/pdf"
            | "text/csv"
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
            .or_insert_with(|| Arc::new(RateLimiter::new(5, google_max_retries()))) // 300 req/min for each user
            .clone();

        Ok(limiter)
    }

    fn get_or_create_user_sheets_rate_limiter(&self, user_email: &str) -> Result<Arc<RateLimiter>> {
        {
            let rate_limiters = self.user_sheets_rate_limiters.read().map_err(|e| {
                anyhow!(
                    "Failed to acquire read lock on user Sheets rate limiters: {:?}",
                    e
                )
            })?;
            if let Some(limiter) = rate_limiters.get(user_email) {
                return Ok(Arc::clone(limiter));
            }
        }

        let mut rate_limiters = self.user_sheets_rate_limiters.write().map_err(|e| {
            anyhow!(
                "Failed to acquire write lock on user Sheets rate limiters: {:?}",
                e
            )
        })?;

        let limiter = rate_limiters
            .entry(user_email.to_string())
            .or_insert_with(|| Arc::new(RateLimiter::new(1, google_max_retries())))
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
        drop(rate_limiters);

        let mut sheets_rate_limiters = self.user_sheets_rate_limiters.write().map_err(|e| {
            anyhow!(
                "Failed to acquire write lock on user Sheets rate limiters: {:?}",
                e
            )
        })?;
        sheets_rate_limiters.remove(user_email);
        Ok(())
    }

    async fn send_sheets_get_with_retry(
        &self,
        rate_limiter: Arc<RateLimiter>,
        token: &str,
        url: &str,
        context: String,
    ) -> Result<ApiResult<reqwest::Response>> {
        let token = token.to_string();
        let url = url.to_string();
        let context_for_error = context.clone();
        let result = rate_limiter
            .execute_with_retry(|| {
                let client = self.client.clone();
                let token = token.clone();
                let url = url.clone();
                let context = context.clone();

                async move {
                    let response = client
                        .get(&url)
                        .bearer_auth(&token)
                        .send()
                        .await
                        .map_err(|e| RetryableError::Transient(anyhow!(e)))?;

                    let status = response.status();
                    if !status.is_success() {
                        return match classify_google_api_error(response, context)
                            .await
                            .map_err(RetryableError::Transient)?
                        {
                            ApiResult::RetryableError(e) => Err(e),
                            other => Ok(other),
                        };
                    }

                    Ok(ApiResult::Success(response))
                }
            })
            .await;

        match result {
            Ok(api_result) => Ok(api_result),
            Err(e) => Ok(ApiResult::OtherError(e.context(context_for_error))),
        }
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
                if !status.is_success() {
                    return classify_google_api_error(
                        response,
                        format!("Google Docs API returned error for file {}", file_id),
                    )
                    .await;
                }

                debug!("Google Docs API response status: {}", status);
                let response_text = response
                    .text()
                    .await
                    .context("Failed to read response body from Google Docs API")?;

                let doc: GoogleDocument = match serde_json::from_str(&response_text) {
                    Ok(doc) => doc,
                    Err(e) => {
                        return Ok(ApiResult::OtherError(anyhow!(
                            "Failed to parse Google Docs API response for file {}: {}. Raw response: {}",
                            file_id,
                            e,
                            response_text
                        )));
                    }
                };

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

        let rate_limiter = self.get_or_create_user_sheets_rate_limiter(user_email)?;
        execute_with_auth_retry(auth, user_email, rate_limiter.clone(), |token| {
            let file_id = file_id.clone();
            let rate_limiter = rate_limiter.clone();
            async move {
                let url = format!("{}/spreadsheets/{}", SHEETS_API_BASE, &file_id);

                let response = match self
                    .send_sheets_get_with_retry(
                        rate_limiter.clone(),
                        &token,
                        &url,
                        format!("Failed to get spreadsheet metadata for {}", file_id),
                    )
                    .await?
                {
                    ApiResult::Success(response) => response,
                    ApiResult::AuthError(e) => return Ok(ApiResult::AuthError(e)),
                    ApiResult::RetryableError(e) => return Ok(ApiResult::RetryableError(e)),
                    ApiResult::OtherError(e) => return Ok(ApiResult::OtherError(e)),
                };

                let sheet: GoogleSpreadsheet = match response.json().await {
                    Ok(sheet) => sheet,
                    Err(e) => {
                        return Ok(ApiResult::OtherError(anyhow!(
                            "Failed to parse spreadsheet metadata for {}: {}",
                            file_id,
                            e
                        )));
                    }
                };
                let mut content = String::new();
                let max_indexed_rows = google_sheets_max_indexed_rows();

                for sheet_info in &sheet.sheets {
                    let sheet_name = &sheet_info.properties.title;
                    let sheet_rows = sheet_info
                        .properties
                        .grid_properties
                        .as_ref()
                        .and_then(|properties| properties.row_count)
                        .unwrap_or(max_indexed_rows);
                    let rows_to_fetch = sheet_rows.min(max_indexed_rows);
                    if rows_to_fetch == 0 {
                        continue;
                    }
                    let truncated = sheet_rows > rows_to_fetch;

                    let escaped_sheet_name = escape_sheet_name_for_a1(sheet_name);
                    let range = format!("'{}'!1:{}", escaped_sheet_name, rows_to_fetch);

                    let encoded_range = encode_a1_range_for_url(&range);
                    let values_url = format!(
                        "{}/spreadsheets/{}/values/{}",
                        SHEETS_API_BASE, &file_id, encoded_range
                    );

                    let values_response = match self
                        .send_sheets_get_with_retry(
                            rate_limiter.clone(),
                            &token,
                            &values_url,
                            format!(
                                "Failed to get spreadsheet values for {} sheet {}",
                                file_id, sheet_name
                            ),
                        )
                        .await?
                    {
                        ApiResult::Success(response) => response,
                        ApiResult::AuthError(e) => return Ok(ApiResult::AuthError(e)),
                        ApiResult::RetryableError(e) => return Ok(ApiResult::RetryableError(e)),
                        ApiResult::OtherError(e) => return Ok(ApiResult::OtherError(e)),
                    };

                    let values = match values_response.json::<ValueRange>().await {
                        Ok(values) => values,
                        Err(e) => {
                            return Ok(ApiResult::OtherError(anyhow!(
                                "Failed to parse spreadsheet values for {} sheet {}: {}",
                                file_id,
                                sheet_name,
                                e
                            )));
                        }
                    };
                    append_filtered_spreadsheet_sheet(
                        &mut content,
                        sheet_name,
                        values.values.unwrap_or_default(),
                    );

                    if truncated {
                        content.push_str(&format!(
                            "Sheet {} truncated to first {} rows for indexing.\n\n",
                            sheet_name, max_indexed_rows
                        ));
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
                if !status.is_success() {
                    return classify_google_api_error(
                        response,
                        format!("Failed to get presentation content for {}", file_id),
                    )
                    .await;
                }

                debug!("Google Slides API response status: {}", status);
                let response_text = response.text().await?;

                let presentation: GooglePresentation = match serde_json::from_str(&response_text) {
                    Ok(presentation) => presentation,
                    Err(e) => {
                        return Ok(ApiResult::OtherError(anyhow!(
                            "Failed to parse Google Slides API response for file {}: {}. Raw response: {}",
                            file_id,
                            e,
                            response_text
                        )));
                    }
                };

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
                let url = format!("{}/files/{}?alt=media", drive_api_base().as_str(), &file_id);

                debug!(
                    "Downloading file: {} (user={}, url={})",
                    file_id, user_email, url
                );
                let response = self
                    .client
                    .get(&url)
                    .bearer_auth(&token)
                    .send()
                    .await
                    .with_context(|| format!("Failed to send request for file {}", file_id))?;

                let status = response.status();
                if !status.is_success() {
                    return classify_google_api_error(
                        response,
                        format!("Failed to download file {}", file_id),
                    )
                    .await;
                }

                let max_bytes = google_drive_max_download_bytes();
                let bytes = match read_response_bytes_limited(response, &file_id, max_bytes).await?
                {
                    ApiResult::Success(bytes) => bytes,
                    ApiResult::AuthError(e) => return Ok(ApiResult::AuthError(e)),
                    ApiResult::RetryableError(e) => return Ok(ApiResult::RetryableError(e)),
                    ApiResult::OtherError(e) => return Ok(ApiResult::OtherError(e)),
                };
                let content = String::from_utf8(bytes).with_context(|| {
                    format!(
                        "Downloaded file content for {} was not valid UTF-8",
                        file_id
                    )
                })?;

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
                    drive_api_base().as_str(), &file_id
                );

                debug!("Getting file metadata: {} (user={}, url={})", file_id, user_email, url);
                let response = self
                    .client
                    .get(&url)
                    .bearer_auth(&token)
                    .send()
                    .await
                    .with_context(|| format!("Failed to get metadata for file {}", file_id))?;

                let status = response.status();
                if !status.is_success() {
                    return classify_google_api_error(
                        response,
                        format!("Failed to get file metadata {}", file_id),
                    )
                    .await;
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
                    drive_api_base().as_str(),
                    &file_id,
                    &export_mime_type
                );

                debug!(
                    "Exporting file {} as {} (user={}, url={})",
                    file_id, export_mime_type, user_email, url
                );
                let response = self
                    .client
                    .get(&url)
                    .bearer_auth(&token)
                    .send()
                    .await
                    .with_context(|| format!("Failed to export file {}", file_id))?;

                let status = response.status();
                if !status.is_success() {
                    return classify_google_api_error(
                        response,
                        format!("Failed to export file {}", file_id),
                    )
                    .await;
                }

                match read_response_bytes_limited(
                    response,
                    &file_id,
                    google_drive_max_download_bytes(),
                )
                .await?
                {
                    ApiResult::Success(bytes) => Ok(ApiResult::Success(bytes)),
                    ApiResult::AuthError(e) => Ok(ApiResult::AuthError(e)),
                    ApiResult::RetryableError(e) => Ok(ApiResult::RetryableError(e)),
                    ApiResult::OtherError(e) => Ok(ApiResult::OtherError(e)),
                }
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
                let url = format!("{}/files/{}?alt=media", drive_api_base().as_str(), &file_id);

                debug!(
                    "Downloading binary file: {} (user={}, url={})",
                    file_id, user_email, url
                );
                let response = self
                    .client
                    .get(&url)
                    .bearer_auth(&token)
                    .send()
                    .await
                    .with_context(|| format!("Failed to send request for file {}", file_id))?;

                let status = response.status();
                if !status.is_success() {
                    return classify_google_api_error(
                        response,
                        format!("Failed to download file {}", file_id),
                    )
                    .await;
                }

                match read_response_bytes_limited(
                    response,
                    &file_id,
                    google_drive_max_download_bytes(),
                )
                .await?
                {
                    ApiResult::Success(bytes) => Ok(ApiResult::Success(bytes)),
                    ApiResult::AuthError(e) => Ok(ApiResult::AuthError(e)),
                    ApiResult::RetryableError(e) => Ok(ApiResult::RetryableError(e)),
                    ApiResult::OtherError(e) => Ok(ApiResult::OtherError(e)),
                }
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
        let url = format!("{}/changes/watch", drive_api_base().as_str());

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
        let url = format!("{}/channels/stop", drive_api_base().as_str());

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
        let url = format!("{}/changes/startPageToken", drive_api_base().as_str());

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

    pub async fn get_start_page_token_for_user(
        &self,
        auth: &GoogleAuth,
        user_email: &str,
    ) -> Result<String> {
        execute_with_auth_retry(
            auth,
            user_email,
            self.rate_limiter.clone(),
            |token| async move {
                let url = format!("{}/changes/startPageToken", drive_api_base().as_str());
                let params = vec![("supportsAllDrives", "true")];

                let response = self
                    .client
                    .get(&url)
                    .bearer_auth(&token)
                    .query(&params)
                    .send()
                    .await?;

                if !response.status().is_success() {
                    return classify_google_api_error(response, "Failed to get start page token")
                        .await;
                }

                let response_json: serde_json::Value = response.json().await?;
                let start_page_token = response_json["startPageToken"]
                    .as_str()
                    .ok_or_else(|| anyhow!("Missing startPageToken in response"))?
                    .to_string();

                Ok(ApiResult::Success(start_page_token))
            },
        )
        .await
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
                let url = format!("{}/files/{}", drive_api_base().as_str(), folder_id);

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
                if !status.is_success() {
                    return classify_google_api_error(
                        response,
                        format!("Failed to get folder metadata for {}", folder_id),
                    )
                    .await;
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
        let url = format!("{}/changes", drive_api_base().as_str());

        let params = vec![
            ("pageToken", page_token),
            ("includeItemsFromAllDrives", "true"),
            ("supportsAllDrives", "true"),
            ("includeRemoved", "true"),
            ("fields", "nextPageToken,changes(changeType,removed,file(id,name,mimeType,webViewLink,createdTime,modifiedTime,size,parents,shared,permissions(id,type,emailAddress,role),owners(emailAddress)),fileId,time)"),
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
    #[serde(rename = "gridProperties")]
    grid_properties: Option<GridProperties>,
}

#[derive(Debug, Deserialize)]
struct GridProperties {
    #[serde(rename = "rowCount")]
    row_count: Option<usize>,
}

#[derive(Debug, Deserialize)]
struct ValueRange {
    values: Option<Vec<Vec<String>>>,
}

fn is_textual_spreadsheet_cell(cell: &str) -> bool {
    let trimmed = cell.trim();
    if trimmed.is_empty() || is_numeric_like_spreadsheet_cell(trimmed) {
        return false;
    }

    trimmed.chars().any(char::is_alphabetic)
}

fn is_numeric_like_spreadsheet_cell(cell: &str) -> bool {
    let mut normalized = cell.trim().to_string();
    if normalized.is_empty() {
        return false;
    }

    if normalized.starts_with('(') && normalized.ends_with(')') && normalized.len() > 2 {
        normalized = format!("-{}", &normalized[1..normalized.len() - 1]);
    }

    normalized.retain(|ch| {
        !matches!(
            ch,
            ',' | '_' | ' ' | '$' | '€' | '£' | '¥' | '₹' | '%' | '+'
        )
    });

    !normalized.is_empty() && normalized.parse::<f64>().is_ok()
}

fn append_filtered_spreadsheet_sheet<I, R, C>(content: &mut String, sheet_name: &str, rows: I)
where
    I: IntoIterator<Item = R>,
    R: IntoIterator<Item = C>,
    C: AsRef<str>,
{
    content.push_str(&format!("Sheet: {}\n", sheet_name));

    for row in rows {
        let row_text: Vec<String> = row
            .into_iter()
            .filter_map(|cell| {
                let trimmed = cell.as_ref().trim();
                if is_textual_spreadsheet_cell(trimmed) {
                    Some(trimmed.to_string())
                } else {
                    None
                }
            })
            .collect();

        if !row_text.is_empty() {
            content.push_str(&row_text.join("\t"));
            content.push('\n');
        }
    }

    content.push('\n');
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn spreadsheet_cell_textual_heuristic_filters_non_textual_values() {
        let non_textual = [
            "",
            "   ",
            "123",
            "-123",
            "+123",
            "3.1415",
            "-3.1415",
            "1,234,567",
            "$99.00",
            "€1,234.56",
            "12%",
            "(123)",
            "2024-01-31",
            "1.2e6",
            "1,234e5",
            "---",
            "***",
        ];

        for cell in non_textual {
            assert!(
                !is_textual_spreadsheet_cell(cell),
                "expected non-textual cell to be filtered: {cell:?}"
            );
        }
    }

    #[test]
    fn spreadsheet_cell_textual_heuristic_keeps_text_bearing_values() {
        let textual = [
            "Invoice 123",
            "Q4 revenue",
            "SKU123",
            "customer@example.com",
            "hello",
            "東京",
            "مرحبا",
            "строка 12",
        ];

        for cell in textual {
            assert!(
                is_textual_spreadsheet_cell(cell),
                "expected text-bearing cell to be kept: {cell:?}"
            );
        }
    }

    #[test]
    fn filtered_spreadsheet_formatter_skips_numeric_only_rows() {
        let rows = vec![
            vec!["123".to_string(), "456".to_string()],
            vec![
                "Invoice".to_string(),
                "123".to_string(),
                "$10.00".to_string(),
            ],
            vec!["Q4 revenue".to_string(), "12%".to_string()],
            vec!["   ".to_string(), "---".to_string()],
            vec!["東京".to_string(), "2024-01-31".to_string()],
        ];
        let mut content = String::new();

        append_filtered_spreadsheet_sheet(&mut content, "Budget", rows);

        assert_eq!(content, "Sheet: Budget\nInvoice\nQ4 revenue\n東京\n\n");
    }

    #[test]
    fn spreadsheet_metadata_reads_sheet_row_count() {
        let sheet: GoogleSpreadsheet = serde_json::from_str(
            r#"{
                "sheets": [
                    {
                        "properties": {
                            "title": "Data",
                            "gridProperties": { "rowCount": 1500 }
                        }
                    }
                ]
            }"#,
        )
        .unwrap();

        assert_eq!(
            sheet.sheets[0]
                .properties
                .grid_properties
                .as_ref()
                .and_then(|properties| properties.row_count),
            Some(1500)
        );
    }

    #[test]
    fn sheet_name_escape_doubles_single_quotes_for_a1_ranges() {
        assert_eq!(escape_sheet_name_for_a1("Bob's Sheet"), "Bob''s Sheet");
    }

    #[test]
    fn a1_range_url_encoding_preserves_special_sheet_name_chars() {
        let escaped_sheet_name = escape_sheet_name_for_a1("Bob's Gaming/Casino Disney+");
        let range = format!("'{}'!1:1000", escaped_sheet_name);

        assert_eq!(
            encode_a1_range_for_url(&range),
            "%27Bob%27%27s%20Gaming%2FCasino%20Disney%2B%27%211%3A1000"
        );
    }
}

use anyhow::{anyhow, Context, Result};
use reqwest::Client;
use serde::Deserialize;
use std::sync::{Arc, RwLock};
use std::time::Duration;
use tracing::{debug, warn};

// Office document parsing imports
use docx_rs::read_docx;
use quick_xml::events::Event;
use quick_xml::Reader as XmlReader;
use std::collections::HashMap;
use std::io::Cursor;
use zip::ZipArchive;

use crate::auth::{execute_with_auth_retry, is_auth_error, ApiResult, GoogleAuth};
use crate::models::{
    DriveChangesResponse, GoogleDriveFile, GooglePresentation, WebhookChannel,
    WebhookChannelResponse,
};
use shared::{AIClient, RateLimiter};

const DRIVE_API_BASE: &str = "https://www.googleapis.com/drive/v3";
const DOCS_API_BASE: &str = "https://docs.googleapis.com/v1";
const SHEETS_API_BASE: &str = "https://sheets.googleapis.com/v4";
const SLIDES_API_BASE: &str = "https://slides.googleapis.com/v1";

#[derive(Clone)]
pub struct DriveClient {
    client: Client,
    // This rate limiter is for Drive APIs (rate limit: 12k req/min)
    rate_limiter: Arc<RateLimiter>,
    ai_client: Option<AIClient>,
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
            ai_client: None,
            user_rate_limiters,
        }
    }

    pub fn with_rate_limiter(rate_limiter: Arc<RateLimiter>, ai_client: AIClient) -> Self {
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
            ai_client: Some(ai_client),
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
    ) -> Result<String> {
        match file.mime_type.as_str() {
            "application/vnd.google-apps.document" => {
                self.get_google_doc_content(auth, user_email, &file.id)
                    .await
            }
            "application/vnd.google-apps.spreadsheet" => {
                self.get_google_sheet_content(auth, user_email, &file.id)
                    .await
            }
            "application/vnd.google-apps.presentation" => {
                self.get_google_slides_content(auth, user_email, &file.id)
                    .await
            }
            "text/plain" | "text/html" | "text/csv" => {
                self.download_file_content(auth, user_email, &file.id).await
            }
            "application/pdf" => self.get_pdf_content(auth, user_email, &file.id).await,
            "application/vnd.openxmlformats-officedocument.wordprocessingml.document" => {
                self.get_docx_content(auth, user_email, &file.id).await
            }
            "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet" => {
                self.get_excel_content(auth, user_email, &file.id).await
            }
            "application/vnd.openxmlformats-officedocument.presentationml.presentation" => {
                self.get_powerpoint_content(auth, user_email, &file.id)
                    .await
            }
            "application/msword" | "application/vnd.ms-excel" | "application/vnd.ms-powerpoint" => {
                self.get_legacy_office_content(auth, user_email, &file.id, &file.mime_type)
                    .await
            }
            _ => {
                debug!("Unsupported file type: {}", file.mime_type);
                Ok(String::new())
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

    async fn download_file_binary(
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

                // Check content length to warn about large files
                if let Some(content_length) =
                    response.headers().get(reqwest::header::CONTENT_LENGTH)
                {
                    if let Ok(length_str) = content_length.to_str() {
                        if let Ok(length) = length_str.parse::<u64>() {
                            let mb = length as f64 / (1024.0 * 1024.0);
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

    async fn get_pdf_content(
        &self,
        auth: &GoogleAuth,
        user_email: &str,
        file_id: &str,
    ) -> Result<String> {
        // Check if AI client is available
        let ai_client = match &self.ai_client {
            Some(client) => client,
            None => {
                warn!("AI client not configured, cannot extract PDF text");
                return Ok(String::new());
            }
        };

        let file_id = file_id.to_string();
        let ai_client = ai_client.clone();

        let rate_limiter = self.get_or_create_user_rate_limiter(user_email)?;
        execute_with_auth_retry(auth, user_email, rate_limiter.clone(), |token| {
            let file_id = file_id.clone();
            let ai_client = ai_client.clone();
            async move {
                let url = format!("{}/files/{}?alt=media", DRIVE_API_BASE, &file_id);

                debug!("Downloading PDF file: {}", file_id);
                let response = self
                    .client
                    .get(&url)
                    .bearer_auth(&token)
                    .send()
                    .await
                    .with_context(|| format!("Failed to send request for PDF file {}", file_id))?;

                let status = response.status();
                debug!("Download file {} response status: {}", file_id, status);
                if is_auth_error(status) {
                    return Ok(ApiResult::AuthError);
                } else if !status.is_success() {
                    let error_text = response.text().await?;
                    return Ok(ApiResult::OtherError(anyhow!(
                        "Failed to download PDF {}: HTTP {} - {}",
                        file_id,
                        status,
                        error_text
                    )));
                }

                // Check content length to warn about large files
                if let Some(content_length) =
                    response.headers().get(reqwest::header::CONTENT_LENGTH)
                {
                    if let Ok(length_str) = content_length.to_str() {
                        if let Ok(length) = length_str.parse::<u64>() {
                            let mb = length as f64 / (1024.0 * 1024.0);
                            if mb > 10.0 {
                                warn!("Downloading large PDF file {} ({:.2} MB)", file_id, mb);
                            } else {
                                debug!("PDF file {} size: {:.2} MB", file_id, mb);
                            }
                        }
                    }
                }

                let pdf_bytes = response
                    .bytes()
                    .await
                    .with_context(|| format!("Failed to read PDF content for file {}", file_id))?;

                debug!("Sending PDF to AI service for text extraction: {}", file_id);

                // Use AI service to extract text from PDF
                match ai_client.extract_pdf_text(pdf_bytes.to_vec()).await {
                    Ok(extraction_result) => {
                        if let Some(error) = extraction_result.error {
                            debug!(
                                "PDF extraction completed with error for file {}: {}",
                                file_id, error
                            );
                            // Return empty string if extraction failed
                            Ok(ApiResult::Success(String::new()))
                        } else {
                            debug!(
                                "Successfully extracted text from PDF {}: {} pages, {} characters",
                                file_id,
                                extraction_result.page_count,
                                extraction_result.text.len()
                            );
                            Ok(ApiResult::Success(extraction_result.text))
                        }
                    }
                    Err(e) => {
                        warn!("Failed to extract text from PDF {}: {:#}", file_id, e);
                        // Return empty string if extraction failed
                        Ok(ApiResult::Success(String::new()))
                    }
                }
            }
        })
        .await
    }

    async fn get_docx_content(
        &self,
        auth: &GoogleAuth,
        user_email: &str,
        file_id: &str,
    ) -> Result<String> {
        debug!("Extracting DOCX content for file: {}", file_id);
        let binary_data = self.download_file_binary(auth, user_email, file_id).await?;
        let text =
            extract_docx_text(binary_data).context("Failed to extract text from DOCX file")?;
        debug!(
            "Successfully extracted {} characters from DOCX file: {}",
            text.len(),
            file_id
        );
        Ok(text)
    }

    async fn get_excel_content(
        &self,
        auth: &GoogleAuth,
        user_email: &str,
        file_id: &str,
    ) -> Result<String> {
        debug!("Extracting Excel content for file: {}", file_id);
        let binary_data = self.download_file_binary(auth, user_email, file_id).await?;
        let text =
            extract_excel_text(binary_data).context("Failed to extract text from Excel file")?;
        debug!(
            "Successfully extracted {} characters from Excel file: {}",
            text.len(),
            file_id
        );
        Ok(text)
    }

    async fn get_powerpoint_content(
        &self,
        auth: &GoogleAuth,
        user_email: &str,
        file_id: &str,
    ) -> Result<String> {
        debug!("Extracting PowerPoint content for file: {}", file_id);
        let binary_data = self.download_file_binary(auth, user_email, file_id).await?;
        let text = extract_pptx_text(binary_data)
            .context("Failed to extract text from PowerPoint file")?;
        debug!(
            "Successfully extracted {} characters from PowerPoint file: {}",
            text.len(),
            file_id
        );
        Ok(text)
    }

    async fn get_legacy_office_content(
        &self,
        auth: &GoogleAuth,
        user_email: &str,
        file_id: &str,
        mime_type: &str,
    ) -> Result<String> {
        debug!(
            "Attempting to extract legacy Office content for file: {} ({})",
            file_id, mime_type
        );
        let binary_data = self.download_file_binary(auth, user_email, file_id).await?;

        // Try to extract based on detected format
        let result = match mime_type {
            "application/msword" => {
                // Try DOCX extraction (sometimes works for legacy DOC files)
                extract_docx_text(binary_data)
                    .context("Failed to extract text from legacy DOC file")
            }
            "application/vnd.ms-excel" => {
                // Try Excel extraction (calamine supports legacy XLS)
                extract_excel_text(binary_data)
                    .context("Failed to extract text from legacy Excel file")
            }
            "application/vnd.ms-powerpoint" => {
                Err(anyhow!("Legacy PowerPoint format not fully supported"))
            }
            _ => Err(anyhow!("Unknown legacy Office format: {}", mime_type)),
        };

        match result {
            Ok(text) => {
                debug!(
                    "Successfully extracted {} characters from legacy Office file: {}",
                    text.len(),
                    file_id
                );
                Ok(text)
            }
            Err(e) => {
                debug!(
                    "Failed to extract text from legacy Office file {}: {}",
                    file_id, e
                );
                Err(e)
            }
        }
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

// Office document text extraction functions

fn extract_docx_text(binary_data: Vec<u8>) -> Result<String> {
    let docx = read_docx(&binary_data).context("Failed to read DOCX file")?;

    let mut text = String::new();

    // Extract text from document children - focus on paragraphs for now
    for child in &docx.document.children {
        match child {
            docx_rs::DocumentChild::Paragraph(paragraph) => {
                for para_child in &paragraph.children {
                    if let docx_rs::ParagraphChild::Run(run) = para_child {
                        for run_child in &run.children {
                            if let docx_rs::RunChild::Text(text_element) = run_child {
                                text.push_str(&text_element.text);
                            }
                        }
                    }
                }
                text.push('\n');
            }
            // Skip tables for now - they have a complex structure
            // TODO: Add table support once we figure out the correct API
            _ => {} // Skip other types like Table, SectionProperty
        }
    }

    Ok(text.trim().to_string())
}

fn extract_excel_text(binary_data: Vec<u8>) -> Result<String> {
    use calamine::{open_workbook_auto_from_rs, Reader};

    let cursor = Cursor::new(binary_data);
    let mut workbook =
        open_workbook_auto_from_rs(cursor).context("Failed to open Excel file from binary data")?;

    let mut text = String::new();
    let sheet_names = workbook.sheet_names().to_owned();

    for sheet_name in &sheet_names {
        text.push_str(&format!("Sheet: {}\n", sheet_name));

        if let Some(Ok(range)) = workbook.worksheet_range(sheet_name) {
            for row in range.rows() {
                let row_text: Vec<String> = row.iter().map(|cell| cell.to_string()).collect();
                text.push_str(&row_text.join("\t"));
                text.push('\n');
            }
        }
        text.push('\n'); // Separate sheets with newline
    }

    Ok(text.trim().to_string())
}

fn extract_pptx_text(binary_data: Vec<u8>) -> Result<String> {
    let cursor = Cursor::new(binary_data);
    let mut archive = ZipArchive::new(cursor).context("Failed to read PPTX as ZIP archive")?;

    let mut text = String::new();
    let mut slide_counter = 0;

    // Look for slide files in the archive
    for i in 0..archive.len() {
        let mut file = archive
            .by_index(i)
            .context("Failed to read file from PPTX archive")?;
        let file_name = file.name().to_string();

        // Process slide content files
        if file_name.starts_with("ppt/slides/slide") && file_name.ends_with(".xml") {
            slide_counter += 1;
            text.push_str(&format!("Slide {}\n", slide_counter));

            let mut slide_content = String::new();
            std::io::Read::read_to_string(&mut file, &mut slide_content)
                .context("Failed to read slide XML content")?;

            // Parse XML to extract text content
            let slide_text = extract_text_from_pptx_xml(&slide_content)?;
            text.push_str(&slide_text);
            text.push_str("\n\n");
        }
    }

    Ok(text.trim().to_string())
}

fn extract_text_from_pptx_xml(xml_content: &str) -> Result<String> {
    let mut reader = XmlReader::from_str(xml_content);
    reader.trim_text(true);

    let mut text = String::new();
    let mut buf = Vec::new();
    let mut inside_text = false;

    loop {
        match reader.read_event_into(&mut buf) {
            Ok(Event::Start(ref e)) => {
                match e.name().as_ref() {
                    b"a:t" => inside_text = true, // Text element in PowerPoint XML
                    _ => {}
                }
            }
            Ok(Event::Text(e)) => {
                if inside_text {
                    let content = e.unescape().context("Failed to unescape XML text")?;
                    text.push_str(&content);
                    text.push(' ');
                }
            }
            Ok(Event::End(ref e)) => {
                match e.name().as_ref() {
                    b"a:t" => inside_text = false,
                    b"a:p" => text.push('\n'), // New line for paragraphs
                    _ => {}
                }
            }
            Ok(Event::Eof) => break,
            Err(e) => return Err(anyhow!("Error reading PowerPoint XML: {}", e)),
            _ => {}
        }
        buf.clear();
    }

    Ok(text.trim().to_string())
}

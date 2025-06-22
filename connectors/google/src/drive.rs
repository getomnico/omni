use anyhow::{anyhow, Context, Result};
use pdfium_render::prelude::*;
use reqwest::Client;
use serde::Deserialize;
use std::sync::Arc;
use std::time::Duration;
use tokio::time::sleep;
use tracing::{debug, warn};

use crate::models::{
    DriveChangesResponse, GoogleDriveFile, GooglePresentation, WebhookChannel,
    WebhookChannelResponse,
};
use shared::RateLimiter;

const DRIVE_API_BASE: &str = "https://www.googleapis.com/drive/v3";
const DOCS_API_BASE: &str = "https://docs.googleapis.com/v1";
const SHEETS_API_BASE: &str = "https://sheets.googleapis.com/v4";
const SLIDES_API_BASE: &str = "https://slides.googleapis.com/v1";

pub struct DriveClient {
    client: Client,
    rate_limiter: Option<Arc<RateLimiter>>,
}

impl DriveClient {
    pub fn new() -> Self {
        let client = Client::builder()
            .timeout(Duration::from_secs(60)) // 60 second timeout for all requests
            .connect_timeout(Duration::from_secs(10)) // 10 second connection timeout
            .build()
            .expect("Failed to build HTTP client");

        Self {
            client,
            rate_limiter: None,
        }
    }

    pub fn with_rate_limiter(rate_limiter: Arc<RateLimiter>) -> Self {
        let client = Client::builder()
            .timeout(Duration::from_secs(60)) // 60 second timeout for all requests
            .connect_timeout(Duration::from_secs(10)) // 10 second connection timeout
            .build()
            .expect("Failed to build HTTP client");

        Self {
            client,
            rate_limiter: Some(rate_limiter),
        }
    }

    pub async fn list_files(
        &self,
        token: &str,
        page_token: Option<&str>,
    ) -> Result<FilesListResponse> {
        let list_files_impl = || async {
            let url = format!("{}/files", DRIVE_API_BASE);

            let mut params = vec![
                ("pageSize", "100"),
                ("fields", "nextPageToken,files(id,name,mimeType,webViewLink,createdTime,modifiedTime,size,parents,shared,permissions(id,type,emailAddress,role))"),
                ("q", "trashed=false"),
                ("includeItemsFromAllDrives", "true"),
                ("supportsAllDrives", "true"),
            ];

            if let Some(token) = page_token {
                params.push(("pageToken", token));
            }

            let response = self
                .client
                .get(&url)
                .bearer_auth(token)
                .query(&params)
                .send()
                .await?;

            if !response.status().is_success() {
                let error_text = response.text().await?;
                return Err(anyhow!("Failed to list files: {}", error_text));
            }

            debug!("Drive API response status: {}", response.status());
            let response_text = response.text().await?;
            debug!("Drive API raw response: {}", response_text);

            serde_json::from_str(&response_text).map_err(|e| {
                anyhow!(
                    "Failed to parse Drive API response: {}. Raw response: {}",
                    e,
                    response_text
                )
            })
        };

        match &self.rate_limiter {
            Some(limiter) => limiter.execute_with_retry(list_files_impl).await,
            None => list_files_impl().await,
        }
    }

    pub async fn get_file_content(&self, token: &str, file: &GoogleDriveFile) -> Result<String> {
        match file.mime_type.as_str() {
            "application/vnd.google-apps.document" => {
                self.get_google_doc_content(token, &file.id).await
            }
            "application/vnd.google-apps.spreadsheet" => {
                self.get_google_sheet_content(token, &file.id).await
            }
            "application/vnd.google-apps.presentation" => {
                self.get_google_slides_content(token, &file.id).await
            }
            "text/plain" | "text/html" | "text/csv" => {
                self.download_file_content(token, &file.id).await
            }
            "application/pdf" => self.get_pdf_content(token, &file.id).await,
            _ => {
                debug!("Unsupported file type: {}", file.mime_type);
                Ok(String::new())
            }
        }
    }

    async fn get_google_doc_content(&self, token: &str, file_id: &str) -> Result<String> {
        let token = token.to_string();
        let file_id = file_id.to_string();

        let get_doc_impl = || async {
            let url = format!("{}/documents/{}", DOCS_API_BASE, &file_id);

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

            if !response.status().is_success() {
                let status = response.status();
                let error_text = response.text().await?;
                return Err(anyhow!(
                    "Google Docs API returned error for file {}: HTTP {} - {}",
                    file_id,
                    status,
                    error_text
                ));
            }

            debug!("Google Docs API response status: {}", response.status());
            let response_text = response
                .text()
                .await
                .context("Failed to read response body from Google Docs API")?;

            let doc: GoogleDocument = serde_json::from_str(&response_text).with_context(|| {
                format!(
                    "Failed to parse Google Docs API response for file {}. Raw response: {}",
                    file_id, response_text
                )
            })?;
            Ok(extract_text_from_document(&doc))
        };

        match &self.rate_limiter {
            Some(limiter) => limiter.execute_with_retry(get_doc_impl).await,
            None => get_doc_impl().await,
        }
    }

    async fn get_google_sheet_content(&self, token: &str, file_id: &str) -> Result<String> {
        let token = token.to_string();
        let file_id = file_id.to_string();

        let get_sheet_impl = || async {
            let url = format!("{}/spreadsheets/{}", SHEETS_API_BASE, &file_id);

            let response = self.client.get(&url).bearer_auth(&token).send().await?;

            if !response.status().is_success() {
                let error_text = response.text().await?;
                return Err(anyhow!(
                    "Failed to get spreadsheet metadata: {}",
                    error_text
                ));
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

                let values_response = match &self.rate_limiter {
                    Some(limiter) => {
                        limiter
                            .execute(|| async {
                                self.client
                                    .get(&values_url)
                                    .bearer_auth(&token)
                                    .send()
                                    .await
                                    .map_err(|e| anyhow::anyhow!("Request failed: {}", e))
                            })
                            .await?
                    }
                    None => {
                        self.client
                            .get(&values_url)
                            .bearer_auth(&token)
                            .send()
                            .await?
                    }
                };

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

            Ok(content)
        };

        match &self.rate_limiter {
            Some(limiter) => limiter.execute_with_retry(get_sheet_impl).await,
            None => get_sheet_impl().await,
        }
    }

    async fn get_google_slides_content(&self, token: &str, file_id: &str) -> Result<String> {
        let token = token.to_string();
        let file_id = file_id.to_string();

        let get_slides_impl = || async {
            let url = format!("{}/presentations/{}", SLIDES_API_BASE, &file_id);

            let response = self.client.get(&url).bearer_auth(&token).send().await?;

            if !response.status().is_success() {
                let error_text = response.text().await?;
                return Err(anyhow!(
                    "Failed to get presentation content: {}",
                    error_text
                ));
            }

            debug!("Google Slides API response status: {}", response.status());
            let response_text = response.text().await?;

            let presentation: GooglePresentation =
                serde_json::from_str(&response_text).map_err(|e| {
                    anyhow!(
                        "Failed to parse Google Slides API response: {}. Raw response: {}",
                        e,
                        response_text
                    )
                })?;

            Ok(extract_text_from_presentation(&presentation))
        };

        match &self.rate_limiter {
            Some(limiter) => limiter.execute_with_retry(get_slides_impl).await,
            None => get_slides_impl().await,
        }
    }

    async fn download_file_content(&self, token: &str, file_id: &str) -> Result<String> {
        let token = token.to_string();
        let file_id = file_id.to_string();

        let download_impl = || async {
            let url = format!("{}/files/{}?alt=media", DRIVE_API_BASE, &file_id);

            debug!("Downloading file: {}", file_id);
            let response = self
                .client
                .get(&url)
                .bearer_auth(&token)
                .send()
                .await
                .with_context(|| format!("Failed to send request for file {}", file_id))?;

            if !response.status().is_success() {
                let status = response.status();
                let error_text = response.text().await?;
                return Err(anyhow!(
                    "Failed to download file {}: HTTP {} - {}",
                    file_id,
                    status,
                    error_text
                ));
            }

            response
                .text()
                .await
                .with_context(|| format!("Failed to read file content for {}", file_id))
        };

        match &self.rate_limiter {
            Some(limiter) => limiter.execute_with_retry(download_impl).await,
            None => download_impl().await,
        }
    }

    async fn get_pdf_content(&self, token: &str, file_id: &str) -> Result<String> {
        let token = token.to_string();
        let file_id = file_id.to_string();

        let get_pdf_impl = || async {
            let url = format!("{}/files/{}?alt=media", DRIVE_API_BASE, &file_id);

            debug!("Downloading PDF file: {}", file_id);
            let response = self
                .client
                .get(&url)
                .bearer_auth(&token)
                .send()
                .await
                .with_context(|| format!("Failed to send request for PDF file {}", file_id))?;

            if !response.status().is_success() {
                let status = response.status();
                let error_text = response.text().await?;
                return Err(anyhow!(
                    "Failed to download PDF {}: HTTP {} - {}",
                    file_id,
                    status,
                    error_text
                ));
            }

            // Check content length to warn about large files
            if let Some(content_length) = response.headers().get(reqwest::header::CONTENT_LENGTH) {
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

            // Initialize pdfium
            let pdfium = Pdfium::default();

            // Load PDF from bytes
            let document = match pdfium.load_pdf_from_byte_slice(&pdf_bytes, None) {
                Ok(doc) => doc,
                Err(e) => {
                    debug!(
                        "Failed to load PDF: {}. File might be corrupted or password-protected.",
                        e
                    );
                    return Ok(String::new());
                }
            };

            let mut full_text = String::new();

            // Extract text from each page
            for page in document.pages().iter() {
                match page.text() {
                    Ok(page_text) => {
                        let text = page_text.all();
                        full_text.push_str(&text);
                        full_text.push('\n'); // Add page separator
                    }
                    Err(e) => {
                        debug!("Failed to extract text from PDF page: {}", e);
                        // Continue with other pages
                    }
                }
            }

            Ok(full_text.trim().to_string())
        };

        match &self.rate_limiter {
            Some(limiter) => limiter.execute_with_retry(get_pdf_impl).await,
            None => get_pdf_impl().await,
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

fn extract_text_from_document(doc: &GoogleDocument) -> String {
    let mut text = String::new();

    for element in &doc.body.content {
        if let Some(paragraph) = &element.paragraph {
            for elem in &paragraph.elements {
                if let Some(text_run) = &elem.text_run {
                    text.push_str(&text_run.content);
                }
            }
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

use anyhow::{anyhow, Result};
use reqwest::Client;
use serde::Deserialize;
use tracing::debug;

use crate::models::GoogleDriveFile;

const DRIVE_API_BASE: &str = "https://www.googleapis.com/drive/v3";
const DOCS_API_BASE: &str = "https://docs.googleapis.com/v1";
const SHEETS_API_BASE: &str = "https://sheets.googleapis.com/v4";

pub struct DriveClient {
    client: Client,
}

impl DriveClient {
    pub fn new() -> Self {
        Self {
            client: Client::new(),
        }
    }

    pub async fn list_files(
        &self,
        token: &str,
        page_token: Option<&str>,
    ) -> Result<FilesListResponse> {
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
    }

    pub async fn get_file_content(&self, token: &str, file: &GoogleDriveFile) -> Result<String> {
        match file.mime_type.as_str() {
            "application/vnd.google-apps.document" => {
                self.get_google_doc_content(token, &file.id).await
            }
            "application/vnd.google-apps.spreadsheet" => {
                self.get_google_sheet_content(token, &file.id).await
            }
            "text/plain" | "text/html" | "text/csv" => {
                self.download_file_content(token, &file.id).await
            }
            _ => {
                debug!("Unsupported file type: {}", file.mime_type);
                Ok(String::new())
            }
        }
    }

    async fn get_google_doc_content(&self, token: &str, file_id: &str) -> Result<String> {
        let url = format!("{}/documents/{}", DOCS_API_BASE, file_id);

        let response = self.client.get(&url).bearer_auth(token).send().await?;

        if !response.status().is_success() {
            let error_text = response.text().await?;
            return Err(anyhow!("Failed to get document content: {}", error_text));
        }

        debug!("Google Docs API response status: {}", response.status());
        let response_text = response.text().await?;

        let doc: GoogleDocument = serde_json::from_str(&response_text).map_err(|e| {
            anyhow!(
                "Failed to parse Google Docs API response: {}. Raw response: {}",
                e,
                response_text
            )
        })?;
        Ok(extract_text_from_document(&doc))
    }

    async fn get_google_sheet_content(&self, token: &str, file_id: &str) -> Result<String> {
        let url = format!("{}/spreadsheets/{}", SHEETS_API_BASE, file_id);

        let response = self.client.get(&url).bearer_auth(token).send().await?;

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
                SHEETS_API_BASE, file_id, range
            );

            let values_response = self
                .client
                .get(&values_url)
                .bearer_auth(token)
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

        Ok(content)
    }

    async fn download_file_content(&self, token: &str, file_id: &str) -> Result<String> {
        let url = format!("{}/files/{}?alt=media", DRIVE_API_BASE, file_id);

        let response = self.client.get(&url).bearer_auth(token).send().await?;

        if !response.status().is_success() {
            let error_text = response.text().await?;
            return Err(anyhow!("Failed to download file: {}", error_text));
        }

        response.text().await.map_err(Into::into)
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

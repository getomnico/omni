use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use shared::models::{ConnectorEvent, DocumentMetadata, DocumentPermissions};
use sqlx::types::time::OffsetDateTime;
use std::collections::HashMap;
use uuid::Uuid;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GoogleDriveFile {
    pub id: String,
    pub name: String,
    #[serde(rename = "mimeType")]
    pub mime_type: String,
    #[serde(rename = "webViewLink")]
    pub web_view_link: Option<String>,
    #[serde(rename = "createdTime")]
    pub created_time: Option<String>,
    #[serde(rename = "modifiedTime")]
    pub modified_time: Option<String>,
    pub size: Option<String>,
    pub parents: Option<Vec<String>>,
    pub shared: Option<bool>,
    pub permissions: Option<Vec<Permission>>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Permission {
    pub id: String,
    #[serde(rename = "type")]
    pub permission_type: String,
    #[serde(rename = "emailAddress")]
    pub email_address: Option<String>,
    pub role: String,
}

#[derive(Debug, Clone)]
pub struct FolderMetadata {
    pub id: String,
    pub name: String,
    pub parents: Option<Vec<String>>,
}

impl From<GoogleDriveFile> for FolderMetadata {
    fn from(file: GoogleDriveFile) -> Self {
        Self {
            id: file.id,
            name: file.name,
            parents: file.parents,
        }
    }
}

impl GoogleDriveFile {
    pub fn to_connector_event(
        &self,
        sync_run_id: String,
        source_id: String,
        content_id: String,
    ) -> ConnectorEvent {
        self.to_connector_event_with_path(sync_run_id, source_id, content_id, None)
    }

    pub fn to_connector_event_with_path(
        &self,
        sync_run_id: String,
        source_id: String,
        content_id: String,
        path: Option<String>,
    ) -> ConnectorEvent {
        let mut users = Vec::new();

        if let Some(file_permissions) = &self.permissions {
            for perm in file_permissions {
                if let Some(email) = &perm.email_address {
                    users.push(email.clone());
                }
            }
        }

        let mut extra = HashMap::new();
        extra.insert("file_id".to_string(), serde_json::json!(self.id));
        extra.insert(
            "shared".to_string(),
            serde_json::json!(self.shared.unwrap_or(false)),
        );

        // Store Google Drive specific hierarchical data
        let mut google_drive_metadata = HashMap::new();
        if let Some(parents) = &self.parents {
            google_drive_metadata.insert("parents".to_string(), serde_json::json!(parents));
            if let Some(parent) = parents.first() {
                google_drive_metadata.insert("parent_id".to_string(), serde_json::json!(parent));
            }
        }
        extra.insert(
            "google_drive".to_string(),
            serde_json::json!(google_drive_metadata),
        );

        let metadata = DocumentMetadata {
            title: Some(self.name.clone()),
            author: None,
            created_at: self.created_time.as_ref().and_then(|t| {
                t.parse::<DateTime<Utc>>()
                    .ok()
                    .map(|dt| OffsetDateTime::from_unix_timestamp(dt.timestamp()).unwrap())
            }),
            updated_at: self.modified_time.as_ref().and_then(|t| {
                t.parse::<DateTime<Utc>>()
                    .ok()
                    .map(|dt| OffsetDateTime::from_unix_timestamp(dt.timestamp()).unwrap())
            }),
            mime_type: Some(self.mime_type.clone()),
            size: self.size.clone(),
            url: self.web_view_link.clone(),
            path,
            extra: Some(extra),
        };

        let permissions = DocumentPermissions {
            public: false,
            users,
            groups: vec![],
        };

        ConnectorEvent::DocumentCreated {
            sync_run_id,
            source_id,
            document_id: self.id.clone(),
            content_id,
            metadata,
            permissions,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WebhookChannel {
    pub id: String,
    #[serde(rename = "type")]
    pub channel_type: String,
    pub address: String,
    pub params: Option<HashMap<String, String>>,
    pub expiration: Option<String>,
    pub token: Option<String>,
}

impl WebhookChannel {
    pub fn new(webhook_url: String, token: Option<String>) -> Self {
        Self {
            id: Uuid::new_v4().to_string(),
            channel_type: "web_hook".to_string(),
            address: webhook_url,
            params: None,
            expiration: None,
            token,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WebhookChannelResponse {
    pub id: String,
    #[serde(rename = "resourceId")]
    pub resource_id: String,
    #[serde(rename = "resourceUri")]
    pub resource_uri: String,
    pub token: Option<String>,
    pub expiration: Option<String>,
}

#[derive(Debug, Clone)]
pub struct WebhookNotification {
    pub channel_id: String,
    pub resource_state: String,
    pub resource_id: Option<String>,
    pub resource_uri: Option<String>,
    pub changed: Option<String>,
}

impl WebhookNotification {
    pub fn from_headers(headers: &axum::http::HeaderMap) -> Option<Self> {
        let channel_id = headers.get("x-goog-channel-id")?.to_str().ok()?.to_string();

        let resource_state = headers
            .get("x-goog-resource-state")?
            .to_str()
            .ok()?
            .to_string();

        let resource_id = headers
            .get("x-goog-resource-id")
            .and_then(|h| h.to_str().ok())
            .map(|s| s.to_string());

        let resource_uri = headers
            .get("x-goog-resource-uri")
            .and_then(|h| h.to_str().ok())
            .map(|s| s.to_string());

        let changed = headers
            .get("x-goog-changed")
            .and_then(|h| h.to_str().ok())
            .map(|s| s.to_string());

        Some(Self {
            channel_id,
            resource_state,
            resource_id,
            resource_uri,
            changed,
        })
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DriveChangesResponse {
    #[serde(rename = "nextPageToken")]
    pub next_page_token: Option<String>,
    pub changes: Vec<DriveChange>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DriveChange {
    #[serde(rename = "changeType")]
    pub change_type: String,
    pub removed: Option<bool>,
    pub file: Option<GoogleDriveFile>,
    #[serde(rename = "fileId")]
    pub file_id: Option<String>,
    pub time: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct GooglePresentation {
    #[serde(rename = "presentationId")]
    pub presentation_id: String,
    pub title: String,
    pub slides: Vec<Slide>,
}

#[derive(Debug, Deserialize)]
pub struct Slide {
    #[serde(rename = "objectId")]
    pub object_id: String,
    #[serde(rename = "pageElements", default)]
    pub page_elements: Vec<PageElement>,
}

#[derive(Debug, Deserialize)]
pub struct PageElement {
    #[serde(rename = "objectId")]
    pub object_id: String,
    pub shape: Option<Shape>,
    pub table: Option<Table>,
}

#[derive(Debug, Deserialize)]
pub struct Shape {
    pub text: Option<TextContent>,
}

#[derive(Debug, Deserialize)]
pub struct Table {
    #[serde(rename = "tableRows", default)]
    pub table_rows: Vec<TableRow>,
}

#[derive(Debug, Deserialize)]
pub struct TableRow {
    #[serde(rename = "tableCells", default)]
    pub table_cells: Vec<TableCell>,
}

#[derive(Debug, Deserialize)]
pub struct TableCell {
    pub text: Option<TextContent>,
}

#[derive(Debug, Deserialize)]
pub struct TextContent {
    #[serde(rename = "textElements", default)]
    pub text_elements: Vec<TextElement>,
}

#[derive(Debug, Deserialize)]
pub struct TextElement {
    #[serde(rename = "textRun")]
    pub text_run: Option<TextRun>,
}

#[derive(Debug, Deserialize)]
pub struct TextRun {
    pub content: String,
}

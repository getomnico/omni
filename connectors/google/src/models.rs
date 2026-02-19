use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::json;
use shared::models::{ConnectorEvent, DocumentAttributes, DocumentMetadata, DocumentPermissions};
use sqlx::types::time::OffsetDateTime;
use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use uuid::Uuid;

use crate::gmail::GmailMessage;

#[derive(Debug, Clone)]
pub struct UserFile {
    pub user_email: Arc<String>,
    pub file: GoogleDriveFile,
}

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
    pub owners: Option<Vec<Owner>>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Owner {
    pub id: String,
    #[serde(rename = "emailAddress")]
    pub email_address: Option<String>,
    #[serde(rename = "displayName")]
    pub display_name: Option<String>,
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

/// Structured attributes for Google Drive files, used for filtering and faceting.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GoogleDriveFileAttributes {
    pub mime_type: String,
}

impl GoogleDriveFileAttributes {
    pub fn into_attributes(self) -> DocumentAttributes {
        let mut attrs = HashMap::new();
        attrs.insert("mime_type".into(), json!(self.mime_type));
        attrs
    }
}

/// Structured attributes for Gmail threads, used for filtering and faceting.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GmailThreadAttributes {
    pub sender: Option<String>,
    pub labels: Vec<String>,
    pub message_count: usize,
    pub date: Option<String>, // ISO 8601 date (YYYY-MM-DD) for date range queries
}

impl GmailThreadAttributes {
    pub fn into_attributes(self) -> DocumentAttributes {
        let mut attrs = HashMap::new();
        if let Some(sender) = self.sender {
            attrs.insert("sender".into(), json!(sender));
        }
        if !self.labels.is_empty() {
            attrs.insert("labels".into(), json!(self.labels));
        }
        attrs.insert("message_count".into(), json!(self.message_count));
        if let Some(date) = self.date {
            attrs.insert("date".into(), json!(date));
        }
        attrs
    }
}

impl GoogleDriveFile {
    pub fn to_attributes(&self) -> GoogleDriveFileAttributes {
        GoogleDriveFileAttributes {
            mime_type: self.mime_type.clone(),
        }
    }

    pub fn to_connector_event(
        &self,
        sync_run_id: &str,
        source_id: &str,
        content_id: &str,
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
        extra.insert("file_id".to_string(), json!(self.id));
        extra.insert("shared".to_string(), json!(self.shared.unwrap_or(false)));

        // Store Google Drive specific hierarchical data
        let mut google_drive_metadata = HashMap::new();
        if let Some(parents) = &self.parents {
            google_drive_metadata.insert("parents".to_string(), json!(parents));
            if let Some(parent) = parents.first() {
                google_drive_metadata.insert("parent_id".to_string(), json!(parent));
            }
        }
        extra.insert("google_drive".to_string(), json!(google_drive_metadata));

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

        let attributes = self.to_attributes().into_attributes();

        ConnectorEvent::DocumentCreated {
            sync_run_id: sync_run_id.to_string(),
            source_id: source_id.to_string(),
            document_id: self.id.clone(),
            content_id: content_id.to_string(),
            metadata,
            permissions,
            attributes: Some(attributes),
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

#[derive(Debug, Clone)]
pub struct GmailThread {
    pub thread_id: String,
    pub messages: Vec<GmailMessage>,
    pub participants: HashSet<String>,
    pub subject: String,
    pub latest_date: String,
    pub total_messages: usize,
}

impl GmailThread {
    pub fn new(thread_id: String) -> Self {
        Self {
            thread_id,
            messages: Vec::new(),
            participants: HashSet::new(),
            subject: String::new(),
            latest_date: String::new(),
            total_messages: 0,
        }
    }

    pub fn add_message(&mut self, message: GmailMessage) {
        // Update subject from first message if not set
        if self.subject.is_empty() {
            if let Some(subject) = self.extract_header_value(&message, "Subject") {
                self.subject = subject;
            }
        }

        // Extract participants from headers
        self.extract_participants(&message);

        // Update latest date
        if let Some(internal_date) = &message.internal_date {
            if self.latest_date.is_empty() {
                self.latest_date = internal_date.clone();
            } else {
                // Parse both dates as timestamps for proper comparison
                if let (Ok(current_ts), Ok(latest_ts)) = (
                    internal_date.parse::<i64>(),
                    self.latest_date.parse::<i64>(),
                ) {
                    if current_ts > latest_ts {
                        self.latest_date = internal_date.clone();
                    }
                }
            }
        }

        self.messages.push(message);
        self.total_messages = self.messages.len();
    }

    fn extract_participants(&mut self, message: &GmailMessage) {
        let headers_to_check = ["From", "To", "Cc", "Bcc"];

        for header_name in &headers_to_check {
            if let Some(header_value) = self.extract_header_value(message, header_name) {
                // Parse email addresses from header value
                // Simple parsing - in production might want more sophisticated email parsing
                for email in header_value.split(',') {
                    let email = email.trim();
                    // Extract email from "Name <email@domain.com>" format
                    if let Some(start) = email.find('<') {
                        if let Some(end) = email.find('>') {
                            if start < end {
                                let extracted_email = email[start + 1..end].trim().to_lowercase();
                                if !extracted_email.is_empty() {
                                    self.participants.insert(extracted_email);
                                }
                            }
                        }
                    } else if email.contains('@') {
                        // Direct email format
                        self.participants.insert(email.to_lowercase());
                    }
                }
            }
        }
    }

    fn extract_header_value(&self, message: &GmailMessage, header_name: &str) -> Option<String> {
        message
            .payload
            .as_ref()?
            .headers
            .as_ref()?
            .iter()
            .find(|h| h.name.eq_ignore_ascii_case(header_name))
            .map(|h| h.value.clone())
    }

    pub fn aggregate_content(
        &self,
        gmail_client: &crate::gmail::GmailClient,
    ) -> Result<String, anyhow::Error> {
        let mut content_parts = Vec::new();

        // Add subject as the first part
        if !self.subject.is_empty() {
            content_parts.push(format!("Subject: {}", self.subject));
            content_parts.push(String::new()); // Empty line
        }

        // Add each message content
        for (i, message) in self.messages.iter().enumerate() {
            content_parts.push(format!("=== Message {} ===", i + 1));

            // Add basic message info
            if let Some(from) = self.extract_header_value(message, "From") {
                content_parts.push(format!("From: {}", from));
            }
            if let Some(date) = &message.internal_date {
                content_parts.push(format!("Date: {}", date));
            }

            content_parts.push(String::new()); // Empty line

            // Add message content
            match gmail_client.extract_message_content(message) {
                Ok(message_content) => {
                    if !message_content.trim().is_empty() {
                        content_parts.push(message_content.trim().to_string());
                    }
                }
                Err(e) => {
                    content_parts.push(format!("Error extracting message content: {}", e));
                }
            }

            content_parts.push(String::new()); // Empty line between messages
        }

        Ok(content_parts.join("\n"))
    }

    pub fn to_attributes(&self) -> GmailThreadAttributes {
        // Extract sender from first message
        let sender = self
            .messages
            .first()
            .and_then(|msg| self.extract_header_value(msg, "From"));

        // Collect unique labels from all messages
        let labels: Vec<String> = self
            .messages
            .iter()
            .filter_map(|msg| msg.label_ids.as_ref())
            .flatten()
            .cloned()
            .collect::<HashSet<_>>()
            .into_iter()
            .collect();

        // Convert latest_date (millis timestamp) to ISO date
        let date = if !self.latest_date.is_empty() {
            self.latest_date.parse::<i64>().ok().and_then(|millis| {
                DateTime::from_timestamp(millis / 1000, 0)
                    .map(|dt| dt.format("%Y-%m-%d").to_string())
            })
        } else {
            None
        };

        GmailThreadAttributes {
            sender,
            labels,
            message_count: self.total_messages,
            date,
        }
    }

    pub fn to_connector_event(
        &self,
        sync_run_id: &str,
        source_id: &str,
        content_id: &str,
        _gmail_client: &crate::gmail::GmailClient,
    ) -> Result<ConnectorEvent, anyhow::Error> {
        let mut extra = HashMap::new();
        extra.insert("thread_id".to_string(), json!(self.thread_id));
        extra.insert(
            "participants".to_string(),
            json!(self.participants.iter().collect::<Vec<_>>()),
        );

        // Parse latest date for metadata
        let updated_at = if !self.latest_date.is_empty() {
            self.latest_date
                .parse::<i64>()
                .ok()
                .and_then(|millis| OffsetDateTime::from_unix_timestamp(millis / 1000).ok())
        } else {
            None
        };

        let metadata = DocumentMetadata {
            title: Some(if self.subject.is_empty() {
                format!("Gmail Thread {}", self.thread_id)
            } else {
                self.subject.clone()
            }),
            author: None,
            created_at: updated_at,
            updated_at,
            mime_type: Some("application/x-gmail-thread".to_string()),
            size: None,
            url: Some(format!(
                "https://mail.google.com/mail/u/0/#inbox/{}",
                self.thread_id
            )),
            path: Some(format!("/Gmail/{}", self.subject)),
            extra: Some(extra),
        };

        let permissions = DocumentPermissions {
            public: false,
            users: self.participants.iter().cloned().collect(),
            groups: vec![],
        };

        let attributes = self.to_attributes().into_attributes();

        Ok(ConnectorEvent::DocumentCreated {
            sync_run_id: sync_run_id.to_string(),
            source_id: source_id.to_string(),
            document_id: self.thread_id.clone(),
            content_id: content_id.to_string(),
            metadata,
            permissions,
            attributes: Some(attributes),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_google_drive_file_to_connector_event() {
        let file = GoogleDriveFile {
            id: "file123".to_string(),
            name: "Test Document.docx".to_string(),
            mime_type: "application/vnd.openxmlformats-officedocument.wordprocessingml.document"
                .to_string(),
            web_view_link: Some("https://docs.google.com/document/d/file123/view".to_string()),
            created_time: Some("2023-01-15T10:30:00Z".to_string()),
            modified_time: Some("2023-06-20T14:45:00Z".to_string()),
            size: Some("12345".to_string()),
            parents: Some(vec!["folder456".to_string()]),
            shared: Some(true),
            permissions: Some(vec![Permission {
                id: "perm1".to_string(),
                permission_type: "user".to_string(),
                email_address: Some("user@example.com".to_string()),
                role: "reader".to_string(),
            }]),
            owners: None,
        };

        let event = file.to_connector_event("sync123", "source456", "content789", None);

        match event {
            ConnectorEvent::DocumentCreated {
                sync_run_id,
                source_id,
                document_id,
                content_id,
                metadata,
                permissions,
                attributes,
            } => {
                assert_eq!(sync_run_id, "sync123");
                assert_eq!(source_id, "source456");
                assert_eq!(document_id, "file123");
                assert_eq!(content_id, "content789");
                assert_eq!(metadata.title, Some("Test Document.docx".to_string()));
                assert_eq!(
                    metadata.mime_type,
                    Some(
                        "application/vnd.openxmlformats-officedocument.wordprocessingml.document"
                            .to_string()
                    )
                );
                assert_eq!(
                    metadata.url,
                    Some("https://docs.google.com/document/d/file123/view".to_string())
                );
                assert_eq!(metadata.size, Some("12345".to_string()));
                assert!(metadata.created_at.is_some());
                assert!(metadata.updated_at.is_some());

                // Check permissions
                assert!(!permissions.public);
                assert_eq!(permissions.users, vec!["user@example.com".to_string()]);

                // Check attributes
                let attrs = attributes.unwrap();
                assert_eq!(
                    attrs.get("mime_type").unwrap().as_str().unwrap(),
                    "application/vnd.openxmlformats-officedocument.wordprocessingml.document"
                );
            }
            _ => panic!("Expected DocumentCreated event"),
        }
    }

    #[test]
    fn test_google_drive_file_attributes() {
        let file = GoogleDriveFile {
            id: "file123".to_string(),
            name: "test.pdf".to_string(),
            mime_type: "application/pdf".to_string(),
            web_view_link: None,
            created_time: None,
            modified_time: None,
            size: None,
            parents: None,
            shared: None,
            permissions: None,
            owners: None,
        };

        let attrs = file.to_attributes();
        assert_eq!(attrs.mime_type, "application/pdf");

        let doc_attrs = attrs.into_attributes();
        assert_eq!(
            doc_attrs.get("mime_type").unwrap().as_str().unwrap(),
            "application/pdf"
        );
    }

    #[test]
    fn test_gmail_thread_attributes() {
        let attrs = GmailThreadAttributes {
            sender: Some("sender@example.com".to_string()),
            labels: vec!["INBOX".to_string(), "IMPORTANT".to_string()],
            message_count: 5,
            date: Some("2023-06-15".to_string()),
        };

        let doc_attrs = attrs.into_attributes();

        assert_eq!(
            doc_attrs.get("sender").unwrap().as_str().unwrap(),
            "sender@example.com"
        );
        assert!(doc_attrs.get("labels").unwrap().is_array());
        assert_eq!(doc_attrs.get("message_count").unwrap().as_i64().unwrap(), 5);
        assert_eq!(
            doc_attrs.get("date").unwrap().as_str().unwrap(),
            "2023-06-15"
        );
    }

    #[test]
    fn test_gmail_thread_attributes_minimal() {
        let attrs = GmailThreadAttributes {
            sender: None,
            labels: vec![],
            message_count: 1,
            date: None,
        };

        let doc_attrs = attrs.into_attributes();

        assert!(doc_attrs.get("sender").is_none());
        assert!(doc_attrs.get("labels").is_none());
        assert_eq!(doc_attrs.get("message_count").unwrap().as_i64().unwrap(), 1);
        assert!(doc_attrs.get("date").is_none());
    }

    #[test]
    fn test_folder_metadata_from_google_drive_file() {
        let file = GoogleDriveFile {
            id: "folder123".to_string(),
            name: "My Folder".to_string(),
            mime_type: "application/vnd.google-apps.folder".to_string(),
            web_view_link: None,
            created_time: None,
            modified_time: None,
            size: None,
            parents: Some(vec!["parent456".to_string()]),
            shared: None,
            permissions: None,
            owners: None,
        };

        let folder: FolderMetadata = file.into();

        assert_eq!(folder.id, "folder123");
        assert_eq!(folder.name, "My Folder");
        assert_eq!(folder.parents, Some(vec!["parent456".to_string()]));
    }

    #[test]
    fn test_webhook_channel_creation() {
        let channel = WebhookChannel::new(
            "https://example.com/webhook".to_string(),
            Some("token123".to_string()),
        );

        assert!(!channel.id.is_empty()); // UUID generated
        assert_eq!(channel.channel_type, "web_hook");
        assert_eq!(channel.address, "https://example.com/webhook");
        assert_eq!(channel.token, Some("token123".to_string()));
        assert!(channel.params.is_none());
        assert!(channel.expiration.is_none());
    }

    #[test]
    fn test_gmail_thread_new() {
        let thread = GmailThread::new("thread123".to_string());

        assert_eq!(thread.thread_id, "thread123");
        assert!(thread.messages.is_empty());
        assert!(thread.participants.is_empty());
        assert!(thread.subject.is_empty());
        assert!(thread.latest_date.is_empty());
        assert_eq!(thread.total_messages, 0);
    }

    #[test]
    fn test_drive_file_without_permissions() {
        let file = GoogleDriveFile {
            id: "file123".to_string(),
            name: "test.txt".to_string(),
            mime_type: "text/plain".to_string(),
            web_view_link: None,
            created_time: None,
            modified_time: None,
            size: None,
            parents: None,
            shared: None,
            permissions: None,
            owners: None,
        };

        let event = file.to_connector_event("sync1", "source1", "content1", None);

        match event {
            ConnectorEvent::DocumentCreated { permissions, .. } => {
                assert!(permissions.users.is_empty());
                assert!(permissions.groups.is_empty());
                assert!(!permissions.public);
            }
            _ => panic!("Expected DocumentCreated event"),
        }
    }

    #[test]
    fn test_drive_file_with_path() {
        let file = GoogleDriveFile {
            id: "file123".to_string(),
            name: "report.pdf".to_string(),
            mime_type: "application/pdf".to_string(),
            web_view_link: None,
            created_time: None,
            modified_time: None,
            size: None,
            parents: Some(vec!["folder1".to_string()]),
            shared: None,
            permissions: None,
            owners: None,
        };

        let event = file.to_connector_event(
            "sync1",
            "source1",
            "content1",
            Some("/Documents/Reports/report.pdf".to_string()),
        );

        match event {
            ConnectorEvent::DocumentCreated { metadata, .. } => {
                assert_eq!(
                    metadata.path,
                    Some("/Documents/Reports/report.pdf".to_string())
                );
            }
            _ => panic!("Expected DocumentCreated event"),
        }
    }
}

// ============================================================================
// Connector Protocol Models
// ============================================================================

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConnectorManifest {
    pub name: String,
    pub version: String,
    pub sync_modes: Vec<String>,
    #[serde(default)]
    pub actions: Vec<ActionDefinition>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ActionDefinition {
    pub name: String,
    pub description: String,
    pub parameters: serde_json::Value,
}

// Import SyncRequest and SyncResponse from shared crate
pub use shared::models::{SyncRequest, SyncResponse};

/// Extension trait for SyncResponse helper methods
pub trait SyncResponseExt {
    fn started() -> SyncResponse;
    fn error(msg: impl Into<String>) -> SyncResponse;
}

impl SyncResponseExt for SyncResponse {
    fn started() -> SyncResponse {
        SyncResponse {
            status: "started".to_string(),
            message: None,
        }
    }

    fn error(msg: impl Into<String>) -> SyncResponse {
        SyncResponse {
            status: "error".to_string(),
            message: Some(msg.into()),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CancelRequest {
    pub sync_run_id: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CancelResponse {
    pub status: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ActionRequest {
    pub action: String,
    pub params: serde_json::Value,
    pub credentials: serde_json::Value,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ActionResponse {
    pub status: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub result: Option<serde_json::Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

impl ActionResponse {
    pub fn not_supported(action: &str) -> Self {
        Self {
            status: "error".to_string(),
            result: None,
            error: Some(format!("Action not supported: {}", action)),
        }
    }
}

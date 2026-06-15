use chrono::{DateTime, Utc};
use omni_connector_sdk::{
    ConnectorEvent, DocumentAttributes, DocumentMetadata, DocumentPermissions,
};
use serde::{Deserialize, Serialize};
use serde_json::json;
use sqlx::types::time::OffsetDateTime;
use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use uuid::Uuid;

use crate::gmail::GmailMessage;

#[derive(Debug, Clone, Serialize)]
pub struct GoogleDirectoryUser {
    pub id: String,
    pub email: String,
    pub name: String,
    #[serde(rename = "orgUnit")]
    pub org_unit: String,
    pub suspended: bool,
    #[serde(rename = "isAdmin")]
    pub is_admin: bool,
}

#[derive(Debug, Clone, Serialize)]
pub struct SearchUsersResponse {
    pub users: Vec<GoogleDirectoryUser>,
    #[serde(rename = "nextPageToken")]
    pub next_page_token: Option<String>,
    #[serde(rename = "hasMore")]
    pub has_more: bool,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct GoogleConnectorState {
    pub webhook_channel_id: Option<String>,
    pub webhook_resource_id: Option<String>,
    pub webhook_expires_at: Option<i64>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct GoogleSyncCheckpoint {
    pub gmail_history_ids: Option<HashMap<String, String>>,
    pub drive_page_tokens: Option<HashMap<String, String>>,
    pub chat: Option<GoogleChatCheckpoint>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct GoogleChatCheckpoint {
    #[serde(default)]
    pub spaces: HashMap<String, GoogleChatSpaceCheckpoint>,
    pub last_space_discovery_at: Option<String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct GoogleChatSpaceCheckpoint {
    pub space_name: String,
    pub space_id: String,
    pub space_type: String,
    pub display_name: Option<String>,
    pub reader_email: Option<String>,
    pub last_full_sync_at: Option<String>,
    pub last_message_create_time: Option<String>,
    pub last_event_time: Option<String>,
    pub last_acl_sync_at: Option<String>,
    #[serde(default)]
    pub segments: Vec<GoogleChatSegmentCheckpoint>,
    #[serde(default)]
    pub full_in_progress: bool,
    pub full_resume_after_time: Option<String>,
    pub incremental_event_page_token: Option<String>,
    #[serde(default)]
    pub incremental_in_progress: bool,
    pub pending_event_watermark: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GoogleChatSegmentCheckpoint {
    pub external_id: String,
    pub start_message_name: String,
    pub end_message_name: String,
    pub start_time: String,
    pub end_time: String,
    pub message_count: u32,
    pub text_bytes: u32,
    pub finalized: bool,
}

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
    pub permissions: Option<Vec<GoogleDrivePermission>>,
    pub owners: Option<Vec<Owner>>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Owner {
    pub id: Option<String>,
    #[serde(rename = "emailAddress")]
    pub email_address: Option<String>,
    #[serde(rename = "displayName")]
    pub display_name: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GoogleDrivePermission {
    pub id: String,
    #[serde(rename = "type")]
    pub permission_type: String,
    #[serde(rename = "emailAddress")]
    pub email_address: Option<String>,
    pub domain: Option<String>,
    pub role: String,
    #[serde(rename = "allowFileDiscovery")]
    pub allow_file_discovery: Option<bool>,
    #[serde(rename = "permissionDetails")]
    pub permission_details: Option<Vec<serde_json::Value>>,
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

pub fn mime_type_to_content_type(mime_type: &str) -> Option<String> {
    match mime_type {
        "application/vnd.google-apps.document"
        | "application/vnd.openxmlformats-officedocument.wordprocessingml.document"
        | "application/msword" => Some("document".to_string()),
        "application/vnd.google-apps.spreadsheet"
        | "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet"
        | "application/vnd.ms-excel" => Some("spreadsheet".to_string()),
        "application/vnd.google-apps.presentation"
        | "application/vnd.openxmlformats-officedocument.presentationml.presentation"
        | "application/vnd.ms-powerpoint" => Some("presentation".to_string()),
        "application/pdf" => Some("pdf".to_string()),
        _ => None,
    }
}

/// Structured attributes for Gmail threads, used for filtering and faceting.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GmailThreadAttributes {
    pub sender: Option<String>,
    pub from: Option<String>,
    pub to: Vec<String>,
    pub cc: Vec<String>,
    pub labels: Vec<String>,
    pub message_count: usize,
    pub date: Option<String>, // ISO 8601 date (YYYY-MM-DD) for date range queries
}

impl GmailThreadAttributes {
    pub fn into_attributes(self) -> DocumentAttributes {
        let mut attrs = HashMap::new();
        if let Some(sender) = self.sender {
            attrs.insert("sender".into(), json!(sender.clone()));
        }
        if let Some(from) = self.from {
            attrs.insert("from".into(), json!(from));
        }
        if !self.to.is_empty() {
            attrs.insert("to".into(), json!(self.to));
        }
        if !self.cc.is_empty() {
            attrs.insert("cc".into(), json!(self.cc));
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
    pub fn to_document_permissions(&self, oauth_user_email: Option<&str>) -> DocumentPermissions {
        let mut is_public = false;
        let mut users = Vec::new();
        let mut groups = Vec::new();

        if let Some(file_permissions) = &self.permissions {
            for perm in file_permissions {
                match perm.permission_type.as_str() {
                    "anyone" => {
                        if perm.allow_file_discovery.unwrap_or(true) {
                            is_public = true;
                        }
                    }
                    "group" => {
                        if let Some(email) = &perm.email_address {
                            groups.push(email.clone());
                        }
                    }
                    "user" => {
                        if let Some(email) = &perm.email_address {
                            users.push(email.clone());
                        }
                    }
                    "domain" => {
                        if perm.allow_file_discovery.unwrap_or(true) {
                            if let Some(domain) =
                                perm.domain.as_ref().or(perm.email_address.as_ref())
                            {
                                groups.push(domain.clone());
                            }
                        }
                    }
                    _ => {}
                }
            }
        }

        // Owner access is implicit in personal Drive and not listed in permissions
        if let Some(owners) = &self.owners {
            for owner in owners {
                if let Some(email) = &owner.email_address {
                    if !users.contains(email) {
                        users.push(email.clone());
                    }
                }
            }
        }

        // Viewers can't see the permissions array, but presence in Drive listing implies access
        if let Some(email) = oauth_user_email {
            let email = email.to_string();
            if !users.contains(&email) {
                users.push(email);
            }
        }

        DocumentPermissions {
            public: is_public,
            users,
            groups,
        }
    }

    pub fn to_connector_event(
        &self,
        sync_run_id: &str,
        source_id: &str,
        content_id: &str,
        path: Option<String>,
        oauth_user_email: Option<&str>,
    ) -> ConnectorEvent {
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
            content_type: mime_type_to_content_type(&self.mime_type),
            mime_type: Some(self.mime_type.clone()),
            size: self.size.clone(),
            url: self.web_view_link.clone(),
            path,
            extra: Some(extra),
        };

        let permissions = self.to_document_permissions(oauth_user_email);

        let attributes = HashMap::new();

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
    pub fn new(webhook_url: String, source_id: &str) -> Self {
        Self {
            id: Uuid::new_v4().to_string(),
            channel_type: "web_hook".to_string(),
            address: webhook_url,
            params: None,
            expiration: None,
            token: Some(source_id.to_string()),
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
    pub source_id: Option<String>,
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

        let source_id = headers
            .get("x-goog-channel-token")
            .and_then(|h| h.to_str().ok())
            .map(|s| s.to_string());

        Some(Self {
            channel_id,
            resource_state,
            resource_id,
            resource_uri,
            changed,
            source_id,
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
    pub from: HashSet<String>,
    pub to: HashSet<String>,
    pub cc: HashSet<String>,
    pub subject: String,
    pub latest_date: String,
    pub total_messages: usize,
    pub message_id: Option<String>,
}

/// A back-reference from an email thread to one of its indexed attachments.
/// Surfaced in the thread document's `metadata.extra.attachments`.
///
/// `id` is the attachment's external_id (composite: `{thread}:att:{msg}:{att}`),
/// not the indexer-assigned ULID — that ULID isn't known when the thread is
/// emitted. The AI service's `read_document` accepts either form, so the model
/// can use this id without an extra resolution step.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AttachmentPointer {
    pub id: String,
    pub filename: String,
    pub mime_type: String,
    pub size: u64,
}

impl GmailThread {
    pub fn new(thread_id: String) -> Self {
        Self {
            thread_id,
            messages: Vec::new(),
            participants: HashSet::new(),
            from: HashSet::new(),
            to: HashSet::new(),
            cc: HashSet::new(),
            subject: String::new(),
            latest_date: String::new(),
            total_messages: 0,
            message_id: None,
        }
    }

    pub fn add_message(&mut self, message: GmailMessage) {
        // Extract Message-ID from first message for permalink URL
        if self.message_id.is_none() {
            if let Some(mid) = self.extract_header_value(&message, "Message-ID") {
                self.message_id = Some(mid);
            }
        }

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
        for (header_name, recipient_set) in [
            ("From", &mut self.from),
            ("To", &mut self.to),
            ("Cc", &mut self.cc),
        ] {
            if let Some(header_value) = extract_header_value(message, header_name) {
                for email in parse_email_addresses(&header_value) {
                    recipient_set.insert(email.clone());
                    self.participants.insert(email);
                }
            }
        }
    }

    fn extract_header_value(&self, message: &GmailMessage, header_name: &str) -> Option<String> {
        extract_header_value(message, header_name)
    }

    pub fn canonical_external_id(&self) -> String {
        self.message_id
            .as_deref()
            .map(clean_message_id)
            .filter(|id| !id.is_empty())
            .unwrap_or_else(|| self.thread_id.clone())
    }

    pub async fn aggregate_content(
        &self,
        gmail_client: &crate::gmail::GmailClient,
        sdk_client: &omni_connector_sdk::SdkClient,
        sync_run_id: &str,
    ) -> Result<String, anyhow::Error> {
        let mut content_parts = Vec::new();

        // Add subject as the first part
        if !self.subject.is_empty() {
            content_parts.push(format!("Subject: {}", self.subject));
            content_parts.push(String::new());
        }

        // Add each message's text content (attachments are indexed as separate documents)
        for (i, message) in self.messages.iter().enumerate() {
            content_parts.push(format!("=== Message {} ===", i + 1));

            if let Some(from) = self.extract_header_value(message, "From") {
                content_parts.push(format!("From: {}", from));
            }
            if let Some(date) = &message.internal_date {
                content_parts.push(format!("Date: {}", date));
            }

            content_parts.push(String::new());

            match gmail_client.extract_message_content(message) {
                Ok((message_content, is_html)) => {
                    let raw_len = message_content.len();
                    if !message_content.trim().is_empty() {
                        let text = if is_html {
                            match sdk_client
                                .extract_text(
                                    sync_run_id,
                                    message_content.into_bytes(),
                                    "text/html",
                                    None,
                                )
                                .await
                            {
                                Ok(t) => t,
                                Err(e) => {
                                    tracing::warn!(
                                        thread_id = %self.thread_id,
                                        msg = i + 1,
                                        error = %e,
                                        "extract_text failed; skipping HTML body for this message"
                                    );
                                    String::new()
                                }
                            }
                        } else {
                            message_content
                        };
                        if !text.trim().is_empty() {
                            content_parts.push(text.trim().to_string());
                        } else {
                            tracing::warn!(
                                thread_id = %self.thread_id,
                                message_id = %message.id,
                                msg = i + 1,
                                is_html,
                                raw_len,
                                "Indexed body is empty despite a non-empty source payload"
                            );
                        }
                    }
                }
                Err(e) => {
                    content_parts.push(format!("Error extracting message content: {}", e));
                }
            }

            content_parts.push(String::new());
        }

        Ok(content_parts.join("\n"))
    }

    pub fn to_attributes(&self) -> GmailThreadAttributes {
        let sender = self
            .messages
            .first()
            .and_then(|msg| extract_header_value(msg, "From"))
            .and_then(|from| parse_email_addresses(&from).into_iter().next());

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
            sender: sender.clone(),
            from: sender,
            to: self.sorted_email_values(&self.to),
            cc: self.sorted_email_values(&self.cc),
            labels,
            message_count: self.total_messages,
            date,
        }
    }

    fn sorted_email_values(&self, values: &HashSet<String>) -> Vec<String> {
        let mut values = values.iter().cloned().collect::<Vec<_>>();
        values.sort();
        values
    }

    pub fn to_connector_event(
        &self,
        sync_run_id: &str,
        source_id: &str,
        content_id: &str,
        known_groups: &HashSet<String>,
        user_email: &str,
        attachments: &[AttachmentPointer],
    ) -> Result<ConnectorEvent, anyhow::Error> {
        let canonical_external_id = self.canonical_external_id();
        let mut extra = HashMap::new();
        extra.insert("thread_id".to_string(), json!(self.thread_id));
        extra.insert(
            "participants".to_string(),
            json!(self.sorted_email_values(&self.participants)),
        );
        if !attachments.is_empty() {
            extra.insert("attachments".to_string(), json!(attachments));
        }

        // Parse latest date for metadata
        let updated_at = if !self.latest_date.is_empty() {
            self.latest_date
                .parse::<i64>()
                .ok()
                .and_then(|millis| OffsetDateTime::from_unix_timestamp(millis / 1000).ok())
        } else {
            None
        };

        // Build URL using rfc822msgid search for reliable Gmail permalinks.
        // Gmail web UI uses internal IDs that differ from API thread IDs,
        // so a search-based URL is the most reliable way to link to a thread.
        let url = self
            .message_id
            .as_ref()
            .map(|mid| {
                let clean_id = mid.trim_start_matches('<').trim_end_matches('>');
                let encoded = urlencoding::encode(clean_id);
                format!(
                    "https://mail.google.com/mail/#search/rfc822msgid%3A{}",
                    encoded
                )
            })
            .or_else(|| {
                Some(format!(
                    "https://mail.google.com/mail/#all/{}",
                    self.thread_id
                ))
            });

        let metadata = DocumentMetadata {
            title: Some(if self.subject.is_empty() {
                format!("Gmail Thread {}", self.thread_id)
            } else {
                self.subject.clone()
            }),
            author: None,
            created_at: updated_at,
            updated_at,
            content_type: Some("email_thread".to_string()),
            mime_type: Some("application/x-gmail-thread".to_string()),
            size: None,
            url,
            path: Some(format!("/Gmail/{}", self.subject)),
            extra: Some(extra),
        };

        // Split participants into users and groups based on known org groups
        let mut users = Vec::new();
        let mut groups = Vec::new();
        let mut permission_participants = self.participants.clone();
        permission_participants.insert(user_email.to_lowercase());
        for participant in &permission_participants {
            if known_groups.contains(participant) {
                groups.push(participant.clone());
            } else {
                users.push(participant.clone());
            }
        }
        users.sort();
        users.dedup();
        groups.sort();
        groups.dedup();

        let permissions = DocumentPermissions {
            public: false,
            users,
            groups,
        };

        let attributes = self.to_attributes().into_attributes();

        Ok(ConnectorEvent::DocumentCreated {
            sync_run_id: sync_run_id.to_string(),
            source_id: source_id.to_string(),
            document_id: canonical_external_id,
            content_id: content_id.to_string(),
            metadata,
            permissions,
            attributes: Some(attributes),
        })
    }
}

fn extract_header_value(message: &GmailMessage, header_name: &str) -> Option<String> {
    message
        .payload
        .as_ref()?
        .headers
        .as_ref()?
        .iter()
        .find(|h| h.name.eq_ignore_ascii_case(header_name))
        .map(|h| h.value.clone())
}

fn clean_message_id(message_id: &str) -> String {
    message_id
        .trim()
        .trim_start_matches('<')
        .trim_end_matches('>')
        .to_string()
}

/// Parse email addresses from a header value, handling quoted display names
/// that may contain commas (e.g., `"Smith, John" <john@example.com>`).
fn parse_email_addresses(header_value: &str) -> Vec<String> {
    let mut results = Vec::new();
    let mut current = String::new();
    let mut in_quotes = false;

    for ch in header_value.chars() {
        match ch {
            '"' => {
                in_quotes = !in_quotes;
                current.push(ch);
            }
            ',' if !in_quotes => {
                if let Some(email) = extract_email(&current) {
                    results.push(email);
                }
                current.clear();
            }
            _ => current.push(ch),
        }
    }

    // Process the last segment
    if let Some(email) = extract_email(&current) {
        results.push(email);
    }

    results
}

/// Extract a lowercased email address from a string that may be in
/// `"Display Name" <email@domain.com>` or bare `email@domain.com` format.
fn extract_email(s: &str) -> Option<String> {
    let s = s.trim();
    if s.is_empty() {
        return None;
    }
    if let Some(start) = s.find('<') {
        if let Some(end) = s.find('>') {
            if start < end {
                let email = s[start + 1..end].trim().to_lowercase();
                if !email.is_empty() && email.contains('@') {
                    return Some(email);
                }
            }
        }
    } else if s.contains('@') {
        return Some(s.to_lowercase());
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::gmail::{GmailMessage, Header, MessagePart};

    fn gmail_message_with_headers(headers: Vec<(&str, &str)>) -> GmailMessage {
        GmailMessage {
            id: "msg1".to_string(),
            thread_id: "gmail-thread-1".to_string(),
            label_ids: Some(vec!["INBOX".to_string()]),
            snippet: None,
            history_id: None,
            internal_date: Some("1686787200000".to_string()),
            payload: Some(MessagePart {
                part_id: None,
                mime_type: Some("text/plain".to_string()),
                filename: None,
                headers: Some(
                    headers
                        .into_iter()
                        .map(|(name, value)| Header {
                            name: name.to_string(),
                            value: value.to_string(),
                        })
                        .collect(),
                ),
                body: None,
                parts: None,
            }),
            size_estimate: None,
            raw: None,
        }
    }

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
            permissions: Some(vec![GoogleDrivePermission {
                id: "perm1".to_string(),
                permission_type: "user".to_string(),
                email_address: Some("user@example.com".to_string()),
                domain: None,
                role: "reader".to_string(),
                allow_file_discovery: None,
                permission_details: None,
            }]),
            owners: None,
        };

        let event = file.to_connector_event("sync123", "source456", "content789", None, None);

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
                assert!(permissions.groups.is_empty());

                // Attributes should be empty (mime_type moved to metadata)
                let attrs = attributes.unwrap();
                assert!(attrs.is_empty());
            }
            _ => panic!("Expected DocumentCreated event"),
        }
    }

    #[test]
    fn test_google_drive_file_content_type_mapping() {
        let cases = vec![
            ("application/vnd.google-apps.document", Some("document")),
            (
                "application/vnd.google-apps.spreadsheet",
                Some("spreadsheet"),
            ),
            (
                "application/vnd.google-apps.presentation",
                Some("presentation"),
            ),
            ("application/pdf", Some("pdf")),
            ("text/plain", None),
        ];

        for (mime, expected) in cases {
            assert_eq!(
                mime_type_to_content_type(mime).as_deref(),
                expected,
                "Failed for MIME type: {}",
                mime
            );
        }
    }

    #[test]
    fn test_gmail_thread_attributes() {
        let attrs = GmailThreadAttributes {
            sender: Some("sender@example.com".to_string()),
            from: Some("sender@example.com".to_string()),
            to: vec![
                "alice@example.com".to_string(),
                "bob@example.com".to_string(),
            ],
            cc: vec!["carol@example.com".to_string()],
            labels: vec!["INBOX".to_string(), "IMPORTANT".to_string()],
            message_count: 5,
            date: Some("2023-06-15".to_string()),
        };

        let doc_attrs = attrs.into_attributes();

        assert_eq!(
            doc_attrs.get("sender").unwrap().as_str().unwrap(),
            "sender@example.com"
        );
        assert_eq!(
            doc_attrs.get("from").unwrap().as_str().unwrap(),
            "sender@example.com"
        );
        assert_eq!(doc_attrs.get("to").unwrap().as_array().unwrap().len(), 2);
        assert_eq!(doc_attrs.get("cc").unwrap().as_array().unwrap().len(), 1);
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
            from: None,
            to: vec![],
            cc: vec![],
            labels: vec![],
            message_count: 1,
            date: None,
        };

        let doc_attrs = attrs.into_attributes();

        assert!(doc_attrs.get("sender").is_none());
        assert!(doc_attrs.get("from").is_none());
        assert!(doc_attrs.get("to").is_none());
        assert!(doc_attrs.get("cc").is_none());
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
        let channel = WebhookChannel::new("https://example.com/webhook".to_string(), "source123");

        assert!(!channel.id.is_empty()); // UUID generated
        assert_eq!(channel.channel_type, "web_hook");
        assert_eq!(channel.address, "https://example.com/webhook");
        assert_eq!(channel.token, Some("source123".to_string()));
        assert!(channel.params.is_none());
        assert!(channel.expiration.is_none());
    }

    #[test]
    fn test_gmail_thread_new() {
        let thread = GmailThread::new("thread123".to_string());

        assert_eq!(thread.thread_id, "thread123");
        assert!(thread.messages.is_empty());
        assert!(thread.participants.is_empty());
        assert!(thread.from.is_empty());
        assert!(thread.to.is_empty());
        assert!(thread.cc.is_empty());
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

        let event = file.to_connector_event("sync1", "source1", "content1", None, None);

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
    fn test_drive_file_permission_types() {
        let file = GoogleDriveFile {
            id: "file_mixed".to_string(),
            name: "mixed.txt".to_string(),
            mime_type: "text/plain".to_string(),
            web_view_link: None,
            created_time: None,
            modified_time: None,
            size: None,
            parents: None,
            shared: Some(true),
            permissions: Some(vec![
                GoogleDrivePermission {
                    id: "perm1".to_string(),
                    permission_type: "user".to_string(),
                    email_address: Some("alice@example.com".to_string()),
                    domain: None,
                    role: "writer".to_string(),
                    allow_file_discovery: None,
                    permission_details: None,
                },
                GoogleDrivePermission {
                    id: "perm2".to_string(),
                    permission_type: "group".to_string(),
                    email_address: Some("team@example.com".to_string()),
                    domain: None,
                    role: "reader".to_string(),
                    allow_file_discovery: None,
                    permission_details: None,
                },
                GoogleDrivePermission {
                    id: "perm3".to_string(),
                    permission_type: "anyone".to_string(),
                    email_address: None,
                    domain: None,
                    role: "reader".to_string(),
                    allow_file_discovery: None,
                    permission_details: None,
                },
                GoogleDrivePermission {
                    id: "perm4".to_string(),
                    permission_type: "domain".to_string(),
                    email_address: Some("example.com".to_string()),
                    domain: None,
                    role: "reader".to_string(),
                    allow_file_discovery: None,
                    permission_details: None,
                },
            ]),
            owners: None,
        };

        let event = file.to_connector_event("sync1", "source1", "content1", None, None);
        match event {
            ConnectorEvent::DocumentCreated { permissions, .. } => {
                assert!(permissions.public);
                assert_eq!(permissions.users, vec!["alice@example.com".to_string()]);
                assert_eq!(
                    permissions.groups,
                    vec!["team@example.com".to_string(), "example.com".to_string()]
                );
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
            None,
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

    #[test]
    fn test_drive_file_link_only_permissions_do_not_overgrant() {
        let file = GoogleDriveFile {
            id: "file_link_only".to_string(),
            name: "link-only.txt".to_string(),
            mime_type: "text/plain".to_string(),
            web_view_link: None,
            created_time: None,
            modified_time: None,
            size: None,
            parents: None,
            shared: Some(true),
            permissions: Some(vec![
                GoogleDrivePermission {
                    id: "perm_anyone_link".to_string(),
                    permission_type: "anyone".to_string(),
                    email_address: None,
                    domain: None,
                    role: "reader".to_string(),
                    allow_file_discovery: Some(false),
                    permission_details: None,
                },
                GoogleDrivePermission {
                    id: "perm_domain_link".to_string(),
                    permission_type: "domain".to_string(),
                    email_address: None,
                    domain: Some("example.com".to_string()),
                    role: "reader".to_string(),
                    allow_file_discovery: Some(false),
                    permission_details: Some(vec![json!({"inherited": true})]),
                },
            ]),
            owners: None,
        };

        let permissions = file.to_document_permissions(None);
        assert!(!permissions.public);
        assert!(permissions.groups.is_empty());
        assert!(permissions.users.is_empty());
    }

    #[test]
    fn test_drive_file_domain_permission_uses_domain_field() {
        let file = GoogleDriveFile {
            id: "file_domain".to_string(),
            name: "domain.txt".to_string(),
            mime_type: "text/plain".to_string(),
            web_view_link: None,
            created_time: None,
            modified_time: None,
            size: None,
            parents: None,
            shared: Some(true),
            permissions: Some(vec![GoogleDrivePermission {
                id: "perm_domain".to_string(),
                permission_type: "domain".to_string(),
                email_address: None,
                domain: Some("example.com".to_string()),
                role: "reader".to_string(),
                allow_file_discovery: Some(true),
                permission_details: None,
            }]),
            owners: None,
        };

        let permissions = file.to_document_permissions(None);
        assert_eq!(permissions.groups, vec!["example.com".to_string()]);
        assert!(!permissions.public);
    }

    #[test]
    fn test_drive_file_owner_added_from_owners_array() {
        // Personal Drive files have empty permissions array; owner is implicit via owners field
        let file = GoogleDriveFile {
            id: "file1".to_string(),
            name: "doc.txt".to_string(),
            mime_type: "text/plain".to_string(),
            web_view_link: None,
            created_time: None,
            modified_time: None,
            size: None,
            parents: None,
            shared: None,
            permissions: Some(vec![]),
            owners: Some(vec![Owner {
                id: None,
                email_address: Some("owner@example.com".to_string()),
                display_name: None,
            }]),
        };

        let event = file.to_connector_event("sync1", "source1", "content1", None, None);
        match event {
            ConnectorEvent::DocumentCreated { permissions, .. } => {
                assert_eq!(permissions.users, vec!["owner@example.com".to_string()]);
                assert!(!permissions.public);
            }
            _ => panic!("Expected DocumentCreated event"),
        }
    }

    #[test]
    fn test_drive_file_oauth_viewer_added_to_permissions() {
        // Viewer syncing a shared file: permissions array is empty (non-owners can't read it),
        // but the syncing user must be included since the file appeared in their listing
        let file = GoogleDriveFile {
            id: "file1".to_string(),
            name: "doc.txt".to_string(),
            mime_type: "text/plain".to_string(),
            web_view_link: None,
            created_time: None,
            modified_time: None,
            size: None,
            parents: None,
            shared: Some(true),
            permissions: Some(vec![]),
            owners: Some(vec![Owner {
                id: None,
                email_address: Some("owner@example.com".to_string()),
                display_name: None,
            }]),
        };

        let event = file.to_connector_event(
            "sync1",
            "source1",
            "content1",
            None,
            Some("viewer@example.com"),
        );
        match event {
            ConnectorEvent::DocumentCreated { permissions, .. } => {
                assert!(permissions.users.contains(&"owner@example.com".to_string()));
                assert!(
                    permissions
                        .users
                        .contains(&"viewer@example.com".to_string())
                );
                assert_eq!(permissions.users.len(), 2);
            }
            _ => panic!("Expected DocumentCreated event"),
        }
    }

    #[test]
    fn test_drive_file_oauth_user_not_duplicated_if_already_in_permissions() {
        // If the OAuth user is already in the permissions array (e.g. they are the owner),
        // they should not appear twice
        let file = GoogleDriveFile {
            id: "file1".to_string(),
            name: "doc.txt".to_string(),
            mime_type: "text/plain".to_string(),
            web_view_link: None,
            created_time: None,
            modified_time: None,
            size: None,
            parents: None,
            shared: None,
            permissions: Some(vec![GoogleDrivePermission {
                id: "perm1".to_string(),
                permission_type: "user".to_string(),
                email_address: Some("owner@example.com".to_string()),
                domain: None,
                role: "owner".to_string(),
                allow_file_discovery: None,
                permission_details: None,
            }]),
            owners: Some(vec![Owner {
                id: None,
                email_address: Some("owner@example.com".to_string()),
                display_name: None,
            }]),
        };

        let event = file.to_connector_event(
            "sync1",
            "source1",
            "content1",
            None,
            Some("owner@example.com"),
        );
        match event {
            ConnectorEvent::DocumentCreated { permissions, .. } => {
                assert_eq!(permissions.users, vec!["owner@example.com".to_string()]);
            }
            _ => panic!("Expected DocumentCreated event"),
        }
    }

    #[test]
    fn test_parse_email_addresses_simple() {
        let result = parse_email_addresses("alice@example.com, bob@example.com");
        assert_eq!(result, vec!["alice@example.com", "bob@example.com"]);
    }

    #[test]
    fn test_parse_email_addresses_with_display_names() {
        let result = parse_email_addresses("Alice <alice@example.com>, Bob <bob@example.com>");
        assert_eq!(result, vec!["alice@example.com", "bob@example.com"]);
    }

    #[test]
    fn test_parse_email_addresses_quoted_commas() {
        let result = parse_email_addresses(
            r#""Smith, John" <john@example.com>, "Doe, Jane" <jane@example.com>"#,
        );
        assert_eq!(result, vec!["john@example.com", "jane@example.com"]);
    }

    #[test]
    fn test_parse_email_addresses_mixed() {
        let result =
            parse_email_addresses(r#"plain@example.com, "Quoted, Name" <quoted@example.com>"#);
        assert_eq!(result, vec!["plain@example.com", "quoted@example.com"]);
    }

    #[test]
    fn test_gmail_thread_new_has_no_message_id() {
        let thread = GmailThread::new("t1".to_string());
        assert!(thread.message_id.is_none());
    }

    #[test]
    fn test_gmail_thread_uses_message_id_as_canonical_external_id() {
        let mut thread = GmailThread::new("gmail-thread-local".to_string());
        thread.add_message(gmail_message_with_headers(vec![
            ("Message-ID", "<canonical@example.com>"),
            ("From", "Sender <sender@example.com>"),
            ("To", "Alice <alice@example.com>"),
        ]));

        assert_eq!(thread.canonical_external_id(), "canonical@example.com");

        let event = thread
            .to_connector_event(
                "sync1",
                "source1",
                "content1",
                &HashSet::new(),
                "alice@example.com",
                &[],
            )
            .unwrap();

        match event {
            ConnectorEvent::DocumentCreated {
                document_id,
                metadata,
                ..
            } => {
                assert_eq!(document_id, "canonical@example.com");
                let extra = metadata.extra.expect("extra populated");
                assert_eq!(extra["thread_id"], "gmail-thread-local");
            }
            _ => panic!("Expected DocumentCreated event"),
        }
    }

    #[test]
    fn test_gmail_thread_extracts_recipient_attributes_without_bcc() {
        let mut thread = GmailThread::new("thread123".to_string());
        thread.add_message(gmail_message_with_headers(vec![
            ("Message-ID", "<m1@example.com>"),
            ("From", "Sender <sender@example.com>"),
            (
                "To",
                "Alice <alice@example.com>, \"Bob, Example\" <bob@example.com>",
            ),
            ("Cc", "Carol <carol@example.com>"),
            ("Bcc", "Hidden <hidden@example.com>"),
        ]));

        let attrs = thread.to_attributes().into_attributes();
        assert_eq!(attrs["sender"], "sender@example.com");
        assert_eq!(attrs["from"], "sender@example.com");
        assert_eq!(
            attrs["to"].as_array().unwrap(),
            &vec![json!("alice@example.com"), json!("bob@example.com")]
        );
        assert_eq!(
            attrs["cc"].as_array().unwrap(),
            &vec![json!("carol@example.com")]
        );
        assert!(attrs.get("bcc").is_none());
    }

    #[test]
    fn test_gmail_thread_permissions_include_mailbox_owner() {
        let mut thread = GmailThread::new("thread123".to_string());
        thread.add_message(gmail_message_with_headers(vec![
            ("Message-ID", "<m1@example.com>"),
            ("From", "Sender <sender@example.com>"),
            ("To", "Alice <alice@example.com>"),
        ]));

        let event = thread
            .to_connector_event(
                "sync1",
                "source1",
                "content1",
                &HashSet::new(),
                "owner@example.com",
                &[],
            )
            .unwrap();

        match event {
            ConnectorEvent::DocumentCreated { permissions, .. } => {
                assert!(permissions.users.contains(&"alice@example.com".to_string()));
                assert!(permissions.users.contains(&"owner@example.com".to_string()));
                assert!(
                    permissions
                        .users
                        .contains(&"sender@example.com".to_string())
                );
            }
            _ => panic!("Expected DocumentCreated event"),
        }
    }

    #[test]
    fn test_gmail_thread_to_connector_event_includes_attachment_pointers() {
        let mut thread = GmailThread::new("thread123".to_string());
        thread.subject = "Q3 Report".to_string();
        thread.total_messages = 1;
        thread.participants.insert("alice@co.com".to_string());

        let attachments = vec![AttachmentPointer {
            id: "CABc123%40mail.example.test:att:report.pdf:12345".to_string(),
            filename: "report.pdf".to_string(),
            mime_type: "application/pdf".to_string(),
            size: 12345,
        }];

        let event = thread
            .to_connector_event(
                "sync1",
                "source1",
                "content1",
                &HashSet::new(),
                "alice@co.com",
                &attachments,
            )
            .unwrap();

        match event {
            ConnectorEvent::DocumentCreated { metadata, .. } => {
                let extra = metadata.extra.expect("extra populated");
                let arr = extra
                    .get("attachments")
                    .expect("attachments key present")
                    .as_array()
                    .expect("attachments is array");
                assert_eq!(arr.len(), 1);
                assert_eq!(
                    arr[0]["id"],
                    "CABc123%40mail.example.test:att:report.pdf:12345"
                );
                assert_eq!(arr[0]["filename"], "report.pdf");
                assert_eq!(arr[0]["mime_type"], "application/pdf");
                assert_eq!(arr[0]["size"], 12345);
            }
            _ => panic!("Expected DocumentCreated event"),
        }
    }

    #[test]
    fn test_gmail_thread_to_connector_event_omits_attachments_when_empty() {
        let mut thread = GmailThread::new("thread123".to_string());
        thread.subject = "No attachments".to_string();
        thread.total_messages = 1;
        thread.participants.insert("alice@co.com".to_string());

        let event = thread
            .to_connector_event(
                "sync1",
                "source1",
                "content1",
                &HashSet::new(),
                "alice@co.com",
                &[],
            )
            .unwrap();

        match event {
            ConnectorEvent::DocumentCreated { metadata, .. } => {
                let extra = metadata.extra.expect("extra populated");
                assert!(
                    extra.get("attachments").is_none(),
                    "attachments key should be absent when no attachments were indexed"
                );
            }
            _ => panic!("Expected DocumentCreated event"),
        }
    }
}

use anyhow::{Context, Result, anyhow};
use dashmap::DashMap;
use futures::{StreamExt, stream};
use omni_connector_sdk::SyncContext;
use std::collections::{HashMap, HashSet, VecDeque};
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};
use time::{self, OffsetDateTime};
use tokio::sync::{Mutex, Notify, OwnedSemaphorePermit, Semaphore};
use tracing::{debug, error, info, warn};

const GOOGLE_FILE_CONCURRENCY: usize = 8;
const DEFAULT_GOOGLE_DRIVE_PARALLEL_USERS: usize = 3;
const DEFAULT_GOOGLE_DRIVE_MAX_DOWNLOAD_BYTES: usize = 50 * 1024 * 1024;
const DEFAULT_GOOGLE_WEBHOOK_DEBOUNCE_SECONDS: u64 = 4 * 60 * 60;
const GOOGLE_MAX_BUFFERED_BYTES: usize = 512 * 1024 * 1024;
const GOOGLE_BUFFER_PERMIT_UNIT: usize = 64 * 1024;
const GOOGLE_BUFFER_PERMITS: usize = GOOGLE_MAX_BUFFERED_BYTES / GOOGLE_BUFFER_PERMIT_UNIT;

/// Batch size for shared-drive ACL-aware file emission.
const SA_DIRECT_BATCH_SIZE: usize = 200;

/// Bump when scoped DWD permission derivation changes so existing documents
/// are re-emitted with freshly computed shared-drive ACLs.
const SCOPED_DWD_PERMISSION_MODEL_VERSION: &str = "scoped-dwd-effective-acl-v1";

/// `application/vnd.google-apps.folder` — folders carry access metadata but are
/// never indexed as documents.
const FOLDER_MIME_TYPE: &str = "application/vnd.google-apps.folder";

pub(crate) fn permits_for_bytes(bytes: usize) -> u32 {
    if bytes == 0 {
        return 0;
    }

    bytes.div_ceil(GOOGLE_BUFFER_PERMIT_UNIT) as u32
}

fn is_google_api_service_disabled_message(message: &str) -> bool {
    let message = message.to_lowercase();
    message.contains("service_disabled")
        || (message.contains("has not been used in project") && message.contains("disabled"))
}

fn is_google_api_service_disabled_error(error: &anyhow::Error) -> bool {
    is_google_api_service_disabled_message(&format!("{:#}", error))
}

fn google_api_service_disabled_error(api_name: &str, error: &anyhow::Error) -> anyhow::Error {
    anyhow!(
        "{} API is disabled or has not been enabled in the Google Cloud project used by this service account. Enable the API in Google Cloud Console, wait for propagation, then retry. Original error: {:#}",
        api_name,
        error
    )
}

fn file_content_len(content: &FileContent) -> usize {
    match content {
        FileContent::Text(text) => text.len(),
        FileContent::Binary { data, .. } => data.len(),
    }
}

fn estimated_file_size_bytes(file: &crate::models::GoogleDriveFile) -> Option<usize> {
    file.size.as_ref()?.parse::<usize>().ok()
}

fn is_native_google_workspace_mime(mime_type: &str) -> bool {
    mime_type.starts_with("application/vnd.google-apps.")
}

fn permits_for_unknown_size_file(file: &crate::models::GoogleDriveFile) -> u32 {
    if is_native_google_workspace_mime(&file.mime_type) {
        permits_for_bytes(google_drive_max_download_bytes())
    } else {
        GOOGLE_BUFFER_PERMITS as u32
    }
}

fn google_drive_parallel_users() -> usize {
    std::env::var("GOOGLE_DRIVE_PARALLEL_USERS")
        .ok()
        .and_then(|value| value.parse::<usize>().ok())
        .filter(|users| *users > 0)
        .unwrap_or(DEFAULT_GOOGLE_DRIVE_PARALLEL_USERS)
}

fn google_drive_max_download_bytes() -> usize {
    std::env::var("GOOGLE_DRIVE_MAX_DOWNLOAD_BYTES")
        .ok()
        .and_then(|value| value.parse::<usize>().ok())
        .filter(|bytes| *bytes > 0)
        .unwrap_or(DEFAULT_GOOGLE_DRIVE_MAX_DOWNLOAD_BYTES)
}

fn drive_scope_requires_full_sync(
    sync_type: SyncType,
    stored_fingerprint: Option<&str>,
    current_fingerprint: &str,
) -> bool {
    sync_type != SyncType::Incremental || stored_fingerprint != Some(current_fingerprint)
}

/// A drive scoped by folder-path filters: either the entire drive or a set of
/// folder subtrees within it (or both — entire-drive supersedes folders).
#[derive(Debug, Clone)]
struct DriveGroup {
    drive_id: String,
    entire_drive: bool,
    folder_ids: Vec<String>,
}

#[derive(Debug, Default)]
struct SharedDriveAclState {
    drive_acl_fingerprints: HashMap<String, String>,
    drive_acl_overrides: HashMap<String, DocumentPermissions>,
    drive_acl_raw: HashMap<String, Vec<GoogleDrivePermission>>,
    drive_acl_changed: HashMap<String, bool>,
}

struct DriveUserSyncResult {
    scanned: usize,
    updated: usize,
    next_page_token: Option<String>,
}

/// Build drive groups from folder-path filter entries. Multiple entries for the
/// same drive merge; a `SharedDriveRoot` entry makes the whole drive selected
/// and clears any folder IDs (entire-drive supersedes per-folder selection).
fn build_drive_groups(filter_entries: &[crate::models::FolderPathFilterEntry]) -> Vec<DriveGroup> {
    let mut drive_groups: HashMap<String, DriveGroup> = HashMap::new();
    for entry in filter_entries {
        let group = drive_groups
            .entry(entry.drive_id.clone())
            .or_insert(DriveGroup {
                drive_id: entry.drive_id.clone(),
                entire_drive: false,
                folder_ids: Vec::new(),
            });
        match entry.kind {
            crate::models::FolderPathFilterKind::SharedDriveRoot => {
                group.entire_drive = true;
                group.folder_ids.clear(); // entire-drive supersedes per-folder.
            }
            crate::models::FolderPathFilterKind::Folder => {
                if !group.entire_drive {
                    group.folder_ids.push(entry.id.clone());
                }
            }
        }
    }
    drive_groups.into_values().collect()
}

fn google_webhook_debounce_duration_ms() -> u64 {
    std::env::var("GOOGLE_WEBHOOK_DEBOUNCE_SECONDS")
        .ok()
        .and_then(|value| value.parse::<u64>().ok())
        .filter(|seconds| *seconds > 0)
        .unwrap_or(DEFAULT_GOOGLE_WEBHOOK_DEBOUNCE_SECONDS)
        .saturating_mul(1000)
}

#[derive(Default)]
struct DriveContentCache {
    content_ids: DashMap<String, String>,
    permissions: DashMap<String, DocumentPermissions>,
    locks: DashMap<String, Arc<Mutex<()>>>,
}

impl DriveContentCache {
    fn get_content_id(&self, file_id: &str) -> Option<String> {
        self.content_ids
            .get(file_id)
            .map(|content_id| content_id.value().clone())
    }

    fn insert_content_id(&self, file_id: &str, content_id: String) {
        self.content_ids.insert(file_id.to_string(), content_id);
    }

    fn merge_permissions(
        &self,
        file_id: &str,
        permissions: DocumentPermissions,
    ) -> DocumentPermissions {
        let mut merged = self
            .permissions
            .get(file_id)
            .map(|cached| cached.value().clone())
            .unwrap_or(DocumentPermissions {
                public: false,
                users: Vec::new(),
                groups: Vec::new(),
            });

        merged.public |= permissions.public;
        for user in permissions.users {
            if !merged.users.contains(&user) {
                merged.users.push(user);
            }
        }
        for group in permissions.groups {
            if !merged.groups.contains(&group) {
                merged.groups.push(group);
            }
        }

        self.permissions.insert(file_id.to_string(), merged.clone());
        merged
    }

    fn lock_for_file(&self, file_id: &str) -> Arc<Mutex<()>> {
        self.locks
            .entry(file_id.to_string())
            .or_insert_with(|| Arc::new(Mutex::new(())))
            .value()
            .clone()
    }
}

async fn await_with_heartbeat<T, F>(ctx: &SyncContext, operation: F) -> Result<T>
where
    F: Future<Output = Result<T>>,
{
    let heartbeat_interval = std::env::var("GOOGLE_SYNC_HEARTBEAT_INTERVAL_SECS")
        .ok()
        .and_then(|value| value.parse::<u64>().ok())
        .unwrap_or(30)
        .max(1);
    let mut ticker = tokio::time::interval(Duration::from_secs(heartbeat_interval));
    tokio::pin!(operation);

    loop {
        tokio::select! {
            result = &mut operation => return result,
            _ = ticker.tick() => {
                if ctx.is_cancelled() {
                    return Err(anyhow!("Sync cancelled"));
                }
                if let Err(e) = ctx.heartbeat().await {
                    warn!("Failed to heartbeat during Google sync operation: {}", e);
                }
            }
        }
    }
}

async fn emit_drive_event_with_content(
    ctx: &SyncContext,
    user_file: &crate::models::UserFile,
    sync_run_id: &str,
    source_id: &str,
    content_id: &str,
    file_path: Option<String>,
    permissions: DocumentPermissions,
) -> bool {
    let mut event = user_file.file.to_connector_event(
        sync_run_id,
        source_id,
        content_id,
        file_path,
        Some(&user_file.user_email),
    );
    if let ConnectorEvent::DocumentCreated {
        permissions: event_permissions,
        ..
    } = &mut event
    {
        *event_permissions = permissions;
    }

    match ctx.emit_event(event).await {
        Ok(_) => true,
        Err(e) => {
            error!(
                "Failed to queue event for Drive file {} ({}): {:?}",
                user_file.file.name, user_file.file.id, e
            );
            false
        }
    }
}

async fn emit_metadata_only_drive_event(
    ctx: &SyncContext,
    user_file: &crate::models::UserFile,
    sync_run_id: &str,
    source_id: &str,
    reason: &str,
    permissions: DocumentPermissions,
) -> bool {
    let metadata_content = format!(
        "Title: {}\nMIME type: {}\nContent note: {}\n",
        user_file.file.name, user_file.file.mime_type, reason
    );

    let content_id = match ctx.store_content(&metadata_content).await {
        Ok(content_id) => content_id,
        Err(e) => {
            error!(
                "Failed to store metadata-only content for Drive file {} ({}): {}",
                user_file.file.name, user_file.file.id, e
            );
            return false;
        }
    };

    let mut event = user_file.file.to_connector_event(
        sync_run_id,
        source_id,
        &content_id,
        Some(format!("/{}", user_file.file.name)),
        Some(&user_file.user_email),
    );
    if let ConnectorEvent::DocumentCreated {
        permissions: event_permissions,
        ..
    } = &mut event
    {
        *event_permissions = permissions;
    }

    match ctx.emit_event(event).await {
        Ok(_) => true,
        Err(e) => {
            error!(
                "Failed to queue metadata-only event for Drive file {} ({}): {:?}",
                user_file.file.name, user_file.file.id, e
            );
            false
        }
    }
}

use crate::admin::AdminClient;
use crate::auth::{GoogleAuth, GoogleOAuthCredentials, OAuthAuth, google_max_retries};
use crate::cache::LruFolderCache;
use crate::chat::{
    ChatClient, GoogleChatAttachmentSource, GoogleChatMessage, GoogleChatSpace,
    GoogleChatSpaceEvent, GoogleChatSpaceType,
};
use crate::connector::build_attachment_doc_id;
use crate::drive::{DriveClient, FileContent};
use crate::gmail::{BatchThreadResult, ExtractedAttachment, GmailClient, MessageFormat};
use crate::models::{
    AttachmentPointer, GmailThread, GoogleChatSegmentCheckpoint, GoogleChatSpaceCheckpoint,
    GoogleConnectorState, GoogleDriveFile, GoogleDrivePermission, GoogleSyncCheckpoint, UserFile,
    WebhookChannel, WebhookChannelResponse, WebhookNotification, mime_type_to_content_type,
};
use omni_connector_sdk::RateLimiter;
use omni_connector_sdk::SdkClient;
use omni_connector_sdk::{
    AuthType, ConnectorEvent, DocumentAttributes, DocumentMetadata, DocumentPermissions,
    ServiceCredential, ServiceProvider, Source, SourceType, SyncType,
};
use serde_json::json;

const GOOGLE_CHAT_DEAD_TIME_SECONDS: i64 = 45 * 60;
const GOOGLE_CHAT_MAX_SEGMENT_MESSAGES: usize = 100;
const GOOGLE_CHAT_MAX_SEGMENT_BYTES: usize = 100 * 1024;
const GOOGLE_CHAT_MAX_MESSAGE_BYTES: usize = 16 * 1024;
const GOOGLE_CHAT_MAX_SEGMENT_SPAN_SECONDS: i64 = 12 * 60 * 60;
const GOOGLE_CHAT_MAX_TARGETED_INCREMENTAL_WINDOWS: usize = 20;

#[derive(Debug, Clone)]
struct GoogleChatSegmentAttachmentRef {
    name: String,
    content_name: Option<String>,
    content_type: Option<String>,
    source: Option<GoogleChatAttachmentSource>,
    resource_name: Option<String>,
    drive_file_id: Option<String>,
}

struct GoogleChatAttachmentStoredContent {
    content_id: String,
    content_extracted: bool,
    extraction_error: Option<String>,
    source_url: Option<String>,
    size: Option<String>,
}

#[derive(Debug, Clone)]
struct GoogleChatRebuildWindow {
    start: OffsetDateTime,
    end: OffsetDateTime,
    stale_segment_ids: HashSet<String>,
}

#[derive(Debug, Clone)]
struct GoogleChatSegmentBounds {
    external_id: String,
    start: OffsetDateTime,
    end: OffsetDateTime,
}

#[derive(Debug, Clone)]
struct GoogleChatIncrementalChanges {
    affected_times: Vec<OffsetDateTime>,
    requires_full_rebuild: bool,
    latest_event_time: String,
}

#[derive(Debug, Clone)]
struct GoogleChatSegmentMessage {
    name: String,
    sender: String,
    sender_email: Option<String>,
    create_time: OffsetDateTime,
    update_time: Option<OffsetDateTime>,
    text: String,
    thread_name: Option<String>,
    thread_reply: bool,
    truncated: bool,
    attachments: Vec<GoogleChatSegmentAttachmentRef>,
}

#[derive(Debug, Clone)]
struct GoogleChatSegment {
    external_id: String,
    space_name: String,
    space_id: String,
    space_display_name: Option<String>,
    messages: Vec<GoogleChatSegmentMessage>,
    truncated_message_names: Vec<String>,
    attachment_external_ids: Vec<String>,
}

impl GoogleChatSegment {
    fn render_content(&self) -> String {
        let start = self
            .messages
            .first()
            .map(|m| m.create_time.to_string())
            .unwrap_or_default();
        let end = self
            .messages
            .last()
            .map(|m| m.create_time.to_string())
            .unwrap_or_default();
        let mut out = format!(
            "Space: {}\nConversation segment: {} → {}\nMessages: {}\n\n",
            self.space_display_name
                .as_deref()
                .unwrap_or(&self.space_name),
            start,
            end,
            self.messages.len()
        );
        for msg in &self.messages {
            let reply_marker = if msg.thread_reply {
                " (thread reply)"
            } else {
                ""
            };
            out.push_str(&format!(
                "[{}] {}{}:\n",
                msg.create_time, msg.sender, reply_marker
            ));
            out.push_str(&msg.text);
            if msg.truncated {
                out.push_str("\n[message truncated]");
            }
            if !msg.attachments.is_empty() {
                out.push_str("\nAttachments:\n");
                for attachment in &msg.attachments {
                    out.push_str(&format!(
                        "- {}\n",
                        attachment
                            .content_name
                            .as_deref()
                            .unwrap_or(&attachment.name)
                    ));
                }
            }
            out.push_str("\n\n");
        }
        out
    }

    fn metadata(&self) -> DocumentMetadata {
        let start = self.messages.first().map(|m| m.create_time);
        let end = self.messages.last().map(|m| m.create_time);
        let updated = self
            .messages
            .iter()
            .filter_map(|m| m.update_time.or(Some(m.create_time)))
            .max();
        let mut extra = HashMap::new();
        extra.insert("space_name".to_string(), json!(self.space_name));
        extra.insert("space_id".to_string(), json!(self.space_id));
        extra.insert(
            "space_display_name".to_string(),
            json!(self.space_display_name),
        );
        extra.insert("segment_external_id".to_string(), json!(self.external_id));
        if let Some(first) = self.messages.first() {
            extra.insert("segment_start_message_name".to_string(), json!(first.name));
            extra.insert(
                "segment_start_time".to_string(),
                json!(first.create_time.to_string()),
            );
        }
        if let Some(last) = self.messages.last() {
            extra.insert("segment_end_message_name".to_string(), json!(last.name));
            extra.insert(
                "segment_end_time".to_string(),
                json!(last.create_time.to_string()),
            );
        }
        extra.insert("message_count".to_string(), json!(self.messages.len()));
        extra.insert(
            "message_names".to_string(),
            json!(
                self.messages
                    .iter()
                    .map(|m| m.name.clone())
                    .collect::<Vec<_>>()
            ),
        );
        extra.insert("thread_names".to_string(), json!(self.thread_names()));
        extra.insert(
            "participant_emails".to_string(),
            json!(self.participant_emails()),
        );
        extra.insert(
            "attachment_external_ids".to_string(),
            json!(self.attachment_external_ids),
        );
        extra.insert(
            "truncated_message_names".to_string(),
            json!(self.truncated_message_names),
        );
        extra.insert(
            "is_truncated".to_string(),
            json!(!self.truncated_message_names.is_empty()),
        );

        DocumentMetadata {
            title: Some(format!(
                "{}: conversation from {} to {}",
                self.space_display_name
                    .as_deref()
                    .unwrap_or(&self.space_name),
                start.map(|t| t.to_string()).unwrap_or_default(),
                end.map(|t| t.to_string()).unwrap_or_default()
            )),
            author: None,
            created_at: start,
            updated_at: updated,
            content_type: Some("chat".to_string()),
            mime_type: Some("application/x-google-chat-segment".to_string()),
            size: None,
            url: None,
            path: Some(format!(
                "/Google Chat/{}",
                self.space_display_name
                    .as_deref()
                    .unwrap_or(&self.space_name)
            )),
            extra: Some(extra),
        }
    }

    fn attributes(&self) -> DocumentAttributes {
        let mut attrs = HashMap::new();
        attrs.insert(
            "space".to_string(),
            json!(
                self.space_display_name
                    .as_deref()
                    .unwrap_or(&self.space_name)
            ),
        );
        attrs.insert("space_id".to_string(), json!(self.space_name));
        attrs.insert("threads".to_string(), json!(self.thread_names()));
        attrs.insert("participants".to_string(), json!(self.participant_emails()));
        attrs.insert("sender".to_string(), json!(self.participant_emails()));
        attrs.insert(
            "has_attachments".to_string(),
            json!(!self.attachment_external_ids.is_empty()),
        );
        if let Some(first) = self.messages.first() {
            attrs.insert(
                "date".to_string(),
                json!(format!(
                    "{:04}-{:02}-{:02}",
                    first.create_time.year(),
                    first.create_time.month() as u8,
                    first.create_time.day()
                )),
            );
        }
        attrs
    }

    fn to_checkpoint(&self) -> GoogleChatSegmentCheckpoint {
        let first = self.messages.first().expect("segment has first message");
        let last = self.messages.last().expect("segment has last message");
        GoogleChatSegmentCheckpoint {
            external_id: self.external_id.clone(),
            start_message_name: first.name.clone(),
            end_message_name: last.name.clone(),
            start_time: first.create_time.to_string(),
            end_time: last.create_time.to_string(),
            message_count: self.messages.len() as u32,
            text_bytes: self.render_content().len() as u32,
            finalized: true,
        }
    }

    fn thread_names(&self) -> Vec<String> {
        let mut values: Vec<String> = self
            .messages
            .iter()
            .filter_map(|m| m.thread_name.clone())
            .collect();
        values.sort();
        values.dedup();
        values
    }

    fn participant_emails(&self) -> Vec<String> {
        let mut values: Vec<String> = self
            .messages
            .iter()
            .filter_map(|m| m.sender_email.clone())
            .collect();
        values.sort();
        values.dedup();
        values
    }
}

struct GoogleChatSegmentBuilder {
    space_name: String,
    space_id: String,
    space_display_name: Option<String>,
    current: Vec<GoogleChatSegmentMessage>,
    truncated_message_names: Vec<String>,
    attachment_external_ids: Vec<String>,
    current_bytes: usize,
}

impl GoogleChatSegmentBuilder {
    fn new(space: &GoogleChatSpace) -> Self {
        Self {
            space_name: space.name.clone(),
            space_id: chat_space_id(&space.name).to_string(),
            space_display_name: space.display_name.clone(),
            current: Vec::new(),
            truncated_message_names: Vec::new(),
            attachment_external_ids: Vec::new(),
            current_bytes: 0,
        }
    }

    fn push(&mut self, message: GoogleChatMessage) -> Result<Vec<GoogleChatSegment>> {
        if message.private_message_viewer.is_some() {
            return Ok(Vec::new());
        }
        if message.delete_time.is_some()
            && message.text.is_none()
            && message.formatted_text.is_none()
        {
            return Ok(Vec::new());
        }
        let msg = self.convert_message(message)?;
        let mut ready = Vec::new();
        if !self.current.is_empty() && self.should_split_before(&msg) {
            if let Some(segment) = self.take_segment()? {
                ready.push(segment);
            }
        }
        self.current_bytes += msg.text.len();
        if msg.truncated {
            self.truncated_message_names.push(msg.name.clone());
        }
        for attachment in &msg.attachments {
            self.attachment_external_ids
                .push(attachment_external_id(&attachment.name));
        }
        self.current.push(msg);
        Ok(ready)
    }

    fn finish(&mut self) -> Result<Option<GoogleChatSegment>> {
        self.take_segment()
    }

    fn should_split_before(&self, next: &GoogleChatSegmentMessage) -> bool {
        let first = self.current.first().expect("current non-empty");
        let last = self.current.last().expect("current non-empty");
        let gap = next.create_time - last.create_time;
        let span = next.create_time - first.create_time;
        gap.whole_seconds() > GOOGLE_CHAT_DEAD_TIME_SECONDS
            || span.whole_seconds() > GOOGLE_CHAT_MAX_SEGMENT_SPAN_SECONDS
            || self.current.len() >= GOOGLE_CHAT_MAX_SEGMENT_MESSAGES
            || self.current_bytes + next.text.len() > GOOGLE_CHAT_MAX_SEGMENT_BYTES
    }

    fn take_segment(&mut self) -> Result<Option<GoogleChatSegment>> {
        if self.current.is_empty() {
            return Ok(None);
        }
        let first = self.current.first().expect("segment first");
        let external_id = segment_external_id(&self.space_id, &first.name);
        let segment = GoogleChatSegment {
            external_id,
            space_name: self.space_name.clone(),
            space_id: self.space_id.clone(),
            space_display_name: self.space_display_name.clone(),
            messages: std::mem::take(&mut self.current),
            truncated_message_names: std::mem::take(&mut self.truncated_message_names),
            attachment_external_ids: {
                let mut ids = std::mem::take(&mut self.attachment_external_ids);
                ids.sort();
                ids.dedup();
                ids
            },
        };
        self.current_bytes = 0;
        Ok(Some(segment))
    }

    fn convert_message(&self, message: GoogleChatMessage) -> Result<GoogleChatSegmentMessage> {
        let create_time = parse_google_time(message.create_time.as_deref())
            .with_context(|| format!("Chat message {} missing/invalid createTime", message.name))?;
        let update_time = parse_google_time(message.last_update_time.as_deref());
        let sender = message
            .sender
            .as_ref()
            .and_then(|u| u.display_name.clone())
            .unwrap_or_else(|| {
                message
                    .sender
                    .as_ref()
                    .map(|u| u.name.clone())
                    .unwrap_or_else(|| "Unknown".to_string())
            });
        let sender_email = message
            .sender
            .as_ref()
            .and_then(|u| u.name.strip_prefix("users/").map(|v| v.to_lowercase()))
            .filter(|v| v.contains('@'));
        let raw_text = message.formatted_text.or(message.text).unwrap_or_default();
        let (text, truncated) = truncate_message(raw_text, GOOGLE_CHAT_MAX_MESSAGE_BYTES);
        let attachments = message
            .attachment
            .into_iter()
            .map(|a| GoogleChatSegmentAttachmentRef {
                name: a.name,
                content_name: a.content_name,
                content_type: a.content_type,
                source: a.source,
                resource_name: a.attachment_data_ref.and_then(|r| r.resource_name),
                drive_file_id: a.drive_data_ref.map(|r| r.drive_file_id),
            })
            .collect();
        Ok(GoogleChatSegmentMessage {
            name: message.name,
            sender,
            sender_email,
            create_time,
            update_time,
            text,
            thread_name: message.thread.and_then(|t| t.name),
            thread_reply: message.thread_reply.unwrap_or(false),
            truncated,
            attachments,
        })
    }
}

fn parse_google_time(value: Option<&str>) -> Option<OffsetDateTime> {
    value.and_then(|value| {
        OffsetDateTime::parse(value, &time::format_description::well_known::Rfc3339).ok()
    })
}

fn truncate_message(mut text: String, max_bytes: usize) -> (String, bool) {
    if text.len() <= max_bytes {
        return (text, false);
    }
    let mut end = max_bytes.min(text.len());
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    text.truncate(end);
    (text, true)
}

fn chat_event_message_create_time(event: &GoogleChatSpaceEvent) -> Option<OffsetDateTime> {
    let create_time = event
        .message_created
        .as_ref()
        .or(event.message_updated.as_ref())
        .or(event.message_deleted.as_ref())?
        .message
        .create_time
        .as_deref()?;
    parse_google_time(Some(create_time))
}

fn chat_segment_bounds(segment: &GoogleChatSegmentCheckpoint) -> Result<GoogleChatSegmentBounds> {
    let start = parse_google_time(Some(&segment.start_time)).ok_or_else(|| {
        anyhow!(
            "Invalid Google Chat segment start time for {}: {}",
            segment.external_id,
            segment.start_time
        )
    })?;
    let end = parse_google_time(Some(&segment.end_time)).ok_or_else(|| {
        anyhow!(
            "Invalid Google Chat segment end time for {}: {}",
            segment.external_id,
            segment.end_time
        )
    })?;
    Ok(GoogleChatSegmentBounds {
        external_id: segment.external_id.clone(),
        start,
        end,
    })
}

fn merge_chat_rebuild_windows(
    mut windows: Vec<GoogleChatRebuildWindow>,
    max_gap: time::Duration,
) -> Vec<GoogleChatRebuildWindow> {
    windows.sort_by_key(|window| window.start);
    let mut merged: Vec<GoogleChatRebuildWindow> = Vec::new();
    for window in windows {
        let Some(last) = merged.last_mut() else {
            merged.push(window);
            continue;
        };
        if window.start <= last.end + max_gap {
            last.end = std::cmp::max(last.end, window.end);
            last.stale_segment_ids.extend(window.stale_segment_ids);
        } else {
            merged.push(window);
        }
    }
    merged
}

fn sort_chat_segment_checkpoints(segments: &mut [GoogleChatSegmentCheckpoint]) {
    segments.sort_by_key(|segment| parse_google_time(Some(&segment.start_time)));
}

fn chat_space_id(space_name: &str) -> &str {
    space_name.strip_prefix("spaces/").unwrap_or(space_name)
}

fn chat_message_id(message_name: &str) -> &str {
    message_name.rsplit('/').next().unwrap_or(message_name)
}

fn segment_external_id(space_id: &str, start_message_name: &str) -> String {
    format!(
        "google_chat_segment:{}:{}",
        space_id,
        chat_message_id(start_message_name)
    )
}

fn attachment_external_id(attachment_name: &str) -> String {
    format!("google_chat_attachment:{}", attachment_name)
}

fn chat_space_group_id(space_name: &str) -> String {
    format!("google-chat-space:{}", chat_space_id(space_name))
}

fn chat_user_email(user_name: &str, user_id_to_email: &HashMap<String, String>) -> Option<String> {
    let id = user_name
        .strip_prefix("users/")
        .unwrap_or(user_name)
        .to_lowercase();
    if id.contains('@') {
        Some(id)
    } else {
        user_id_to_email.get(&id).cloned()
    }
}

fn chat_event_watermark_expired(value: Option<&str>) -> bool {
    let Some(time) = parse_google_time(value) else {
        return true;
    };
    (OffsetDateTime::now_utc() - time).whole_days() >= 27
}

fn chat_space_allowed(source: &Source, space: &GoogleChatSpace) -> bool {
    let Some(config) = source.config.as_object() else {
        return true;
    };
    if let Some(allowlist) = config.get("space_allowlist").and_then(|v| v.as_array()) {
        if allowlist.is_empty() {
            return true;
        }
        return allowlist.iter().filter_map(|v| v.as_str()).any(|allowed| {
            allowed == space.name || space.display_name.as_deref() == Some(allowed)
        });
    }
    true
}

pub struct WebhookDebounce {
    pub last_received: Instant,
    pub last_event_type: String,
    pub count: u32,
}

pub struct SyncManager {
    drive_client: DriveClient,
    gmail_client: GmailClient,
    chat_client: ChatClient,
    admin_client: Arc<AdminClient>,
    // TODO: Remove this one we wire in the webhook codepath to use SyncContext as well
    pub sdk_client: SdkClient,
    folder_cache: LruFolderCache,
    webhook_url: Option<String>,
    pub webhook_debounce: DashMap<String, WebhookDebounce>,
    webhook_notify: Arc<Notify>,
    drive_buffer_memory_budget: Arc<Semaphore>,
    pub debounce_duration_ms: AtomicU64,
}

impl SyncManager {
    pub fn new(
        admin_client: Arc<AdminClient>,
        sdk_client: SdkClient,
        webhook_url: Option<String>,
    ) -> Self {
        // Google API Rate limits:
        //   - Drive API (list files, etc.): 12,000 req/min
        //   - Docs API (get content, etc.): 3,000 req/min/project, 300 req/min/user
        // The below rate limit is for the Drive API only.
        // For the Docs API, we need to have a separate rate limiter for each user.
        let api_rate_limit = std::env::var("GOOGLE_API_RATE_LIMIT")
            .unwrap_or_else(|_| "50".to_string())
            .parse::<u32>()
            .unwrap_or(50);

        let max_retries = google_max_retries();

        let rate_limiter = Arc::new(RateLimiter::new(api_rate_limit, max_retries));
        let drive_client = DriveClient::with_rate_limiter(rate_limiter.clone());
        let gmail_client = GmailClient::with_rate_limiter(rate_limiter.clone());
        let chat_client = ChatClient::with_rate_limiter(rate_limiter);

        let debounce_duration_ms = google_webhook_debounce_duration_ms();
        info!(
            "Google webhook debounce duration set to {} seconds",
            debounce_duration_ms / 1000
        );

        Self {
            drive_client,
            gmail_client,
            chat_client,
            admin_client,
            sdk_client,
            folder_cache: LruFolderCache::new(10_000),
            webhook_url,
            webhook_debounce: DashMap::new(),
            webhook_notify: Arc::new(Notify::new()),
            drive_buffer_memory_budget: Arc::new(Semaphore::new(GOOGLE_BUFFER_PERMITS)),
            debounce_duration_ms: AtomicU64::new(debounce_duration_ms),
        }
    }

    pub fn gmail_client(&self) -> &GmailClient {
        &self.gmail_client
    }

    /// Run a sync driven by the SDK. The SDK passes in the full Source and
    /// optional ServiceCredential, the persisted State, and a `SyncContext`
    /// whose cancellation flag is flipped by the SDK's `/cancel` handler.
    pub async fn run_sync(
        &self,
        source: Source,
        credentials: Option<ServiceCredential>,
        state: Option<GoogleSyncCheckpoint>,
        ctx: SyncContext,
    ) -> Result<()> {
        let sync_run_id = ctx.sync_run_id().to_string();
        let source_id = ctx.source_id().to_string();

        info!(
            "Starting sync for source {} (sync_run_id: {})",
            source_id, sync_run_id
        );

        let creds =
            credentials.ok_or_else(|| anyhow!("Google sync requires service credentials"))?;
        if creds.provider != ServiceProvider::Google {
            return Err(anyhow!(
                "Expected Google credentials for source {}, found {:?}",
                source_id,
                creds.provider
            ));
        }

        let outcome = self.run_sync_inner(&source, &creds, state, &ctx).await;

        match outcome {
            Ok(Some(final_state)) => {
                // Save the final state explicitly even though run_sync_inner
                // checkpoints mid-sync — the inner pass might have made
                // additional state mutations after the last checkpoint.
                let state_json = serde_json::to_value(&final_state)?;
                ctx.save_checkpoint(state_json).await?;
                ctx.complete().await?;
                Ok(())
            }
            // Cancelled mid-sync: tell the SDK so the run is marked
            // `cancelled` rather than `failed`. Returning Ok keeps the
            // SDK's default-fail branch from firing. Per-user state was
            // already checkpointed mid-sync via `ctx.save_checkpoint`.
            Ok(None) => {
                info!("Sync {} was cancelled", sync_run_id);
                ctx.cancel().await?;
                Ok(())
            }
            Err(e) => Err(e),
        }
    }

    /// Inner sync body. Returns `Ok(None)` if the sync was cancelled
    /// mid-flight (distinct from a successful completion or a hard
    /// failure). Returns `Ok(Some(state))` with the final connector state
    /// on success, which the caller persists via `ctx.complete`.
    async fn run_sync_inner(
        &self,
        source: &Source,
        service_creds: &ServiceCredential,
        existing_state: Option<GoogleSyncCheckpoint>,
        ctx: &SyncContext,
    ) -> Result<Option<GoogleSyncCheckpoint>> {
        let source_id = ctx.source_id();
        let sync_type = ctx.sync_mode();

        // Deserialize the typed source config once. Malformed known fields are a
        // hard sync error; unknown top-level keys are tolerated.
        let config: crate::models::GoogleSourceConfig =
            serde_json::from_value(source.config.clone())
                .map_err(|e| anyhow!("Invalid Google source config: {}", e))?;
        let is_sa_direct = matches!(
            config.auth_mode,
            crate::models::GoogleAuthMode::ServiceAccountDirect
        );

        // SA-direct sources have no Admin SDK access via impersonation, but the
        // SA itself may hold a customer-level admin role (the DWD-free path
        // Google supports); attempt a best-effort group sync with the SA's own
        // identity and degrade gracefully when Directory access is absent.
        let known_groups = if is_sa_direct {
            self.maybe_sync_groups_sa_direct(source, service_creds, ctx)
                .await
        } else {
            self.maybe_sync_groups(source, service_creds, ctx).await
        };

        // The SDK passes us the persisted state on each (re-)dispatch — we use
        // it directly instead of refetching via HTTP. On a fresh sync this is
        // None; on resume after a crash, this is the last checkpoint written
        // mid-sync.
        let existing_state = existing_state.unwrap_or_default();

        let native_source_type = SourceType::try_from(source.source_type.as_str())
            .map_err(|e| anyhow!("Unsupported source type: {}", e))?;

        let result = match native_source_type {
            SourceType::GoogleDrive => {
                self.sync_drive_source_internal(
                    source,
                    &config,
                    service_creds,
                    sync_type,
                    existing_state,
                    ctx,
                )
                .await
            }
            SourceType::Gmail => {
                self.sync_gmail_source_internal(
                    source,
                    service_creds,
                    sync_type,
                    existing_state,
                    known_groups,
                    ctx,
                )
                .await
            }
            SourceType::GoogleChat => {
                self.sync_google_chat_source_internal(
                    source,
                    service_creds,
                    sync_type,
                    existing_state,
                    known_groups,
                    ctx,
                )
                .await
            }
            _ => Err(anyhow!("Unsupported source type: {:?}", source.source_type)),
        };

        // SA-direct is polling-only: skip webhook registration entirely (it
        // would impersonate the source creator, which SA-direct must never do).
        if result.is_ok() && native_source_type == SourceType::GoogleDrive && !is_sa_direct {
            self.ensure_webhook_registered(source_id).await;
        }

        if ctx.is_cancelled() {
            return Ok(None);
        }

        result.map(Some)
    }

    fn get_cutoff_date(&self) -> Result<(String, String)> {
        let max_age_days = std::env::var("GOOGLE_MAX_AGE_DAYS")
            .unwrap_or_else(|_| "730".to_string())
            .parse::<i64>()
            .unwrap_or(730);

        let cutoff_date = OffsetDateTime::now_utc() - time::Duration::days(max_age_days);

        // Format for Drive API (RFC 3339): "2012-06-04T12:00:00-08:00"
        // Use UTC timezone for simplicity
        let drive_format = format!(
            "{:04}-{:02}-{:02}T00:00:00Z",
            cutoff_date.year(),
            cutoff_date.month() as u8,
            cutoff_date.day()
        );

        // Format for Gmail API: "YYYY/MM/DD"
        let gmail_format = format!(
            "{:04}/{:02}/{:02}",
            cutoff_date.year(),
            cutoff_date.month() as u8,
            cutoff_date.day()
        );

        Ok((drive_format, gmail_format))
    }

    // Narrowing or changing folder filters does not prune documents from prior
    // runs; source-wide reconciliation is tracked separately in issue #422.
    /// Check whether a file should be included based on configured folder-path filters.
    /// Uses `self.folder_cache` (the same LRU cache used by `build_full_path`) for
    /// folder-metadata lookups during ancestry resolution.
    ///
    /// Returns `Ok(true)` if no filters are configured (include-everything mode).
    /// Returns `Ok(false)` if the file's ancestry is outside the configured scope.
    /// Returns `Err(...)` if ancestry metadata could not be determined due to a
    /// transient API error — the caller should fail/retry the sync rather than
    /// silently including or excluding the file.
    async fn is_file_in_allowed_folder(
        &self,
        auth: &GoogleAuth,
        user_email: &str,
        file: &crate::models::GoogleDriveFile,
        allowed_folder_ids: &HashSet<String>,
    ) -> Result<bool> {
        if allowed_folder_ids.is_empty() {
            return Ok(true);
        }

        let folder_cache = self.folder_cache.clone();
        let drive_client = self.drive_client.clone();
        let google_auth = auth.clone();
        let user_email_owned = user_email.to_string();

        let lookup = move |folder_id: String| -> Pin<
            Box<dyn Future<Output = Result<(String, Option<String>)>> + Send>,
        > {
            let folder_cache = folder_cache.clone();
            let drive_client = drive_client.clone();
            let google_auth = google_auth.clone();
            let user_email_owned = user_email_owned.clone();
            Box::pin(async move {
                // Try the LRU folder cache first (same cache used by build_full_path).
                if let Some(cached) = folder_cache.get(&folder_id) {
                    let parent = cached.parents.as_ref().and_then(|p| p.first()).cloned();
                    return Ok((cached.name, parent));
                }
                // Cache miss: fetch from API and populate cache.
                let meta = drive_client
                    .get_folder_metadata(&google_auth, &user_email_owned, &folder_id)
                    .await
                    .with_context(|| {
                        format!(
                            "Failed to fetch folder metadata for {} while evaluating folder-path filter",
                            folder_id
                        )
                    })?;
                let name = meta.name.clone();
                let parent = meta.parents.as_ref().and_then(|p| p.first()).cloned();
                folder_cache.insert(folder_id.clone(), meta.into());
                Ok((name, parent))
            })
        };

        check_ancestry_static(&file.parents, allowed_folder_ids, &lookup).await
    }

    async fn sync_drive_for_user(
        &self,
        user_email: &str,
        service_auth: Arc<GoogleAuth>,
        source_id: &str,
        sync_run_id: &str,
        ctx: &SyncContext,
        created_after: Option<&str>,
        content_cache: Arc<DriveContentCache>,
        folder_filter_ids: Option<Arc<HashSet<String>>>,
        preserved_change_token: Option<String>,
    ) -> Result<DriveUserSyncResult> {
        info!("Processing Drive files for user: {}", user_email);

        // A token preserved from a preceding changes pass is intentionally kept
        // instead of replacing it with a later token. Changes made during this
        // crawl must be replayed on the next run.
        let next_page_token = if preserved_change_token.is_some() {
            preserved_change_token
        } else {
            match self
                .drive_client
                .get_start_page_token_for_user(service_auth.as_ref(), user_email)
                .await
            {
                Ok(token) => Some(token),
                Err(error) => {
                    warn!(
                        "Failed to capture Drive change token for user {} before full traversal: {}",
                        user_email, error
                    );
                    None
                }
            }
        };

        let mut total_scanned = 0;
        let mut total_updated = 0;
        let mut page_token: Option<String> = None;
        let mut file_batch = Vec::new();
        const BATCH_SIZE: usize = 200;

        loop {
            debug!(
                "Listing files for user {} with page_token: '{:?}'",
                user_email, page_token
            );

            let response = self
                .drive_client
                .list_files(
                    &service_auth,
                    &user_email,
                    page_token.as_deref(),
                    created_after,
                )
                .await
                .with_context(|| {
                    format!(
                        "Failed to list files for user {} (page_token: {:?})",
                        user_email, page_token
                    )
                })?;

            let page_file_count = response.files.len();
            debug!(
                "Got {} files in this page with page_token: '{:?}' for user {}",
                page_file_count, page_token, user_email
            );

            // Process files in this page. Indexer dedups by (external_id, modified_time);
            // we always emit and let the indexer skip unchanged docs.
            for file in response.files {
                if self.should_index_file(&file) {
                    // Apply folder-path filter if configured.
                    // Errors propagate upward so the sync fails and can be retried.
                    if let Some(ref allowed_ids) = folder_filter_ids {
                        if !self
                            .is_file_in_allowed_folder(
                                &service_auth,
                                user_email,
                                &file,
                                allowed_ids,
                            )
                            .await?
                        {
                            // Full traversals only index in-scope files.
                            continue;
                        }
                    }
                    file_batch.push(UserFile {
                        user_email: Arc::new(user_email.to_string()),
                        file,
                        permissions_override: None,
                    });

                    if file_batch.len() >= BATCH_SIZE {
                        let (scanned, updated) = self
                            .process_file_batch(
                                file_batch.clone(),
                                source_id,
                                sync_run_id,
                                ctx,
                                service_auth.clone(),
                                content_cache.clone(),
                            )
                            .await?;

                        total_scanned += scanned;
                        total_updated += updated;
                        file_batch.clear();
                    }
                }
            }

            // Check for cancellation
            if ctx.is_cancelled() {
                info!(
                    "Sync {} cancelled, stopping Drive sync for user {}",
                    sync_run_id, user_email
                );
                break;
            }

            // Check if there are more pages
            page_token = response.next_page_token;
            if page_token.is_none() {
                break;
            }
        }

        // Process any remaining files in the batch
        if !file_batch.is_empty() {
            let (scanned, updated) = self
                .process_file_batch(
                    file_batch,
                    source_id,
                    sync_run_id,
                    ctx,
                    service_auth.clone(),
                    content_cache.clone(),
                )
                .await?;

            total_scanned += scanned;
            total_updated += updated;
        }

        // Reconcile the user's trashed files on a full traversal (see
        // [`Self::publish_trashed_deletions`]). This function is only invoked
        // from the full-sync branch; incremental syncs go through
        // [`Self::sync_drive_for_user_incremental`] instead.
        let trashed_files = self
            .drive_client
            .list_trashed_files_for_user(&service_auth, user_email)
            .await?;
        self.publish_trashed_deletions(ctx, trashed_files).await?;

        info!(
            "Completed processing user {}: {} scanned, {} updated",
            user_email, total_scanned, total_updated
        );
        Ok(DriveUserSyncResult {
            scanned: total_scanned,
            updated: total_updated,
            next_page_token,
        })
    }

    async fn sync_drive_for_user_incremental(
        &self,
        user_email: &str,
        service_auth: Arc<GoogleAuth>,
        source_id: &str,
        sync_run_id: &str,
        ctx: &SyncContext,
        start_page_token: &str,
        content_cache: Arc<DriveContentCache>,
        folder_filter_ids: Option<Arc<HashSet<String>>>,
    ) -> Result<DriveUserSyncResult> {
        info!(
            "Processing incremental Drive sync for user {} from pageToken {}",
            user_email, start_page_token
        );

        let access_token = service_auth.get_access_token(user_email).await?;

        let mut all_changes = Vec::new();
        let mut next_page_token: Option<String> = None;
        let mut current_token = start_page_token.to_string();

        loop {
            let response = self
                .drive_client
                .list_changes(&access_token, &current_token)
                .await?;

            all_changes.extend(response.changes);
            if response.new_start_page_token.is_some() {
                next_page_token = response.new_start_page_token;
            }

            if ctx.is_cancelled() {
                info!(
                    "Sync {} cancelled during changes listing for user {}",
                    sync_run_id, user_email
                );
                break;
            }

            match response.next_page_token {
                Some(token) => current_token = token,
                None => break,
            }
        }

        info!(
            "Incremental sync found {} changes for user {}",
            all_changes.len(),
            user_email
        );

        let mut file_batch = Vec::new();
        let mut total_scanned = 0;
        let mut total_updated = 0;
        const BATCH_SIZE: usize = 200;

        for change in all_changes {
            // A change reports a file as removed either via `removed: true`
            // (permanent delete) or via the file's own `trashed: true` flag
            // (moved to trash, deleted permanently after the retention
            // window). Both mean the document must leave the index now — a
            // trashed file is no longer accessible to anyone.
            let is_removed = change.removed.unwrap_or(false)
                || change
                    .file
                    .as_ref()
                    .and_then(|f| f.trashed)
                    .unwrap_or(false);

            if is_removed {
                if let Some(file_id) = &change.file_id {
                    info!(
                        "File {} was removed (incremental), publishing deletion",
                        file_id
                    );
                    self.publish_deletion_event(ctx, file_id).await?;
                }
                continue;
            }

            if let Some(file) = change.file {
                if file.mime_type == FOLDER_MIME_TYPE {
                    // Drive does not reliably emit changes for every descendant
                    // when a folder moves. Defer subtree pruning to source-wide
                    // reconciliation rather than guessing from one folder event.
                    continue;
                }
                if !self.should_index_file(&file) {
                    continue;
                }

                // Apply folder-path filter if configured.
                // Errors propagate upward so the sync fails and can be retried.
                if let Some(ref allowed_ids) = folder_filter_ids {
                    if !self
                        .is_file_in_allowed_folder(&service_auth, user_email, &file, allowed_ids)
                        .await?
                    {
                        debug!(
                            "File {} ({}) moved outside configured folder scope; publishing deletion",
                            file.name, file.id
                        );
                        self.publish_deletion_event(ctx, &file.id).await?;
                        continue;
                    }
                }

                file_batch.push(UserFile {
                    user_email: Arc::new(user_email.to_string()),
                    file,
                    permissions_override: None,
                });

                if file_batch.len() >= BATCH_SIZE {
                    let (scanned, updated) = self
                        .process_file_batch(
                            file_batch.clone(),
                            source_id,
                            sync_run_id,
                            ctx,
                            service_auth.clone(),
                            content_cache.clone(),
                        )
                        .await?;
                    total_scanned += scanned;
                    total_updated += updated;
                    file_batch.clear();
                }
            }
        }

        if !file_batch.is_empty() {
            let (scanned, updated) = self
                .process_file_batch(
                    file_batch,
                    source_id,
                    sync_run_id,
                    ctx,
                    service_auth.clone(),
                    content_cache.clone(),
                )
                .await?;
            total_scanned += scanned;
            total_updated += updated;
        }

        info!(
            "Completed incremental Drive sync for user {}: {} scanned, {} updated",
            user_email, total_scanned, total_updated
        );
        let next_page_token = if ctx.is_cancelled() {
            None
        } else {
            // If Google omitted newStartPageToken, retain the old watermark and
            // replay the changes next run rather than taking a fresh token that
            // could skip changes made while this run was processing.
            Some(next_page_token.unwrap_or_else(|| start_page_token.to_string()))
        };
        Ok(DriveUserSyncResult {
            scanned: total_scanned,
            updated: total_updated,
            next_page_token,
        })
    }

    async fn process_file_batch(
        &self,
        files: Vec<UserFile>,
        source_id: &str,
        sync_run_id: &str,
        ctx: &SyncContext,
        service_auth: Arc<GoogleAuth>,
        content_cache: Arc<DriveContentCache>,
    ) -> Result<(usize, usize)> {
        info!("Processing batch of {} files", files.len());

        // (scanned, updated): scanned counts files we read content from
        // (regardless of store/emit outcome); updated counts files we
        // successfully emitted as events.
        let mut scanned = 0;
        let mut updated = 0;

        let sync_run_id_owned = sync_run_id.to_string();
        let source_id_owned = source_id.to_string();

        let tasks = files.into_iter().map(|user_file| {
            let service_auth = service_auth.clone();
            let source_id = source_id_owned.clone();
            let sync_run_id = sync_run_id_owned.clone();
            let drive_client = self.drive_client.clone();
            let memory_budget = self.drive_buffer_memory_budget.clone();
            let content_cache = content_cache.clone();
            let ctx = ctx.clone();

            async move {
                if ctx.is_cancelled() {
                    return (0, 0);
                }

                debug!(
                    "Processing file: {} ({}) for user: {}",
                    user_file.file.name, user_file.file.id, user_file.user_email
                );

                let file_lock = content_cache.lock_for_file(&user_file.file.id);
                let _file_guard = file_lock.lock().await;
                // Shared-drive syncs carry a computed effective ACL override;
                // use it as-is because the file's own permissions array is
                // empty for shared-drive items. Otherwise derive permissions
                // from the file and syncing user as before.
                let current_permissions = match &user_file.permissions_override {
                    Some(override_permissions) => override_permissions.clone(),
                    None => user_file
                        .file
                        .to_document_permissions(Some(&user_file.user_email)),
                };
                let merged_permissions =
                    content_cache.merge_permissions(&user_file.file.id, current_permissions);

                if let Some(content_id) = content_cache.get_content_id(&user_file.file.id) {
                    debug!(
                        "Reusing cached content_id {} for Drive file {} ({}) after waiting for in-flight extraction",
                        content_id, user_file.file.name, user_file.file.id
                    );
                    let file_path = match self
                        .resolve_file_path(&service_auth, &user_file.user_email, &user_file.file)
                        .await
                    {
                        Ok(path) => Some(path),
                        Err(e) => {
                            warn!(
                                "Failed to resolve path for cached file {}: {}",
                                user_file.file.name, e
                            );
                            Some(format!("/{}", user_file.file.name))
                        }
                    };
                    let emitted = emit_drive_event_with_content(
                        &ctx,
                        &user_file,
                        &sync_run_id,
                        &source_id,
                        &content_id,
                        file_path,
                        merged_permissions.clone(),
                    )
                    .await;
                    return (1, usize::from(emitted));
                }

                let reserved_bytes = estimated_file_size_bytes(&user_file.file);
                let max_download_bytes = google_drive_max_download_bytes();
                let reserved_permits = match reserved_bytes {
                    Some(size) if size > max_download_bytes => {
                        warn!(
                            "Indexing metadata only for Drive file {} ({}) because its declared size {} bytes exceeds the {} byte download limit",
                            user_file.file.name,
                            user_file.file.id,
                            size,
                            max_download_bytes
                        );
                        let reason = format!(
                            "File content was not indexed because declared size {} bytes exceeds the {} byte download limit.",
                            size, max_download_bytes
                        );
                        let emitted = emit_metadata_only_drive_event(
                            &ctx,
                            &user_file,
                            &sync_run_id,
                            &source_id,
                            &reason,
                            merged_permissions.clone(),
                        )
                        .await;
                        return (1, usize::from(emitted));
                    }
                    Some(size) if size > GOOGLE_MAX_BUFFERED_BYTES => {
                        warn!(
                            "Indexing metadata only for Drive file {} ({}) because its declared size {} bytes exceeds the {} byte buffer budget",
                            user_file.file.name,
                            user_file.file.id,
                            size,
                            GOOGLE_MAX_BUFFERED_BYTES
                        );
                        let reason = format!(
                            "File content was not indexed because declared size {} bytes exceeds the {} byte in-memory buffer budget.",
                            size, GOOGLE_MAX_BUFFERED_BYTES
                        );
                        let emitted = emit_metadata_only_drive_event(
                            &ctx,
                            &user_file,
                            &sync_run_id,
                            &source_id,
                            &reason,
                            merged_permissions.clone(),
                        )
                        .await;
                        return (1, usize::from(emitted));
                    }
                    Some(size) => permits_for_bytes(size),
                    // Native Google Workspace files usually do not expose byte size. Reserve the
                    // configured download cap for those so they can still run concurrently; keep
                    // the full-budget reservation for unknown-size uploaded files.
                    None => permits_for_unknown_size_file(&user_file.file),
                };

                let buffer_permit: Option<OwnedSemaphorePermit> = if reserved_permits > 0 {
                    match memory_budget.clone().acquire_many_owned(reserved_permits).await {
                        Ok(permit) => Some(permit),
                        Err(e) => {
                            error!(
                                "Drive buffer memory semaphore closed while processing file {} ({}): {:?}",
                                user_file.file.name, user_file.file.id, e
                            );
                            return (1, 0);
                        }
                    }
                } else {
                    None
                };

                let result = await_with_heartbeat(
                    &ctx,
                    drive_client.get_file_content(
                        &service_auth,
                        &user_file.user_email,
                        &user_file.file,
                    ),
                )
                .await
                .with_context(|| {
                    format!(
                        "Getting content for file {} ({})",
                        user_file.file.name, user_file.file.id
                    )
                });

                match result {
                    Ok(file_content) => {
                        let actual_size = file_content_len(&file_content);
                        if actual_size > GOOGLE_MAX_BUFFERED_BYTES {
                            warn!(
                                "Indexing metadata only for Drive file {} ({}) because buffered content is {} bytes, exceeding the {} byte budget",
                                user_file.file.name,
                                user_file.file.id,
                                actual_size,
                                GOOGLE_MAX_BUFFERED_BYTES
                            );
                            let reason = format!(
                                "File content was not indexed because extracted content size {} bytes exceeds the {} byte in-memory buffer budget.",
                                actual_size, GOOGLE_MAX_BUFFERED_BYTES
                            );
                            let emitted = emit_metadata_only_drive_event(
                                &ctx,
                                &user_file,
                                &sync_run_id,
                                &source_id,
                                &reason,
                                merged_permissions.clone(),
                            )
                            .await;
                            return (1, usize::from(emitted));
                        }

                        let actual_permits = permits_for_bytes(actual_size);
                        let extra_permits = actual_permits.saturating_sub(reserved_permits);
                        let extra_buffer_permit: Option<OwnedSemaphorePermit> =
                            if extra_permits > 0 {
                                warn!(
                                    "Drive file {} ({}) buffered content is {} bytes, exceeding reserved size {:?}; acquiring {} additional buffer permits",
                                    user_file.file.name,
                                    user_file.file.id,
                                    actual_size,
                                    reserved_bytes,
                                    extra_permits
                                );
                                match memory_budget.clone().acquire_many_owned(extra_permits).await {
                                    Ok(permit) => Some(permit),
                                    Err(e) => {
                                        error!(
                                            "Drive buffer memory semaphore closed while acquiring extra permits for file {} ({}): {:?}",
                                            user_file.file.name, user_file.file.id, e
                                        );
                                        let reason = "File content was not indexed because the connector could not reserve additional buffer memory after content extraction.";
                                        let emitted = emit_metadata_only_drive_event(
                                            &ctx,
                                            &user_file,
                                            &sync_run_id,
                                            &source_id,
                                            reason,
                                            merged_permissions.clone(),
                                        )
                                        .await;
                                        return (1, usize::from(emitted));
                                    }
                                }
                            } else {
                                None
                            };

                        if ctx.is_cancelled() {
                            return (1, 0);
                        }

                        // Keep the pre-download permit alive until content has been
                        // stored/extracted and the corresponding event has been emitted.
                        // Dropping this value releases the connector-wide memory budget via RAII.
                        let _buffer_permit = buffer_permit;
                        let _extra_buffer_permit = extra_buffer_permit;
                        debug!(
                            "Drive file {} ({}) holds {} pre-download buffer permits for {} bytes",
                            user_file.file.name,
                            user_file.file.id,
                            reserved_permits,
                            actual_size
                        );

                        let store_result = match file_content {
                            FileContent::Text(ref text) if text.is_empty() => {
                                debug!(
                                    "File {} has empty content, indexing metadata only",
                                    user_file.file.name
                                );
                                let emitted = emit_metadata_only_drive_event(
                                    &ctx,
                                    &user_file,
                                    &sync_run_id,
                                    &source_id,
                                    "File content was empty or unsupported, so only metadata was indexed.",
                                    merged_permissions.clone(),
                                )
                                .await;
                                return (1, usize::from(emitted));
                            }
                            FileContent::Text(text) => ctx.store_content(&text).await,
                            FileContent::Binary {
                                data,
                                mime_type,
                                filename,
                            } => {
                                ctx.extract_and_store_content(data, &mime_type, Some(&filename))
                                    .await
                            }
                        };
                        match store_result {
                            Ok(content_id) => {
                                content_cache.insert_content_id(&user_file.file.id, content_id.clone());
                                let file_path = match self
                                    .resolve_file_path(
                                        &service_auth,
                                        &user_file.user_email,
                                        &user_file.file,
                                    )
                                    .await
                                {
                                    Ok(path) => Some(path),
                                    Err(e) => {
                                        warn!(
                                            "Failed to resolve path for file {}: {}",
                                            user_file.file.name, e
                                        );
                                        Some(format!("/{}", user_file.file.name))
                                    }
                                };

                                let emitted = emit_drive_event_with_content(
                                    &ctx,
                                    &user_file,
                                    &sync_run_id,
                                    &source_id,
                                    &content_id,
                                    file_path,
                                    merged_permissions.clone(),
                                )
                                .await;
                                (1, usize::from(emitted))
                            }
                            Err(e) => {
                                warn!(
                                    "Failed to store extracted content for Drive file {} ({}), indexing metadata only: {}",
                                    user_file.file.name, user_file.file.id, e
                                );
                                let reason = format!(
                                    "File content was not indexed because extraction or content storage failed: {}",
                                    e
                                );
                                let emitted = emit_metadata_only_drive_event(
                                    &ctx,
                                    &user_file,
                                    &sync_run_id,
                                    &source_id,
                                    &reason,
                                    merged_permissions.clone(),
                                )
                                .await;
                                (1, usize::from(emitted))
                            }
                        }
                    }
                    Err(e) => {
                        warn!(
                            "Failed to get content for file {} ({}), indexing metadata only: {:?}",
                            user_file.file.name, user_file.file.id, e
                        );
                        let reason = format!(
                            "File content was not indexed because content extraction failed: {}",
                            e
                        );
                        let emitted = emit_metadata_only_drive_event(
                            &ctx,
                            &user_file,
                            &sync_run_id,
                            &source_id,
                            &reason,
                            merged_permissions.clone(),
                        )
                        .await;
                        (1, usize::from(emitted))
                    }
                }
            }
        });

        debug!(
            "Processing Drive file batch with concurrency {}",
            GOOGLE_FILE_CONCURRENCY
        );
        let mut results = stream::iter(tasks).buffer_unordered(GOOGLE_FILE_CONCURRENCY);
        while let Some((s, u)) = results.next().await {
            scanned += s;
            updated += u;
        }

        // Push counts to the manager. Note: counts can over-count on resume
        // since save_checkpoint only fires per-user; an in-flight batch
        // re-runs after crash. Counts are advisory progress, not exact.
        if scanned > 0 {
            ctx.increment_scanned(scanned as i32).await?;
        }
        if updated > 0 {
            ctx.increment_updated(updated as i32).await?;
        }

        info!(
            "Batch processing complete: {} scanned, {} updated",
            scanned, updated
        );
        Ok((scanned, updated))
    }

    /// Build a scope-aware Drive checkpoint that preserves Gmail and Chat state.
    fn build_drive_checkpoint(
        &self,
        existing: &GoogleSyncCheckpoint,
        drive_page_tokens: Option<HashMap<String, String>>,
        drive_change_tokens: Option<HashMap<String, String>>,
        drive_scope_fingerprint: Option<String>,
        drive_acl_fingerprints: Option<HashMap<String, String>>,
        folder_access: Option<HashMap<String, crate::models::FolderAccessInfo>>,
        permission_model_version: Option<String>,
    ) -> GoogleSyncCheckpoint {
        GoogleSyncCheckpoint {
            gmail_history_ids: existing.gmail_history_ids.clone(),
            drive_page_tokens,
            drive_change_tokens,
            drive_scope_fingerprint,
            drive_acl_fingerprints,
            folder_access,
            permission_model_version,
            chat: existing.chat.clone(),
        }
    }

    /// Helper: apply the date cutoff to a single Google Drive file.
    /// Returns `true` if the file passes the cutoff (or no cutoff is set).
    /// Folders are never filtered by the cutoff — they must be discovered
    /// regardless of age so their contents can be indexed.
    #[allow(dead_code)]
    fn pass_cutoff(file: &crate::models::GoogleDriveFile, cutoff: OffsetDateTime) -> bool {
        if file.mime_type == "application/vnd.google-apps.folder" {
            return true;
        }
        file.modified_time
            .as_deref()
            .and_then(|value| parse_google_time(Some(value)))
            .is_some_and(|modified| modified > cutoff)
            || file
                .created_time
                .as_deref()
                .and_then(|value| parse_google_time(Some(value)))
                .is_some_and(|created| created > cutoff)
    }

    /// Flush a batch of user files through `process_file_batch` and update totals.
    async fn flush_batch(
        &self,
        batch: &mut Vec<UserFile>,
        source_id: &str,
        sync_run_id: &str,
        ctx: &SyncContext,
        service_auth: Arc<GoogleAuth>,
        content_cache: Arc<DriveContentCache>,
        total_scanned: &mut usize,
        total_updated: &mut usize,
    ) -> Result<()> {
        if batch.is_empty() {
            return Ok(());
        }
        let (s, u) = self
            .process_file_batch(
                std::mem::take(batch),
                source_id,
                sync_run_id,
                ctx,
                service_auth,
                content_cache,
            )
            .await?;
        *total_scanned += s;
        *total_updated += u;
        Ok(())
    }

    /// Fetch and fingerprint the shared-drive ACLs used by both service-account
    /// direct sync and impersonated DWD sync. The caller controls whether a
    /// technical service-account member is excluded from the mapped document ACL.
    async fn load_shared_drive_acl_state(
        &self,
        access_token: &str,
        drives: &[DriveGroup],
        old_acl_fingerprints: &HashMap<String, String>,
        exclude_email: Option<&str>,
    ) -> Result<SharedDriveAclState> {
        let mut state = SharedDriveAclState::default();
        for drive_group in drives {
            let drive_id = &drive_group.drive_id;
            let drive_acl = self
                .drive_client
                .list_drive_permissions(access_token, drive_id)
                .await
                .with_context(|| format!("Failed to read ACLs for drive {}", drive_id))?;

            let fingerprint = crate::models::drive_acl_fingerprint(&drive_acl);
            let changed = old_acl_fingerprints.get(drive_id) != Some(&fingerprint);
            if changed {
                info!(
                    "ACL fingerprint changed for drive {} — forcing full re-traversal",
                    drive_id
                );
            }
            state
                .drive_acl_fingerprints
                .insert(drive_id.clone(), fingerprint);
            state.drive_acl_overrides.insert(
                drive_id.clone(),
                crate::models::map_drive_permissions(&drive_acl, exclude_email),
            );
            state.drive_acl_raw.insert(drive_id.clone(), drive_acl);
            state.drive_acl_changed.insert(drive_id.clone(), changed);
        }
        Ok(state)
    }

    fn record_folder_access(
        folder_access: &mut HashMap<String, crate::models::FolderAccessEntry>,
        folder: &GoogleDriveFile,
    ) {
        let entry = folder_access.entry(folder.id.clone()).or_default();
        entry.parents = folder.parents.clone().unwrap_or_default();
        entry.inherited_permissions_disabled =
            folder.inherited_permissions_disabled.unwrap_or(false);
    }

    /// Fetch directly-assigned ACLs for folders with bounded concurrency.
    /// SA-direct preserves its historical best-effort behavior while scoped DWD
    /// fails closed because it cannot safely model a selected subtree without
    /// its folder ACLs.
    async fn load_folder_acls(
        &self,
        access_token: &str,
        folder_ids: Vec<String>,
        folder_access: &mut HashMap<String, crate::models::FolderAccessEntry>,
        strict: bool,
    ) -> Result<()> {
        let client = self.drive_client.clone();
        let token = access_token.to_string();
        let semaphore = Arc::new(Semaphore::new(GOOGLE_FILE_CONCURRENCY));
        let mut tasks = tokio::task::JoinSet::new();
        for folder_id in folder_ids {
            let client = client.clone();
            let token = token.clone();
            let permit = semaphore.clone();
            tasks.spawn(async move {
                let _guard = permit.acquire_owned().await;
                let result = client.list_drive_permissions(&token, &folder_id).await;
                (folder_id, result)
            });
        }
        while let Some(joined) = tasks.join_next().await {
            let (folder_id, result) =
                joined.map_err(|error| anyhow!("Folder ACL fetch task failed: {}", error))?;
            match result {
                Ok(raw) => {
                    if let Some(entry) = folder_access.get_mut(&folder_id) {
                        entry.acl = Some(crate::models::filter_direct_permissions(&raw));
                    }
                }
                Err(error) if strict => {
                    return Err(error)
                        .with_context(|| format!("Failed to read ACL for folder {}", folder_id));
                }
                Err(error) => {
                    warn!("Failed to read ACLs for folder {}: {}", folder_id, error);
                }
            }
        }
        Ok(())
    }

    fn record_scoped_folder_access(
        folder_access: &mut HashMap<String, crate::models::FolderAccessEntry>,
        folder_drive_ids: &mut HashMap<String, String>,
        folder: &GoogleDriveFile,
        drive_id: &str,
    ) {
        Self::record_folder_access(folder_access, folder);
        folder_drive_ids.insert(folder.id.clone(), drive_id.to_string());
    }

    /// Load metadata and directly assigned ACLs from `folder_id` up to the
    /// shared-drive root. The scoped traversal needs this for a selected folder
    /// itself and for changed files whose ordinary ancestors are intentionally
    /// absent from the compact persisted ACL checkpoint.
    async fn ensure_scoped_folder_acl_chain(
        &self,
        service_auth: &GoogleAuth,
        principal_email: &str,
        access_token: &str,
        folder_id: Option<String>,
        drive_id: &str,
        folder_access: &mut HashMap<String, crate::models::FolderAccessEntry>,
        folder_drive_ids: &mut HashMap<String, String>,
    ) -> Result<()> {
        let mut current_id = folder_id;
        while let Some(id) = current_id {
            if id == drive_id {
                break;
            }

            if !folder_access.contains_key(&id) {
                let folder = self
                    .drive_client
                    .get_folder_metadata(service_auth, principal_email, &id)
                    .await
                    .with_context(|| format!("Failed to read metadata for folder {}", id))?;
                Self::record_scoped_folder_access(
                    folder_access,
                    folder_drive_ids,
                    &folder,
                    drive_id,
                );
            }

            let needs_acl = folder_access
                .get(&id)
                .is_some_and(|entry| entry.acl.is_none());
            if needs_acl {
                let raw = self
                    .drive_client
                    .list_drive_permissions(access_token, &id)
                    .await
                    .with_context(|| format!("Failed to read ACL for folder {}", id))?;
                if let Some(entry) = folder_access.get_mut(&id) {
                    entry.acl = Some(crate::models::filter_direct_permissions(&raw));
                }
            }

            folder_drive_ids.insert(id.clone(), drive_id.to_string());
            current_id = folder_access
                .get(&id)
                .and_then(|entry| entry.parents.first())
                .cloned();
        }
        Ok(())
    }

    /// Populate the complete folder hierarchy and direct ACL map before files
    /// from an entire shared drive are emitted. `files.list` ordering is
    /// arbitrary, so files cannot safely be permissioned until their ancestor
    /// metadata is available.
    async fn load_scoped_drive_folder_access(
        &self,
        service_auth: &GoogleAuth,
        principal_email: &str,
        access_token: &str,
        drive_id: &str,
        folder_access: &mut HashMap<String, crate::models::FolderAccessEntry>,
        folder_drive_ids: &mut HashMap<String, String>,
    ) -> Result<()> {
        let mut page_token: Option<String> = None;
        loop {
            let response = self
                .drive_client
                .list_folders_in_drive(
                    service_auth,
                    principal_email,
                    drive_id,
                    page_token.as_deref(),
                )
                .await?;
            for folder in response.files {
                Self::record_scoped_folder_access(
                    folder_access,
                    folder_drive_ids,
                    &folder,
                    drive_id,
                );
            }
            page_token = response.next_page_token;
            if page_token.is_none() {
                break;
            }
        }

        let pending: Vec<String> = folder_drive_ids
            .iter()
            .filter(|(_, entry_drive_id)| entry_drive_id.as_str() == drive_id)
            .filter_map(|(folder_id, _)| {
                folder_access
                    .get(folder_id)
                    .is_some_and(|entry| entry.acl.is_none())
                    .then(|| folder_id.clone())
            })
            .collect();
        self.load_folder_acls(access_token, pending, folder_access, true)
            .await
    }

    /// Scoped full sync: crawl selected shared drives and/or folder subtrees only,
    /// using the configured DWD principal (no user impersonation/iteration).
    async fn sync_drive_scoped(
        &self,
        source: &Source,
        service_creds: &ServiceCredential,
        sync_type: SyncType,
        existing_state: &GoogleSyncCheckpoint,
        ctx: &SyncContext,
    ) -> Result<GoogleSyncCheckpoint> {
        let sync_run_id = ctx.sync_run_id();
        let native_source_type = SourceType::try_from(source.source_type.as_str())
            .map_err(|e| anyhow!("Unsupported source type: {}", e))?;
        let service_auth = Arc::new(self.create_auth(service_creds, native_source_type).await?);

        // Parse filters; will error on malformed config.
        let filter_entries = match crate::models::parse_folder_path_filters(&source.config) {
            Ok(Some(f)) => f,
            Ok(None) => {
                // Should not happen — caller guards this branch — but be safe.
                return Err(anyhow!("sync_drive_scoped called without filters"));
            }
            Err(e) => {
                return Err(anyhow!("Invalid folder path filter configuration: {}", e));
            }
        };

        let current_fingerprint = crate::models::compute_scope_fingerprint(Some(&filter_entries));

        // Detect scope transition. If the fingerprint differs from the
        // checkpoint, force a full sync regardless of requested mode.
        let stored_fingerprint = existing_state.drive_scope_fingerprint.as_deref();
        let scope_changed = stored_fingerprint != Some(&current_fingerprint);

        if scope_changed {
            info!(
                "Drive scope transition detected: {:?} -> {} — forcing full sync",
                stored_fingerprint, current_fingerprint
            );
        }
        let effective_full =
            drive_scope_requires_full_sync(sync_type, stored_fingerprint, &current_fingerprint);

        // Build drive groups: driveId → (selected folder IDs, whether entire drive is selected)
        let drives = build_drive_groups(&filter_entries);

        if drives.is_empty() {
            info!("No valid drive scopes found — indexing nothing");
            return Ok(self.build_drive_checkpoint(
                existing_state,
                None,
                None,
                Some(current_fingerprint),
                existing_state.drive_acl_fingerprints.clone(),
                existing_state.folder_access.clone(),
                existing_state.permission_model_version.clone(),
            ));
        }

        // Resolve the delegated principal email.
        let principal_email = service_creds
            .principal_email
            .as_deref()
            .ok_or_else(|| anyhow!("Missing principal_email for scoped Drive sync"))?
            .to_string();

        // Shared-drive operations need a single access token (no impersonation per user).
        let access_token = service_auth.get_access_token(&principal_email).await?;

        // Capture watermarks before ACL preflight. If a membership change
        // forces a full traversal, changes made during that traversal must be
        // replayed from a token that predates the preflight.
        let mut full_traversal_start_tokens: HashMap<String, Option<String>> = HashMap::new();
        for drive_group in &drives {
            let token = match self
                .drive_client
                .get_start_page_token_for_drive(&access_token, &drive_group.drive_id)
                .await
            {
                Ok(token) => Some(token),
                Err(error) => {
                    warn!(
                        "Failed to get start page token for drive {}: {}",
                        drive_group.drive_id, error
                    );
                    None
                }
            };
            full_traversal_start_tokens.insert(drive_group.drive_id.clone(), token);
        }

        // Shared-drive file list responses do not contain their effective ACL.
        // Resolve the drive member list explicitly and fingerprint it so a
        // membership change re-emits unchanged documents with corrected ACLs.
        let old_acl_fingerprints = existing_state
            .drive_acl_fingerprints
            .clone()
            .unwrap_or_default();
        let service_account_email = match service_auth.as_ref() {
            GoogleAuth::ServiceAccount(service_account) => Some(service_account.client_email()),
            GoogleAuth::OAuth(_) => None,
        };
        let acl_state = self
            .load_shared_drive_acl_state(
                &access_token,
                &drives,
                &old_acl_fingerprints,
                service_account_email,
            )
            .await?;
        for (drive_id, drive_acl) in &acl_state.drive_acl_raw {
            if drive_acl.is_empty() {
                return Err(anyhow!(
                    "Drive {} returned an empty permission list; refusing to index shared-drive documents without a complete ACL",
                    drive_id
                ));
            }
        }
        let SharedDriveAclState {
            drive_acl_fingerprints: new_acl_fingerprints,
            drive_acl_overrides,
            drive_acl_raw,
            drive_acl_changed,
        } = acl_state;

        let stored_folder_access = existing_state.folder_access.clone().unwrap_or_default();
        let mut folder_access: HashMap<String, crate::models::FolderAccessEntry> =
            stored_folder_access
                .iter()
                .map(|(folder_id, info)| {
                    (
                        folder_id.clone(),
                        crate::models::FolderAccessEntry {
                            parents: info.parents.clone(),
                            inherited_permissions_disabled: info.inherited_permissions_disabled,
                            acl: None,
                        },
                    )
                })
                .collect();
        let mut folder_drive_ids: HashMap<String, String> = stored_folder_access
            .iter()
            .map(|(folder_id, info)| (folder_id.clone(), info.drive_id.clone()))
            .collect();
        let mut completed_drives: HashSet<String> = HashSet::new();

        let content_cache = Arc::new(DriveContentCache::default());
        let mut total_scanned = 0;
        let mut total_updated = 0;
        let mut new_change_tokens: HashMap<String, String> = HashMap::new();
        let old_change_tokens = existing_state
            .drive_change_tokens
            .clone()
            .unwrap_or_default();
        const BATCH_SIZE: usize = 200;

        for drive_group in &drives {
            if ctx.is_cancelled() {
                info!("Sync {} cancelled during drive-scoped sync", sync_run_id);
                break;
            }

            info!(
                "Processing drive {} (entire={}, folders={:?})",
                drive_group.drive_id, drive_group.entire_drive, drive_group.folder_ids
            );

            let drive_id = &drive_group.drive_id;
            let drive_permissions = drive_acl_overrides
                .get(drive_id)
                .cloned()
                .expect("drive ACL resolved in preflight");
            let raw_drive_acl = drive_acl_raw
                .get(drive_id)
                .expect("drive ACL resolved in preflight");
            let acl_changed = drive_acl_changed.get(drive_id).copied().unwrap_or(true);

            // Permission changes on a folder do not update every descendant in
            // changes.list. Re-check every persisted limited/direct-grant folder
            // and force a complete selected-scope traversal if one changed.
            let mut folder_acl_changed = false;
            if sync_type == SyncType::Incremental && !scope_changed && !acl_changed {
                for (folder_id, info) in &stored_folder_access {
                    if info.drive_id != *drive_id {
                        continue;
                    }
                    let raw = match self
                        .drive_client
                        .list_drive_permissions(&access_token, folder_id)
                        .await
                    {
                        Ok(raw) => raw,
                        Err(error) => {
                            warn!(
                                "Folder {} ACL fetch failed ({}); forcing full re-traversal of drive {}",
                                folder_id, error, drive_id
                            );
                            folder_acl_changed = true;
                            break;
                        }
                    };
                    let direct = crate::models::filter_direct_permissions(&raw);
                    if crate::models::drive_acl_fingerprint(&direct) != info.acl_fingerprint {
                        info!(
                            "ACL fingerprint changed for folder {} — forcing full re-traversal of drive {}",
                            folder_id, drive_id
                        );
                        folder_acl_changed = true;
                        break;
                    }
                    if let Some(entry) = folder_access.get_mut(folder_id) {
                        entry.acl = Some(direct);
                    }
                }
            }

            let model_mismatch = existing_state.permission_model_version.as_deref()
                != Some(SCOPED_DWD_PERMISSION_MODEL_VERSION);
            let stored_token = old_change_tokens.get(drive_id);
            let use_incremental = !effective_full
                && !acl_changed
                && !folder_acl_changed
                && !model_mismatch
                && stored_token.is_some();

            if use_incremental {
                // Incremental per-drive changes path. Use newStartPageToken
                // from the API response to advance the token.
                let start_token = stored_token.unwrap();
                let mut current_token = start_token.clone();

                loop {
                    let response = self
                        .drive_client
                        .list_changes_for_drive(
                            &access_token,
                            &current_token,
                            &drive_group.drive_id,
                        )
                        .await?;

                    if ctx.is_cancelled() {
                        break;
                    }

                    let mut file_batch: Vec<UserFile> = Vec::new();

                    for change in &response.changes {
                        // removed:true (permanent delete) or file.trashed:true
                        // (in trash) both mean the doc must leave the index.
                        let is_removed = change.removed.unwrap_or(false)
                            || change
                                .file
                                .as_ref()
                                .and_then(|f| f.trashed)
                                .unwrap_or(false);

                        if is_removed {
                            if let Some(file_id) = &change.file_id {
                                info!(
                                    "File {} was removed (scoped incremental), publishing deletion",
                                    file_id
                                );
                                self.publish_deletion_event(ctx, file_id).await?;
                            }
                            continue;
                        }

                        if let Some(file) = &change.file {
                            if file.mime_type == FOLDER_MIME_TYPE {
                                // Drive does not reliably emit changes for every
                                // descendant when a folder moves. Defer subtree
                                // pruning to source-wide reconciliation.
                                continue;
                            }
                            if !self.should_index_file(file) {
                                continue;
                            }

                            // For folder-only selections, ancestry-check the change.
                            if !drive_group.entire_drive {
                                let allowed: HashSet<String> =
                                    drive_group.folder_ids.iter().cloned().collect();
                                if !self
                                    .is_file_in_allowed_folder(
                                        &service_auth,
                                        &principal_email,
                                        file,
                                        &allowed,
                                    )
                                    .await?
                                {
                                    debug!(
                                        "File {} ({}) moved outside configured folder scope; publishing deletion",
                                        file.name, file.id
                                    );
                                    self.publish_deletion_event(ctx, &file.id).await?;
                                    continue;
                                }
                            }

                            self.ensure_scoped_folder_acl_chain(
                                service_auth.as_ref(),
                                &principal_email,
                                &access_token,
                                file.parents
                                    .as_ref()
                                    .and_then(|parents| parents.first())
                                    .cloned(),
                                drive_id,
                                &mut folder_access,
                                &mut folder_drive_ids,
                            )
                            .await?;
                            let effective_permissions =
                                crate::models::compute_effective_permissions(
                                    file,
                                    drive_id,
                                    &drive_permissions,
                                    raw_drive_acl,
                                    &folder_access,
                                    None,
                                );
                            file_batch.push(UserFile {
                                user_email: Arc::new(principal_email.clone()),
                                file: file.clone(),
                                permissions_override: Some(effective_permissions),
                            });

                            if file_batch.len() >= BATCH_SIZE {
                                self.flush_batch(
                                    &mut file_batch,
                                    &source.id,
                                    &sync_run_id,
                                    ctx,
                                    service_auth.clone(),
                                    content_cache.clone(),
                                    &mut total_scanned,
                                    &mut total_updated,
                                )
                                .await?;
                            }
                        }
                    }

                    // Flush remaining changes for this page.
                    self.flush_batch(
                        &mut file_batch,
                        &source.id,
                        &sync_run_id,
                        ctx,
                        service_auth.clone(),
                        content_cache.clone(),
                        &mut total_scanned,
                        &mut total_updated,
                    )
                    .await?;

                    // Use newStartPageToken from response if present.
                    if let Some(new_token) = &response.new_start_page_token {
                        new_change_tokens.insert(drive_group.drive_id.clone(), new_token.clone());
                    }

                    match response.next_page_token {
                        Some(token) => current_token = token,
                        None => break,
                    }
                }

                // If Google omitted newStartPageToken, retain the old
                // watermark and replay this page on the next run. Taking a
                // fresh token here could skip changes made while processing.
                if !new_change_tokens.contains_key(&drive_group.drive_id) {
                    if let Some(token) = stored_token {
                        new_change_tokens.insert(drive_group.drive_id.clone(), token.clone());
                    }
                }
            } else {
                let next_change_token = full_traversal_start_tokens
                    .get(drive_id)
                    .cloned()
                    .unwrap_or(None);

                // Rebuild this drive's compact folder ACL state from the full
                // selected-scope traversal, dropping entries for folders that
                // no longer exist or are no longer selected.
                let old_drive_folder_ids: Vec<String> = stored_folder_access
                    .iter()
                    .filter(|(_, info)| info.drive_id == *drive_id)
                    .map(|(folder_id, _)| folder_id.clone())
                    .collect();
                for folder_id in old_drive_folder_ids {
                    folder_access.remove(&folder_id);
                    folder_drive_ids.remove(&folder_id);
                }

                // Full scoped sync for this drive. Stream directly into batches.
                let mut file_batch: Vec<UserFile> = Vec::new();

                if drive_group.entire_drive {
                    self.load_scoped_drive_folder_access(
                        service_auth.as_ref(),
                        &principal_email,
                        &access_token,
                        drive_id,
                        &mut folder_access,
                        &mut folder_drive_ids,
                    )
                    .await?;

                    // Entire-drive selection: list all files with corpora=drive
                    let mut page_token: Option<String> = None;
                    loop {
                        let response = self
                            .drive_client
                            .list_files_in_drive(
                                &service_auth,
                                &principal_email,
                                &drive_group.drive_id,
                                page_token.as_deref(),
                                None,
                            )
                            .await?;

                        for file in response.files {
                            if self.should_index_file(&file) {
                                let effective_permissions =
                                    crate::models::compute_effective_permissions(
                                        &file,
                                        drive_id,
                                        &drive_permissions,
                                        raw_drive_acl,
                                        &folder_access,
                                        None,
                                    );
                                file_batch.push(UserFile {
                                    user_email: Arc::new(principal_email.clone()),
                                    file,
                                    permissions_override: Some(effective_permissions),
                                });
                                if file_batch.len() >= BATCH_SIZE {
                                    self.flush_batch(
                                        &mut file_batch,
                                        &source.id,
                                        &sync_run_id,
                                        ctx,
                                        service_auth.clone(),
                                        content_cache.clone(),
                                        &mut total_scanned,
                                        &mut total_updated,
                                    )
                                    .await?;
                                }
                            }
                        }

                        if ctx.is_cancelled() {
                            break;
                        }

                        page_token = response.next_page_token;
                        if page_token.is_none() {
                            break;
                        }
                    }
                } else {
                    // Folder-level selection: traverse each selected subtree.
                    let mut seen_file_ids: HashSet<String> = HashSet::new();
                    let mut visited_folders: HashSet<String> = HashSet::new();

                    for folder_id in &drive_group.folder_ids {
                        if ctx.is_cancelled() {
                            break;
                        }
                        if !visited_folders.insert(folder_id.clone()) {
                            continue; // already traversed via overlapping selection
                        }

                        self.ensure_scoped_folder_acl_chain(
                            service_auth.as_ref(),
                            &principal_email,
                            &access_token,
                            Some(folder_id.clone()),
                            drive_id,
                            &mut folder_access,
                            &mut folder_drive_ids,
                        )
                        .await?;

                        let mut queue = VecDeque::from([folder_id.clone()]);
                        while let Some(current_folder_id) = queue.pop_front() {
                            if ctx.is_cancelled() {
                                break;
                            }

                            // Mark visited to avoid re-traversal via
                            // overlapping folder selections.
                            visited_folders.insert(current_folder_id.clone());

                            let mut page_token: Option<String> = None;
                            loop {
                                // List ALL children — no date cutoff so old
                                // folders containing new files are never missed.
                                let response = self
                                    .drive_client
                                    .list_files_in_folder(
                                        &service_auth,
                                        &principal_email,
                                        &current_folder_id,
                                        &drive_group.drive_id,
                                        page_token.as_deref(),
                                    )
                                    .await?;

                                for file in response.files {
                                    if seen_file_ids.contains(&file.id) {
                                        continue;
                                    }
                                    seen_file_ids.insert(file.id.clone());

                                    // Folders are enqueued unconditionally and
                                    // retained for descendant ACL computation.
                                    if file.mime_type == FOLDER_MIME_TYPE {
                                        Self::record_scoped_folder_access(
                                            &mut folder_access,
                                            &mut folder_drive_ids,
                                            &file,
                                            drive_id,
                                        );
                                        if !visited_folders.contains(&file.id) {
                                            queue.push_back(file.id.clone());
                                        }
                                    }

                                    // Non-folder files in a selected subtree are
                                    // part of the complete scope regardless of age.
                                    if !self.should_index_file(&file) {
                                        continue;
                                    }
                                    self.ensure_scoped_folder_acl_chain(
                                        service_auth.as_ref(),
                                        &principal_email,
                                        &access_token,
                                        file.parents
                                            .as_ref()
                                            .and_then(|parents| parents.first())
                                            .cloned(),
                                        drive_id,
                                        &mut folder_access,
                                        &mut folder_drive_ids,
                                    )
                                    .await?;
                                    let effective_permissions =
                                        crate::models::compute_effective_permissions(
                                            &file,
                                            drive_id,
                                            &drive_permissions,
                                            raw_drive_acl,
                                            &folder_access,
                                            None,
                                        );
                                    file_batch.push(UserFile {
                                        user_email: Arc::new(principal_email.clone()),
                                        file,
                                        permissions_override: Some(effective_permissions),
                                    });
                                    if file_batch.len() >= BATCH_SIZE {
                                        self.flush_batch(
                                            &mut file_batch,
                                            &source.id,
                                            &sync_run_id,
                                            ctx,
                                            service_auth.clone(),
                                            content_cache.clone(),
                                            &mut total_scanned,
                                            &mut total_updated,
                                        )
                                        .await?;
                                    }
                                }

                                if ctx.is_cancelled() {
                                    break;
                                }

                                page_token = response.next_page_token;
                                if page_token.is_none() {
                                    break;
                                }
                            }
                        }
                    }
                }

                // Flush any remaining files for this drive.
                self.flush_batch(
                    &mut file_batch,
                    &source.id,
                    &sync_run_id,
                    ctx,
                    service_auth.clone(),
                    content_cache.clone(),
                    &mut total_scanned,
                    &mut total_updated,
                )
                .await?;

                // Reconcile trashed files in this drive on a full traversal
                // (see [`Self::publish_trashed_deletions`]).
                let trashed_files = self
                    .drive_client
                    .list_trashed_files_in_drive(
                        &service_auth,
                        &principal_email,
                        &drive_group.drive_id,
                    )
                    .await?;
                self.publish_trashed_deletions(ctx, trashed_files).await?;

                if ctx.is_cancelled() {
                    break;
                }

                if let Some(token) = next_change_token {
                    new_change_tokens.insert(drive_group.drive_id.clone(), token);
                }
            }

            if ctx.is_cancelled() {
                break;
            }
            completed_drives.insert(drive_id.clone());
        }

        let mut new_folder_access = stored_folder_access;
        for drive_group in &drives {
            let drive_id = &drive_group.drive_id;
            if !completed_drives.contains(drive_id) {
                continue;
            }
            new_folder_access.retain(|_, info| info.drive_id != *drive_id);
            for (folder_id, entry) in &folder_access {
                if folder_drive_ids.get(folder_id) != Some(drive_id) {
                    continue;
                }
                let Some(acl) = &entry.acl else {
                    continue;
                };
                if !entry.inherited_permissions_disabled && acl.is_empty() {
                    continue;
                }
                new_folder_access.insert(
                    folder_id.clone(),
                    crate::models::FolderAccessInfo {
                        drive_id: drive_id.clone(),
                        parents: entry.parents.clone(),
                        inherited_permissions_disabled: entry.inherited_permissions_disabled,
                        acl_fingerprint: crate::models::drive_acl_fingerprint(acl),
                    },
                );
            }
        }

        info!(
            "Scoped Drive sync completed for source {}: {} scanned, {} updated",
            source.id, total_scanned, total_updated
        );

        self.folder_cache.clear();

        Ok(self.build_drive_checkpoint(
            existing_state,
            None, // No per-user page tokens in scoped mode.
            Some(new_change_tokens),
            Some(current_fingerprint),
            Some(new_acl_fingerprints),
            Some(new_folder_access),
            Some(SCOPED_DWD_PERMISSION_MODEL_VERSION.to_string()),
        ))
    }

    /// SA-direct drive sync: index the configured shared drives as the service
    /// account itself (no `sub` impersonation, no Admin SDK, no user listing).
    ///
    /// For each drive it resolves the drive-level ACL via `permissions.list`,
    /// validates the SA role (Content manager/Manager — fail closed), maps the
    /// ACL to `DocumentPermissions`, and applies it to every emitted file
    /// (shared-drive `File.permissions` is always empty). Drive-membership
    /// changes are detected via a per-drive ACL fingerprint: when it changes,
    /// the drive is fully re-traversed (no date cutoff) so already-indexed
    /// unchanged documents get the new ACLs.
    async fn sync_drive_sa_direct(
        &self,
        source: &Source,
        config: &crate::models::GoogleSourceConfig,
        service_creds: &ServiceCredential,
        sync_type: SyncType,
        existing_state: &GoogleSyncCheckpoint,
        ctx: &SyncContext,
    ) -> Result<GoogleSyncCheckpoint> {
        let sync_run_id = ctx.sync_run_id();

        // SA-direct must have been validated at the request boundary; re-validate
        // defensively so a config edited behind the sync path still fails closed.
        config
            .validate(SourceType::GoogleDrive, service_creds.auth_type)
            .map_err(anyhow::Error::msg)?;

        let service_auth = Arc::new(
            self.create_auth(service_creds, SourceType::GoogleDrive)
                .await?,
        );
        let access_token = service_auth.get_self_access_token().await?;
        let sa_email = match service_auth.as_ref() {
            crate::auth::GoogleAuth::ServiceAccount(sa) => sa.client_email().to_string(),
            crate::auth::GoogleAuth::OAuth(_) => {
                return Err(anyhow!("SA-direct requires service-account credentials"));
            }
        };

        let (drive_cutoff_date, _gmail_cutoff_date) = self.get_cutoff_date()?;

        // Build drive groups from the typed config (already validated root-only).
        let filter_entries = &config.folder_path_filters;
        let current_fingerprint = format!(
            "sa-direct:{}",
            crate::models::compute_scope_fingerprint(Some(filter_entries))
        );
        let stored_fingerprint = existing_state.drive_scope_fingerprint.as_deref();
        let scope_changed = stored_fingerprint != Some(&current_fingerprint);
        let drives = build_drive_groups(filter_entries);
        if drives.is_empty() {
            return Err(anyhow!(
                "SA-direct sync requires at least one selected shared drive"
            ));
        }

        // Capture every drive watermark before ACL and folder preflight. A full
        // traversal may be selected only after those checks, so the watermark
        // must be established before them to avoid skipping intervening changes.
        let mut full_traversal_start_tokens: HashMap<String, Option<String>> = HashMap::new();
        for drive_group in &drives {
            let token = match self
                .drive_client
                .get_start_page_token_for_drive(&access_token, &drive_group.drive_id)
                .await
            {
                Ok(token) => Some(token),
                Err(error) => {
                    warn!(
                        "Failed to get start page token for drive {}: {}",
                        drive_group.drive_id, error
                    );
                    None
                }
            };
            full_traversal_start_tokens.insert(drive_group.drive_id.clone(), token);
        }

        // Scope fingerprint: include the auth mode so a DWD→SA-direct switch on
        // the same source forces a full resync (defensive; in-place conversion is
        // not a supported v1 path).
        if scope_changed {
            info!(
                "Drive scope transition detected: {:?} -> {} — forcing full sync",
                stored_fingerprint, current_fingerprint
            );
        }

        // Preflight ALL selected drives before processing any file: fetch each
        // drive's ACL, validate the SA role, and compute the ACL fingerprint.
        // A failure on any drive aborts the run before a partial batch is emitted.
        let old_acl_fingerprints = existing_state
            .drive_acl_fingerprints
            .clone()
            .unwrap_or_default();
        let acl_state = self
            .load_shared_drive_acl_state(
                &access_token,
                &drives,
                &old_acl_fingerprints,
                Some(&sa_email),
            )
            .await?;
        for (drive_id, drive_acl) in &acl_state.drive_acl_raw {
            crate::models::validate_sa_drive_access(drive_id, drive_acl, &sa_email)?;
            if drive_acl
                .iter()
                .any(|permission| permission.permission_type == "group")
            {
                warn!(
                    "Drive {} ACL includes group grants. Group-granted documents are visible \
                     only to users whose group memberships are synced into Omni by some source;\
                     this source may not be able to sync them.",
                    drive_id
                );
            }
        }
        let SharedDriveAclState {
            drive_acl_fingerprints: new_acl_fingerprints,
            drive_acl_overrides,
            drive_acl_raw,
            drive_acl_changed,
        } = acl_state;

        let content_cache = Arc::new(DriveContentCache::default());
        let mut total_scanned = 0;
        let mut total_updated = 0;
        let mut new_change_tokens: HashMap<String, String> = HashMap::new();
        let old_change_tokens = existing_state
            .drive_change_tokens
            .clone()
            .unwrap_or_default();

        // Folder access metadata for the effective-permission model. In
        // incremental mode, seed from the checkpoint (fingerprinted folders
        // only: limited-access or directly-granted); full traversals rebuild
        // the map from the drive listing.
        let stored_folder_access = existing_state.folder_access.clone().unwrap_or_default();
        let mut folder_access: HashMap<String, crate::models::FolderAccessEntry> =
            if sync_type == SyncType::Incremental {
                stored_folder_access
                    .iter()
                    .map(|(id, info)| {
                        (
                            id.clone(),
                            crate::models::FolderAccessEntry {
                                parents: info.parents.clone(),
                                inherited_permissions_disabled: info.inherited_permissions_disabled,
                                acl: None,
                            },
                        )
                    })
                    .collect()
            } else {
                HashMap::new()
            };
        // Drives that ran a full traversal this run (their folder-access state
        // replaces the checkpoint's for those folders).
        let mut full_drives: HashSet<String> = HashSet::new();

        for drive_group in &drives {
            if ctx.is_cancelled() {
                info!("Sync {} cancelled during SA-direct sync", sync_run_id);
                break;
            }

            let drive_id = &drive_group.drive_id;
            let drive_permissions = drive_acl_overrides
                .get(drive_id)
                .cloned()
                .expect("drive ACL resolved in preflight");
            let acl_changed = drive_acl_changed.get(drive_id).copied().unwrap_or(true);

            // Folder-level ACL preflight (incremental only): re-fetch ACLs for
            // fingerprinted folders (limited-access or directly-granted) to
            // detect changes that `changes.list` does not deliver for
            // descendants. A mismatch forces an uncut full re-traversal so
            // every descendant is re-emitted with the corrected ACL.
            let mut folder_acl_changed = false;
            if sync_type == SyncType::Incremental && !scope_changed {
                for (folder_id, info) in stored_folder_access.iter() {
                    if info.drive_id != *drive_id {
                        continue;
                    }
                    let raw = match self
                        .drive_client
                        .list_drive_permissions(&access_token, folder_id)
                        .await
                    {
                        Ok(raw) => raw,
                        // The folder was deleted or the SA lost access to it;
                        // its descendants are gone too, so force a full
                        // re-traversal (which prunes the stale fingerprint).
                        Err(e) => {
                            warn!(
                                "Folder {} ACL fetch failed ({}); forcing full re-traversal of drive {}",
                                folder_id, e, drive_id
                            );
                            folder_acl_changed = true;
                            break;
                        }
                    };
                    let direct = crate::models::filter_direct_permissions(&raw);
                    let fingerprint = crate::models::drive_acl_fingerprint(&direct);
                    if fingerprint != info.acl_fingerprint {
                        info!(
                            "ACL fingerprint changed for folder {} — forcing full re-traversal of drive {}",
                            folder_id, drive_id
                        );
                        folder_acl_changed = true;
                        break;
                    }
                }
            }

            // Bumped when the SA-direct permission model changes so a deploy
            // forces one uncut full re-traversal (documents re-emitted with
            // corrected ACLs).
            let model_mismatch = existing_state.permission_model_version.as_deref()
                != Some(crate::models::SA_DIRECT_PERMISSION_MODEL_VERSION);

            // ACL/scope/model changes force an uncut full traversal (no date
            // cutoff) so every previously indexed unchanged document is
            // re-emitted with new ACLs.
            let use_incremental = sync_type == SyncType::Incremental
                && !scope_changed
                && !acl_changed
                && !folder_acl_changed
                && !model_mismatch
                && old_change_tokens.get(drive_id).is_some();

            let mut force_full = false;
            if use_incremental {
                let start_token = old_change_tokens.get(drive_id).unwrap().clone();
                let mut current_token = start_token.clone();

                loop {
                    let response = self
                        .drive_client
                        .list_changes_for_drive(&access_token, &current_token, drive_id)
                        .await?;

                    if ctx.is_cancelled() {
                        break;
                    }

                    let mut file_batch: Vec<UserFile> = Vec::new();
                    for change in &response.changes {
                        // removed:true (permanent delete) or file.trashed:true
                        // (in trash) both mean the doc must leave the index.
                        let is_removed = change.removed.unwrap_or(false)
                            || change
                                .file
                                .as_ref()
                                .and_then(|f| f.trashed)
                                .unwrap_or(false);
                        if is_removed {
                            if let Some(file_id) = &change.file_id {
                                info!(
                                    "File {} was removed (SA-direct incremental), publishing deletion",
                                    file_id
                                );
                                self.publish_deletion_event(ctx, file_id).await?;
                            }
                            continue;
                        }

                        if let Some(file) = &change.file {
                            if file.mime_type == FOLDER_MIME_TYPE {
                                // Folders are not indexed, but they carry access
                                // metadata. Track them and detect boundary
                                // transitions that would leave descendants with
                                // stale ACLs.
                                self.handle_folder_change(
                                    file,
                                    &stored_folder_access,
                                    &mut folder_access,
                                    &mut force_full,
                                );
                                continue;
                            }
                            if self.should_index_file(file) {
                                self.ensure_folder_acl_chain(
                                    &access_token,
                                    file,
                                    drive_id,
                                    &mut folder_access,
                                )
                                .await?;
                                let effective = crate::models::compute_effective_permissions(
                                    file,
                                    drive_id,
                                    &drive_permissions,
                                    drive_acl_raw
                                        .get(drive_id)
                                        .expect("drive ACL resolved in preflight"),
                                    &folder_access,
                                    Some(&sa_email),
                                );
                                file_batch.push(UserFile {
                                    user_email: Arc::new(sa_email.clone()),
                                    file: file.clone(),
                                    permissions_override: Some(effective),
                                });
                                if file_batch.len() >= SA_DIRECT_BATCH_SIZE {
                                    self.flush_batch(
                                        &mut file_batch,
                                        &source.id,
                                        &sync_run_id,
                                        ctx,
                                        service_auth.clone(),
                                        content_cache.clone(),
                                        &mut total_scanned,
                                        &mut total_updated,
                                    )
                                    .await?;
                                }
                            }
                        }
                    }

                    self.flush_batch(
                        &mut file_batch,
                        &source.id,
                        &sync_run_id,
                        ctx,
                        service_auth.clone(),
                        content_cache.clone(),
                        &mut total_scanned,
                        &mut total_updated,
                    )
                    .await?;

                    if let Some(new_token) = &response.new_start_page_token {
                        new_change_tokens.insert(drive_id.clone(), new_token.clone());
                    }

                    if force_full {
                        break;
                    }

                    match response.next_page_token {
                        Some(token) => current_token = token,
                        None => break,
                    }
                }

                if force_full {
                    // A folder crossed into/out of the limited-access model
                    // mid-feed; discard incremental progress and re-traverse
                    // fully so descendants get correct ACLs. Always uncut:
                    // affected descendants can be of any age.
                    new_change_tokens.remove(drive_id);
                    self.sa_direct_full_traverse_drive(
                        source,
                        &sync_run_id,
                        ctx,
                        &service_auth,
                        &sa_email,
                        &access_token,
                        drive_id,
                        &drive_permissions,
                        drive_acl_raw
                            .get(drive_id)
                            .expect("drive ACL resolved in preflight"),
                        &drive_cutoff_date,
                        true,
                        full_traversal_start_tokens
                            .get(drive_id)
                            .cloned()
                            .unwrap_or(None),
                        &content_cache,
                        &mut folder_access,
                        &mut total_scanned,
                        &mut total_updated,
                        &mut new_change_tokens,
                    )
                    .await?;
                    full_drives.insert(drive_id.clone());
                } else if !new_change_tokens.contains_key(drive_id) {
                    // Keep the old watermark when Google did not return a new
                    // one; taking a fresh token here can skip in-flight changes.
                    if let Some(token) = old_change_tokens.get(drive_id) {
                        new_change_tokens.insert(drive_id.clone(), token.clone());
                    }
                }
            } else {
                self.sa_direct_full_traverse_drive(
                    source,
                    &sync_run_id,
                    ctx,
                    &service_auth,
                    &sa_email,
                    &access_token,
                    drive_id,
                    &drive_permissions,
                    drive_acl_raw
                        .get(drive_id)
                        .expect("drive ACL resolved in preflight"),
                    &drive_cutoff_date,
                    sync_type != SyncType::Incremental
                        || acl_changed
                        || model_mismatch
                        || folder_acl_changed,
                    full_traversal_start_tokens
                        .get(drive_id)
                        .cloned()
                        .unwrap_or(None),
                    &content_cache,
                    &mut folder_access,
                    &mut total_scanned,
                    &mut total_updated,
                    &mut new_change_tokens,
                )
                .await?;
                full_drives.insert(drive_id.clone());
            }
        }

        info!(
            "SA-direct Drive sync completed for source {}: {} scanned, {} updated",
            source.id, total_scanned, total_updated
        );

        self.folder_cache.clear();

        // Persist folder access state: rebuilt from the run-local map for
        // drives that were fully traversed; existing entries kept for drives
        // that only ran incrementally.
        let mut new_folder_access = stored_folder_access;
        for drive_group in &drives {
            if !full_drives.contains(&drive_group.drive_id) {
                continue;
            }
            let drive_id = &drive_group.drive_id;
            new_folder_access.retain(|_, info| info.drive_id != *drive_id);
            for (folder_id, entry) in folder_access.iter() {
                let Some(acl) = &entry.acl else {
                    continue;
                };
                if !entry.inherited_permissions_disabled && acl.is_empty() {
                    continue;
                }
                new_folder_access.insert(
                    folder_id.clone(),
                    crate::models::FolderAccessInfo {
                        drive_id: drive_id.clone(),
                        parents: entry.parents.clone(),
                        inherited_permissions_disabled: entry.inherited_permissions_disabled,
                        acl_fingerprint: crate::models::drive_acl_fingerprint(acl),
                    },
                );
            }
        }

        Ok(GoogleSyncCheckpoint {
            gmail_history_ids: existing_state.gmail_history_ids.clone(),
            drive_page_tokens: None,
            drive_change_tokens: Some(new_change_tokens),
            drive_scope_fingerprint: Some(current_fingerprint),
            drive_acl_fingerprints: Some(new_acl_fingerprints),
            folder_access: Some(new_folder_access),
            permission_model_version: Some(
                crate::models::SA_DIRECT_PERMISSION_MODEL_VERSION.to_string(),
            ),
            chat: existing_state.chat.clone(),
        })
    }

    /// Full (cutoff or uncut) traversal of one shared drive in SA-direct mode.
    /// Builds the complete folder access index first (folders-only listing, no
    /// cutoff), then streams indexable files computing per-file effective
    /// permissions from drive + folder ACLs. Also reconciles pre-trashed files.
    #[allow(clippy::too_many_arguments)]
    async fn sa_direct_full_traverse_drive(
        &self,
        source: &Source,
        sync_run_id: &str,
        ctx: &SyncContext,
        service_auth: &Arc<GoogleAuth>,
        sa_email: &str,
        access_token: &str,
        drive_id: &str,
        drive_permissions: &DocumentPermissions,
        drive_acl_raw: &[GoogleDrivePermission],
        drive_cutoff_date: &str,
        uncut: bool,
        start_change_token: Option<String>,
        content_cache: &Arc<DriveContentCache>,
        folder_access: &mut HashMap<String, crate::models::FolderAccessEntry>,
        total_scanned: &mut usize,
        total_updated: &mut usize,
        new_change_tokens: &mut HashMap<String, String>,
    ) -> Result<()> {
        // Changes made after this token and during the full pass are replayed
        // on the next incremental run.
        let next_change_token = start_change_token;

        // Folders-first pass: the complete folder index must be in place before
        // computing effective permissions for any file (listing order is
        // arbitrary, so a file's ancestors may appear on later pages).
        let mut folder_page: Option<String> = None;
        loop {
            let response = self
                .drive_client
                .list_folders_in_drive(service_auth, sa_email, drive_id, folder_page.as_deref())
                .await?;
            for folder in response.files {
                Self::record_folder_access(folder_access, &folder);
            }
            match response.next_page_token {
                Some(token) => folder_page = Some(token),
                None => break,
            }
        }

        // Fetch directly-assigned ACLs for every folder with bounded
        // concurrency. Needed for complete fingerprint coverage: a grant change
        // on any granted folder must be detectable on future incremental runs
        // even when its descendants were not re-walked this run.
        let pending: Vec<String> = folder_access
            .iter()
            .filter(|(_, entry)| entry.acl.is_none())
            .map(|(id, _)| id.clone())
            .collect();
        self.load_folder_acls(access_token, pending, folder_access, false)
            .await?;

        let mut file_batch: Vec<UserFile> = Vec::new();
        let mut page_token: Option<String> = None;
        loop {
            // Full traversal: when the ACL/scope/model changed, pass no cutoff
            // so every document (even unchanged old ones) is re-emitted;
            // otherwise apply the normal date cutoff.
            let modified_after: Option<&str> = if uncut { None } else { Some(drive_cutoff_date) };
            let response = self
                .drive_client
                .list_files_in_drive(
                    service_auth,
                    sa_email,
                    drive_id,
                    page_token.as_deref(),
                    modified_after,
                )
                .await?;

            for file in response.files {
                if self.should_index_file(&file) {
                    self.ensure_folder_acl_chain(access_token, &file, drive_id, folder_access)
                        .await?;
                    let effective = crate::models::compute_effective_permissions(
                        &file,
                        drive_id,
                        drive_permissions,
                        drive_acl_raw,
                        folder_access,
                        Some(sa_email),
                    );
                    file_batch.push(UserFile {
                        user_email: Arc::new(sa_email.to_string()),
                        file,
                        permissions_override: Some(effective),
                    });
                    if file_batch.len() >= SA_DIRECT_BATCH_SIZE {
                        self.flush_batch(
                            &mut file_batch,
                            &source.id,
                            sync_run_id,
                            ctx,
                            service_auth.clone(),
                            content_cache.clone(),
                            total_scanned,
                            total_updated,
                        )
                        .await?;
                    }
                }
            }

            if ctx.is_cancelled() {
                break;
            }

            page_token = response.next_page_token;
            if page_token.is_none() {
                break;
            }
        }

        self.flush_batch(
            &mut file_batch,
            &source.id,
            sync_run_id,
            ctx,
            service_auth.clone(),
            content_cache.clone(),
            total_scanned,
            total_updated,
        )
        .await?;

        // Reconcile trashed files: incremental changes.list only reports a
        // trash transition once, so files trashed before we observed the event
        // (or trashed while incremental was unavailable) would otherwise stay
        // in the index forever. A full traversal is the safe point to list
        // trashed=true and publish deletions.
        let trashed_files = self
            .drive_client
            .list_trashed_files_in_drive(service_auth, sa_email, drive_id)
            .await?;
        self.publish_trashed_deletions(ctx, trashed_files).await?;

        if let Some(token) = next_change_token {
            new_change_tokens.insert(drive_id.to_string(), token);
        }

        Ok(())
    }

    /// Ensure the directly-assigned ACL is fetched (and cached in
    /// `folder_access`) for every ancestor folder of `file` that the effective
    /// permission computation needs. Folders absent from the map contribute
    /// nothing (no direct grants, not limited-access) and are skipped.
    async fn ensure_folder_acl_chain(
        &self,
        access_token: &str,
        file: &GoogleDriveFile,
        drive_id: &str,
        folder_access: &mut HashMap<String, crate::models::FolderAccessEntry>,
    ) -> Result<()> {
        let mut parent = file.parents.as_ref().and_then(|p| p.first()).cloned();
        while let Some(parent_id) = parent {
            if parent_id == drive_id {
                break;
            }
            let needs_fetch = match folder_access.get(&parent_id) {
                Some(entry) => entry.acl.is_none(),
                None => break,
            };
            if needs_fetch {
                let raw = match self
                    .drive_client
                    .list_drive_permissions(access_token, &parent_id)
                    .await
                {
                    Ok(raw) => raw,
                    // Folder vanished mid-sync (its files are being deleted
                    // too); an empty direct ACL keeps the computation
                    // conservative without failing the run.
                    Err(e) => {
                        debug!(
                            "Folder {} ACL fetch failed mid-sync ({}); treating as no direct grants",
                            parent_id, e
                        );
                        if let Some(entry) = folder_access.get_mut(&parent_id) {
                            entry.acl = Some(Vec::new());
                        }
                        break;
                    }
                };
                if let Some(entry) = folder_access.get_mut(&parent_id) {
                    entry.acl = Some(crate::models::filter_direct_permissions(&raw));
                }
            }
            parent = folder_access
                .get(&parent_id)
                .and_then(|entry| entry.parents.first())
                .cloned();
        }
        Ok(())
    }

    /// Track a folder change entry from the incremental changes feed. Updates
    /// the run-local access map and detects boundary transitions that would
    /// leave descendants with stale ACLs (a folder becoming limited-access, or
    /// a fingerprinted folder flipping its flag) — those force a full
    /// re-traversal.
    fn handle_folder_change(
        &self,
        file: &GoogleDriveFile,
        stored: &HashMap<String, crate::models::FolderAccessInfo>,
        folder_access: &mut HashMap<String, crate::models::FolderAccessEntry>,
        force_full: &mut bool,
    ) {
        let folder_id = file.id.clone();
        let limited = file.inherited_permissions_disabled.unwrap_or(false);
        let stored_flag = stored
            .get(&folder_id)
            .map(|i| i.inherited_permissions_disabled);
        match (limited, stored_flag) {
            (true, None) => {
                // Limited-access but never fingerprinted (created limited, or
                // made limited after its last full traversal) — descendants
                // may carry stale drive-wide ACLs.
                info!(
                    "Folder {} is limited-access without a recorded fingerprint — forcing full re-traversal",
                    folder_id
                );
                *force_full = true;
            }
            (flag, Some(stored_flag)) if flag != stored_flag => {
                info!(
                    "Folder {} flipped limited-access {} -> {} — forcing full re-traversal",
                    folder_id, stored_flag, flag
                );
                *force_full = true;
            }
            _ => {}
        }
        folder_access.insert(
            folder_id,
            crate::models::FolderAccessEntry {
                parents: file.parents.clone().unwrap_or_default(),
                inherited_permissions_disabled: limited,
                acl: None,
            },
        );
    }

    async fn sync_drive_source_internal(
        &self,
        source: &Source,
        config: &crate::models::GoogleSourceConfig,
        service_creds: &ServiceCredential,
        sync_type: SyncType,
        existing_state: GoogleSyncCheckpoint,
        ctx: &SyncContext,
    ) -> Result<GoogleSyncCheckpoint> {
        let sync_run_id = ctx.sync_run_id();
        // Folder ancestry metadata is only a run-local view. Discard entries
        // left by an interrupted run so a move is evaluated against current
        // Drive parents during scope evaluation.
        self.folder_cache.clear();

        let native_source_type = SourceType::try_from(source.source_type.as_str())
            .map_err(|e| anyhow!("Unsupported source type: {}", e))?;

        // SA-direct mode: index the selected shared drives as the SA itself.
        if matches!(
            config.auth_mode,
            crate::models::GoogleAuthMode::ServiceAccountDirect
        ) {
            return self
                .sync_drive_sa_direct(
                    source,
                    config,
                    service_creds,
                    sync_type,
                    &existing_state,
                    ctx,
                )
                .await;
        }

        let service_auth = Arc::new(self.create_auth(service_creds, native_source_type).await?);

        // Parse folder-path filters. Malformed config is a sync error.
        let parsed_filters = match crate::models::parse_folder_path_filters(&source.config) {
            Ok(f) => f,
            Err(e) => {
                return Err(anyhow!("Invalid folder path filter configuration: {}", e));
            }
        };

        // Scoped service-account mode uses the drive-corpora traversal. OAuth
        // sources keep the per-user listing and apply ancestry filtering below.
        if parsed_filters.is_some() && !service_auth.is_oauth() {
            return self
                .sync_drive_scoped(source, service_creds, sync_type, &existing_state, ctx)
                .await;
        }

        // === OAuth or unfiltered DWD path: per-user listing ===

        // Scope fingerprints are shared by OAuth and service-account sources.
        // A changed personal scope forces a full traversal so files moved into
        // the selected folders are discovered immediately.
        let current_fingerprint = match &parsed_filters {
            Some(f) => crate::models::compute_scope_fingerprint(Some(f)),
            None => "all".to_string(),
        };

        // Detect scope transition and force full sync if needed.
        let stored_fingerprint = existing_state.drive_scope_fingerprint.as_deref();
        let scope_changed = stored_fingerprint != Some(&current_fingerprint);
        if scope_changed {
            info!(
                "Drive scope transition detected: {:?} -> {} — forcing full sync",
                stored_fingerprint, current_fingerprint
            );
        }

        let (drive_cutoff_date, _gmail_cutoff_date) = self.get_cutoff_date()?;
        // A selected Drive scope represents the user's complete folder
        // selection, not only recently modified files. Full traversals must
        // therefore include older files so they can be indexed.
        let drive_created_after = if parsed_filters.is_some() {
            None
        } else {
            Some(drive_cutoff_date.as_str())
        };
        info!("Using Drive cutoff date: {:?}", drive_created_after);

        // Build user list: single OAuth user or all domain users
        let user_emails: Vec<String> = if service_auth.is_oauth() {
            let email = service_auth
                .oauth_user_email()
                .ok_or_else(|| anyhow::anyhow!("OAuth auth missing user_email"))?
                .to_string();
            info!("OAuth Drive sync for single user: {}", email);
            vec![email]
        } else {
            let domain = crate::auth::get_domain_from_credentials(service_creds)?;
            let user_email = ctx.get_user_email_for_source().await
                .map_err(|e| anyhow::anyhow!("Failed to get user email for source {}: {}. Make sure the source has a valid creator.", source.id, e))?;

            info!("Listing all users in domain: {}", domain);
            info!("Using user email: {}", user_email);
            let admin_access_token = service_auth.get_access_token(&user_email).await
                .map_err(|e| anyhow::anyhow!("Failed to get access token for user {}: {}. Make sure the user is a super-admin and the service account has domain-wide delegation enabled.", user_email, e))?;
            let all_users = self
                .admin_client
                .list_all_users(&admin_access_token, &domain)
                .await?;
            info!("Found {} users in domain {}", all_users.len(), domain);

            let filtered: Vec<String> = all_users
                .into_iter()
                .filter(|user| source.should_index_user(&user.primary_email))
                .map(|user| user.primary_email)
                .collect();
            info!("After filtering: {} users will be indexed", filtered.len());
            filtered
        };

        // Determine effective incremental mode — scope transitions force full sync.
        let is_incremental =
            !drive_scope_requires_full_sync(sync_type, stored_fingerprint, &current_fingerprint);

        let gmail_history_ids = existing_state.gmail_history_ids.clone();
        let chat_checkpoint = existing_state.chat.clone();
        let has_checkpoint = parsed_filters.is_some() && existing_state.drive_page_tokens.is_some();
        let old_page_tokens = existing_state.drive_page_tokens.clone().unwrap_or_default();
        let can_resume_full = sync_type == SyncType::Full && ctx.is_resume() && !scope_changed;
        let requires_complete_full_checkpoint =
            parsed_filters.is_some() && !is_incremental && !can_resume_full;
        let mut new_page_tokens: HashMap<String, String> = if can_resume_full {
            old_page_tokens.clone()
        } else {
            HashMap::new()
        };

        info!(
            "Starting user processing for {} users (Drive, incremental={})",
            user_emails.len(),
            is_incremental
        );

        let mut total_scanned = 0;
        let mut total_updated = 0;
        let mut successful_users = 0;
        let mut errors = 0;
        let mut last_error: Option<String> = None;
        // OAuth sources use the same ancestry filter as DWD sources. Shared
        // drive roots work because Drive file parents include the drive root id;
        // My Drive folder selections are matched by walking parent metadata.
        let folder_filter_ids: Option<Arc<HashSet<String>>> = parsed_filters
            .as_ref()
            .map(|filters| Arc::new(filters.iter().map(|filter| filter.id.clone()).collect()));
        let content_cache = Arc::new(DriveContentCache::default());
        let parallel_users = google_drive_parallel_users();
        info!("Processing Drive users with concurrency {}", parallel_users);

        let user_tasks = stream::iter(user_emails.iter().cloned()).map(|cur_user_email| {
            let service_auth = service_auth.clone();
            let source_id = source.id.clone();
            let sync_run_id = sync_run_id.to_string();
            let ctx = ctx.clone();
            let content_cache = content_cache.clone();
            let stored_page_token = old_page_tokens.get(cur_user_email.as_str()).cloned();
            let folder_filter_ids = folder_filter_ids.clone();

            async move {
                if can_resume_full && stored_page_token.is_some() {
                    info!(
                        "Skipping Drive user {} already checkpointed for sync {}",
                        cur_user_email, sync_run_id
                    );
                    return (
                        cur_user_email,
                        Ok(DriveUserSyncResult {
                            scanned: 0,
                            updated: 0,
                            next_page_token: None,
                        }),
                    );
                }

                if ctx.is_cancelled() {
                    info!(
                        "Sync {} cancelled, skipping Drive sync for user {}",
                        sync_run_id, cur_user_email
                    );
                    return (
                        cur_user_email,
                        Ok(DriveUserSyncResult {
                            scanned: 0,
                            updated: 0,
                            next_page_token: None,
                        }),
                    );
                }

                let _access_token = match service_auth.get_access_token(&cur_user_email).await {
                    Ok(access_token) => access_token,
                    Err(e) => {
                        return (
                            cur_user_email.clone(),
                            Err(anyhow!(
                                "Failed to get access token for user {}: {}. This user may not have Drive access.",
                                cur_user_email,
                                e
                            )),
                        );
                    }
                };

                info!("Processing user: {}", cur_user_email);

                let use_incremental = is_incremental && stored_page_token.is_some();
                let reconcile_changes_before_full =
                    !is_incremental && has_checkpoint && stored_page_token.is_some();
                let result = if reconcile_changes_before_full {
                    let start_token = stored_page_token.as_deref().unwrap();
                    // Consume the saved changes token before the full crawl so
                    // removed or access-revoked files also leave the index.
                    let pre_full_result = match self
                        .sync_drive_for_user_incremental(
                            &cur_user_email,
                            service_auth.clone(),
                            &source_id,
                            &sync_run_id,
                            &ctx,
                            start_token,
                            content_cache.clone(),
                            folder_filter_ids.clone(),
                        )
                        .await
                    {
                        Ok(result) => result,
                        Err(error) => {
                            let user_email = cur_user_email.clone();
                            return (
                                cur_user_email,
                                Err(error).with_context(|| {
                                    format!(
                                        "Pre-full-sync Drive changes reconciliation failed for {}",
                                        user_email
                                    )
                                }),
                            );
                        }
                    };
                    self.sync_drive_for_user(
                        &cur_user_email,
                        service_auth.clone(),
                        &source_id,
                        &sync_run_id,
                        &ctx,
                        drive_created_after,
                        content_cache.clone(),
                        folder_filter_ids.clone(),
                        pre_full_result.next_page_token,
                    )
                    .await
                } else if use_incremental {
                    let start_token = stored_page_token.as_deref().unwrap();
                    info!(
                        "Using incremental Drive sync for user {} from pageToken {}",
                        cur_user_email, start_token
                    );
                    match self
                        .sync_drive_for_user_incremental(
                            &cur_user_email,
                            service_auth.clone(),
                            &source_id,
                            &sync_run_id,
                            &ctx,
                            start_token,
                            content_cache.clone(),
                            folder_filter_ids.clone(),
                        )
                        .await
                    {
                        Ok(result) => Ok(result),
                        Err(e) => {
                            warn!(
                                error = ?e,
                                user = %cur_user_email,
                                "Incremental drive sync failed."
                            );
                            Err(e).with_context(|| {
                                format!(
                                    "Incremental drive sync failed for {} at pageToken {}",
                                    cur_user_email, start_token
                                )
                            })
                        }
                    }
                } else {
                    self.sync_drive_for_user(
                        &cur_user_email,
                        service_auth.clone(),
                        &source_id,
                        &sync_run_id,
                        &ctx,
                        drive_created_after,
                        content_cache.clone(),
                        folder_filter_ids.clone(),
                        None,
                    )
                    .await
                };

                match result {
                    Ok(result) => (cur_user_email, Ok(result)),
                    Err(e) => (cur_user_email, Err(e)),
                }
            }
        });

        let mut user_results = user_tasks.buffer_unordered(parallel_users);
        while let Some((cur_user_email, result)) = user_results.next().await {
            match result {
                Ok(result) => {
                    successful_users += 1;
                    total_scanned += result.scanned;
                    total_updated += result.updated;
                    info!(
                        "User {} Drive sync completed: {} scanned, {} updated",
                        cur_user_email, result.scanned, result.updated
                    );

                    if let Some(token) = result
                        .next_page_token
                        .or_else(|| old_page_tokens.get(&cur_user_email).cloned())
                    {
                        new_page_tokens.insert(cur_user_email.clone(), token);
                    }

                    let checkpoint_state = GoogleSyncCheckpoint {
                        gmail_history_ids: gmail_history_ids.clone(),
                        drive_page_tokens: if new_page_tokens.is_empty() {
                            None
                        } else {
                            Some(new_page_tokens.clone())
                        },
                        drive_change_tokens: None,
                        drive_scope_fingerprint: if requires_complete_full_checkpoint {
                            None
                        } else {
                            Some(current_fingerprint.clone())
                        },
                        drive_acl_fingerprints: existing_state.drive_acl_fingerprints.clone(),
                        chat: chat_checkpoint.clone(),
                        ..Default::default()
                    };
                    ctx.save_checkpoint(serde_json::to_value(&checkpoint_state)?)
                        .await
                        .with_context(|| {
                            format!(
                                "Failed to checkpoint Drive state after user {}",
                                cur_user_email
                            )
                        })?;
                }
                Err(e) => {
                    if is_google_api_service_disabled_error(&e) {
                        return Err(google_api_service_disabled_error("Google Drive", &e));
                    }
                    error!(
                        "Failed to process Drive for user {}: {:#}",
                        cur_user_email, e
                    );
                    last_error = Some(format!("{:#}", e));
                    errors += 1;
                }
            }
        }

        info!(
            "User processing complete. Total: {} scanned, {} updated, {} errors",
            total_scanned, total_updated, errors
        );

        if !ctx.is_cancelled() && successful_users == 0 && errors > 0 {
            return Err(anyhow!(
                "Google Drive sync failed for all {} indexed users; last error: {}",
                errors,
                last_error.unwrap_or_else(|| "unknown error".to_string())
            ));
        }

        let full_scope_sync_completed =
            !ctx.is_cancelled() && errors == 0 && successful_users == user_emails.len();

        info!(
            "Sync completed for source {}: {} scanned, {} updated",
            source.id, total_scanned, total_updated
        );

        // Clear folder cache to free memory after sync
        self.folder_cache.clear();

        info!("Completed sync for source: {}", source.id);

        let checkpoint = GoogleSyncCheckpoint {
            gmail_history_ids,
            drive_page_tokens: if new_page_tokens.is_empty() {
                None
            } else {
                Some(new_page_tokens)
            },
            drive_change_tokens: None,
            drive_scope_fingerprint: if requires_complete_full_checkpoint
                && !full_scope_sync_completed
            {
                None
            } else {
                Some(current_fingerprint)
            },
            drive_acl_fingerprints: existing_state.drive_acl_fingerprints.clone(),
            chat: chat_checkpoint,
            ..Default::default()
        };
        if requires_complete_full_checkpoint {
            ctx.save_checkpoint(serde_json::to_value(&checkpoint)?)
                .await
                .context("Failed to save Drive full-sync checkpoint")?;
        }
        Ok(checkpoint)
    }

    async fn sync_gmail_source_internal(
        &self,
        source: &Source,
        service_creds: &ServiceCredential,
        sync_type: SyncType,
        existing_state: GoogleSyncCheckpoint,
        known_groups: HashSet<String>,
        ctx: &SyncContext,
    ) -> Result<GoogleSyncCheckpoint> {
        let sync_run_id = ctx.sync_run_id();

        let native_source_type = SourceType::try_from(source.source_type.as_str())
            .map_err(|e| anyhow!("Unsupported source type: {}", e))?;
        let service_auth = Arc::new(self.create_auth(service_creds, native_source_type).await?);

        let (_drive_cutoff_date, gmail_cutoff_date) = self.get_cutoff_date()?;
        info!("Using Gmail cutoff date: {}", gmail_cutoff_date);

        // Build user list: single OAuth user or all domain users
        let user_emails: Vec<String> = if service_auth.is_oauth() {
            let email = service_auth
                .oauth_user_email()
                .ok_or_else(|| anyhow::anyhow!("OAuth auth missing user_email"))?
                .to_string();
            info!("OAuth Gmail sync for single user: {}", email);
            vec![email]
        } else {
            let domain = crate::auth::get_domain_from_credentials(service_creds)?;
            let user_email = ctx.get_user_email_for_source().await
                .map_err(|e| anyhow::anyhow!("Failed to get user email for source {}: {}. Make sure the source has a valid creator.", source.id, e))?;

            info!("Listing all users in domain: {}", domain);
            info!("Using user email: {}", user_email);
            let admin_access_token = service_auth.get_access_token(&user_email).await
                .map_err(|e| anyhow::anyhow!("Failed to get access token for user {}: {}. Make sure the user is a super-admin and the service account has domain-wide delegation enabled.", user_email, e))?;
            let all_users = self
                .admin_client
                .list_all_users(&admin_access_token, &domain)
                .await?;
            info!("Found {} users in domain {}", all_users.len(), domain);

            let filtered: Vec<String> = all_users
                .into_iter()
                .filter(|user| source.should_index_user(&user.primary_email))
                .map(|user| user.primary_email)
                .collect();
            info!("After filtering: {} users will be indexed", filtered.len());
            filtered
        };

        let is_incremental = matches!(sync_type, SyncType::Incremental);

        let drive_page_tokens = existing_state.drive_page_tokens.clone();
        let chat_checkpoint = existing_state.chat.clone();
        let old_history_ids = existing_state.gmail_history_ids.unwrap_or_default();
        let can_resume_full = sync_type == SyncType::Full && ctx.is_resume();
        let mut new_history_ids: HashMap<String, String> = if can_resume_full {
            old_history_ids.clone()
        } else {
            HashMap::new()
        };

        let processed_threads = Arc::new(std::sync::Mutex::new(HashSet::<String>::new()));
        let known_groups = Arc::new(known_groups);

        info!(
            "Starting sequential user processing for {} users (Gmail, incremental={})",
            user_emails.len(),
            is_incremental
        );

        let mut total_processed = 0;
        let mut total_updated = 0;
        let mut successful_users = 0;
        let mut failed_users = 0;
        let mut last_error: Option<String> = None;

        for cur_user_email in &user_emails {
            if ctx.is_cancelled() {
                info!("Sync {} cancelled, stopping Gmail sync early", sync_run_id);
                break;
            }

            match service_auth.get_access_token(cur_user_email).await {
                Ok(_token) => {
                    info!("Processing user: {}", cur_user_email);

                    let stored_history_id = old_history_ids.get(cur_user_email.as_str());
                    if can_resume_full && stored_history_id.is_some() {
                        info!(
                            "Skipping Gmail user {} already checkpointed for sync {}",
                            cur_user_email, sync_run_id
                        );
                        continue;
                    }
                    let use_incremental = is_incremental && stored_history_id.is_some();

                    let result = if use_incremental {
                        let start_id = stored_history_id.unwrap();
                        info!(
                            "Using incremental Gmail sync for user {} from historyId {}",
                            cur_user_email, start_id
                        );
                        match self
                            .sync_gmail_for_user_incremental(
                                &cur_user_email,
                                service_auth.clone(),
                                &source.id,
                                sync_run_id,
                                ctx,
                                start_id,
                                processed_threads.clone(),
                                known_groups.clone(),
                            )
                            .await
                        {
                            Ok(result) => Ok(result),
                            Err(e) => {
                                let err_str = format!("{}", e);
                                if err_str.contains("HTTP 404") {
                                    warn!(
                                        "History expired for user {}, falling back to full sync",
                                        cur_user_email
                                    );
                                    self.sync_gmail_for_user(
                                        &cur_user_email,
                                        service_auth.clone(),
                                        ctx,
                                        processed_threads.clone(),
                                        Some(&gmail_cutoff_date),
                                        known_groups.clone(),
                                    )
                                    .await
                                } else {
                                    Err(e)
                                }
                            }
                        }
                    } else {
                        self.sync_gmail_for_user(
                            &cur_user_email,
                            service_auth.clone(),
                            ctx,
                            processed_threads.clone(),
                            Some(&gmail_cutoff_date),
                            known_groups.clone(),
                        )
                        .await
                    };

                    let user_succeeded = match result {
                        Ok((processed, updated)) => {
                            successful_users += 1;
                            total_processed += processed;
                            total_updated += updated;
                            info!(
                                "User {} Gmail sync completed: {} processed, {} updated",
                                cur_user_email, processed, updated
                            );
                            true
                        }
                        Err(e) => {
                            if is_google_api_service_disabled_error(&e) {
                                return Err(google_api_service_disabled_error("Gmail", &e));
                            }
                            error!(
                                "Failed to process Gmail for user {}: {:#}",
                                cur_user_email, e
                            );
                            failed_users += 1;
                            last_error = Some(format!("{:#}", e));
                            false
                        }
                    };

                    // Capture the historyId watermark AFTER the user finishes
                    // and checkpoint immediately. Capturing before processing
                    // would let resume skip past unprocessed history on crash.
                    if user_succeeded {
                        match self
                            .gmail_client
                            .get_profile(&service_auth, &cur_user_email)
                            .await
                        {
                            Ok(profile) => {
                                new_history_ids.insert(cur_user_email.clone(), profile.history_id);
                            }
                            Err(e) => {
                                warn!(
                                    "Failed to get Gmail profile for user {}: {}",
                                    cur_user_email, e
                                );
                            }
                        }

                        let checkpoint_state = GoogleSyncCheckpoint {
                            gmail_history_ids: if new_history_ids.is_empty() {
                                None
                            } else {
                                Some(new_history_ids.clone())
                            },
                            drive_page_tokens: drive_page_tokens.clone(),
                            drive_change_tokens: existing_state.drive_change_tokens.clone(),
                            drive_scope_fingerprint: existing_state.drive_scope_fingerprint.clone(),
                            drive_acl_fingerprints: existing_state.drive_acl_fingerprints.clone(),
                            chat: chat_checkpoint.clone(),
                            ..Default::default()
                        };
                        ctx.save_checkpoint(serde_json::to_value(&checkpoint_state)?)
                            .await
                            .with_context(|| {
                                format!(
                                    "Failed to checkpoint Gmail state after user {}",
                                    cur_user_email
                                )
                            })?;
                    }
                }
                Err(e) => {
                    if is_google_api_service_disabled_error(&e) {
                        return Err(google_api_service_disabled_error("Gmail", &e));
                    }
                    warn!(
                        "Failed to get access token for user {}: {:#}. This user may not have Gmail access.",
                        cur_user_email, e
                    );
                    failed_users += 1;
                    last_error = Some(format!("{:#}", e));
                }
            }
        }

        if !ctx.is_cancelled() && successful_users == 0 && failed_users > 0 {
            return Err(anyhow!(
                "Gmail sync failed for all {} indexed users; last error: {}",
                failed_users,
                last_error.unwrap_or_else(|| "unknown error".to_string())
            ));
        }

        info!(
            "Gmail sync completed for source {}: {} total processed, {} total updated",
            source.id, total_processed, total_updated
        );

        info!("Completed Gmail sync for source: {}", source.id);

        Ok(GoogleSyncCheckpoint {
            gmail_history_ids: if new_history_ids.is_empty() {
                None
            } else {
                Some(new_history_ids)
            },
            drive_page_tokens,
            drive_change_tokens: existing_state.drive_change_tokens.clone(),
            drive_scope_fingerprint: existing_state.drive_scope_fingerprint.clone(),
            drive_acl_fingerprints: existing_state.drive_acl_fingerprints.clone(),
            chat: chat_checkpoint,
            ..Default::default()
        })
    }

    async fn sync_google_chat_source_internal(
        &self,
        source: &Source,
        service_creds: &ServiceCredential,
        sync_type: SyncType,
        existing_state: GoogleSyncCheckpoint,
        _known_groups: HashSet<String>,
        ctx: &SyncContext,
    ) -> Result<GoogleSyncCheckpoint> {
        let service_auth = Arc::new(
            self.create_auth(service_creds, SourceType::GoogleChat)
                .await?,
        );
        if service_auth.is_oauth() {
            return Err(anyhow!(
                "Google Chat sync currently requires a service account with domain-wide delegation"
            ));
        }
        let drive_auth = Arc::new(
            self.create_auth(service_creds, SourceType::GoogleDrive)
                .await?,
        );

        let domain = crate::auth::get_domain_from_credentials(service_creds)?;
        let admin_email = ctx.get_user_email_for_source().await.map_err(|e| {
            anyhow!(
                "Failed to get source creator/admin email for Google Chat source {}: {}",
                source.id,
                e
            )
        })?;
        let admin_access_token =
            service_auth
                .get_access_token(&admin_email)
                .await
                .map_err(|e| {
                    anyhow!(
                        "Failed to get access token for admin {}: {}",
                        admin_email,
                        e
                    )
                })?;
        let all_users = self
            .admin_client
            .list_all_users(&admin_access_token, &domain)
            .await?;
        let mut user_id_to_email: HashMap<String, String> = HashMap::new();
        let user_emails: Vec<String> = all_users
            .into_iter()
            .filter(|user| source.should_index_user(&user.primary_email))
            .map(|user| {
                user_id_to_email.insert(user.id.clone(), user.primary_email.to_lowercase());
                user_id_to_email.insert(
                    user.primary_email.to_lowercase(),
                    user.primary_email.to_lowercase(),
                );
                user.primary_email
            })
            .collect();

        let mut chat_checkpoint = existing_state.chat.unwrap_or_default();
        let discovered = self
            .discover_chat_spaces(&service_auth, &user_emails, source, ctx)
            .await?;
        chat_checkpoint.last_space_discovery_at = Some(OffsetDateTime::now_utc().to_string());

        for (space_name, (space, reader_email)) in discovered {
            if ctx.is_cancelled() {
                break;
            }

            let mut space_checkpoint =
                chat_checkpoint
                    .spaces
                    .remove(&space_name)
                    .unwrap_or_else(|| GoogleChatSpaceCheckpoint {
                        space_name: space.name.clone(),
                        space_id: chat_space_id(&space.name).to_string(),
                        space_type: format!("{:?}", space.space_type),
                        display_name: space.display_name.clone(),
                        reader_email: Some(reader_email.clone()),
                        ..Default::default()
                    });
            space_checkpoint.display_name =
                space.display_name.clone().or(space_checkpoint.display_name);
            space_checkpoint.reader_email = Some(reader_email.clone());

            self.sync_chat_space_acl(
                source,
                ctx,
                &service_auth,
                &admin_email,
                &space,
                &user_id_to_email,
                &mut space_checkpoint,
            )
            .await?;

            if sync_type == SyncType::Incremental
                && space_checkpoint.last_event_time.is_some()
                && !chat_event_watermark_expired(space_checkpoint.last_event_time.as_deref())
            {
                match self
                    .sync_chat_space_incremental(
                        source,
                        ctx,
                        &service_auth,
                        &drive_auth,
                        &reader_email,
                        &space,
                        &mut space_checkpoint,
                    )
                    .await
                {
                    Ok(()) => {}
                    Err(e) => {
                        warn!(
                            "Incremental Google Chat sync failed for {}: {}. Falling back to full-space sync.",
                            space.name, e
                        );
                        self.sync_chat_space_full(
                            source,
                            ctx,
                            &service_auth,
                            &drive_auth,
                            &reader_email,
                            &space,
                            &mut space_checkpoint,
                        )
                        .await?;
                    }
                }
            } else {
                self.sync_chat_space_full(
                    source,
                    ctx,
                    &service_auth,
                    &drive_auth,
                    &reader_email,
                    &space,
                    &mut space_checkpoint,
                )
                .await?;
            }

            chat_checkpoint
                .spaces
                .insert(space_name.clone(), space_checkpoint);
            let checkpoint_state = GoogleSyncCheckpoint {
                gmail_history_ids: existing_state.gmail_history_ids.clone(),
                drive_page_tokens: existing_state.drive_page_tokens.clone(),
                drive_change_tokens: existing_state.drive_change_tokens.clone(),
                drive_scope_fingerprint: existing_state.drive_scope_fingerprint.clone(),
                drive_acl_fingerprints: existing_state.drive_acl_fingerprints.clone(),
                chat: Some(chat_checkpoint.clone()),
                ..Default::default()
            };
            ctx.save_checkpoint(serde_json::to_value(&checkpoint_state)?)
                .await?;
        }

        Ok(GoogleSyncCheckpoint {
            gmail_history_ids: existing_state.gmail_history_ids,
            drive_page_tokens: existing_state.drive_page_tokens,
            drive_change_tokens: existing_state.drive_change_tokens,
            drive_scope_fingerprint: existing_state.drive_scope_fingerprint,
            drive_acl_fingerprints: existing_state.drive_acl_fingerprints,
            chat: Some(chat_checkpoint),
            ..Default::default()
        })
    }

    async fn discover_chat_spaces(
        &self,
        service_auth: &Arc<GoogleAuth>,
        user_emails: &[String],
        source: &Source,
        ctx: &SyncContext,
    ) -> Result<HashMap<String, (GoogleChatSpace, String)>> {
        let mut spaces: HashMap<String, (GoogleChatSpace, String)> = HashMap::new();
        let mut successful_users = 0usize;
        let mut failed_users = 0usize;
        let mut last_error: Option<String> = None;

        for user_email in user_emails {
            if ctx.is_cancelled() {
                break;
            }
            let mut page_token: Option<String> = None;
            let mut user_had_successful_page = false;
            loop {
                let response = match self
                    .chat_client
                    .list_spaces_for_user(service_auth, user_email, page_token.as_deref())
                    .await
                {
                    Ok(response) => {
                        user_had_successful_page = true;
                        response
                    }
                    Err(e) => {
                        failed_users += 1;
                        last_error = Some(e.to_string());
                        warn!(
                            "Failed to list Google Chat spaces for {}: {}",
                            user_email, e
                        );
                        break;
                    }
                };
                for space in response.spaces {
                    if space.space_type != GoogleChatSpaceType::Space {
                        continue;
                    }
                    if !chat_space_allowed(source, &space) {
                        continue;
                    }
                    spaces
                        .entry(space.name.clone())
                        .or_insert((space, user_email.clone()));
                }
                page_token = response.next_page_token;
                if page_token.is_none() {
                    break;
                }
            }
            if user_had_successful_page {
                successful_users += 1;
            }
        }

        if !ctx.is_cancelled() && successful_users == 0 && failed_users > 0 {
            let last_error = last_error.unwrap_or_else(|| "unknown error".to_string());
            if is_google_api_service_disabled_message(&last_error) {
                return Err(anyhow!(
                    "Google Chat API is disabled or has not been enabled in the Google Cloud project used by this service account. Enable the API in Google Cloud Console, wait for propagation, then retry. Original error: {}",
                    last_error
                ));
            }
            return Err(anyhow!(
                "Failed to discover Google Chat spaces for all {} indexed users; last error: {}",
                failed_users,
                last_error
            ));
        }

        info!(
            "Discovered {} Google Chat named spaces ({} users succeeded, {} users failed)",
            spaces.len(),
            successful_users,
            failed_users
        );
        Ok(spaces)
    }

    async fn sync_chat_space_acl(
        &self,
        source: &Source,
        ctx: &SyncContext,
        service_auth: &Arc<GoogleAuth>,
        admin_email: &str,
        space: &GoogleChatSpace,
        user_id_to_email: &HashMap<String, String>,
        checkpoint: &mut GoogleChatSpaceCheckpoint,
    ) -> Result<()> {
        let mut members: HashSet<String> = HashSet::new();
        let mut page_token: Option<String> = None;
        loop {
            let response = self
                .chat_client
                .list_members(
                    service_auth,
                    admin_email,
                    &space.name,
                    page_token.as_deref(),
                    true,
                    true,
                )
                .await?;
            for membership in response.memberships {
                if membership.state.as_deref() != Some("JOINED") {
                    continue;
                }
                if let Some(member) = membership.member {
                    if member.user_type == Some(crate::chat::GoogleChatUserType::Human) {
                        if let Some(email) = chat_user_email(&member.name, user_id_to_email) {
                            if source.should_index_user(&email) {
                                members.insert(email);
                            }
                        }
                    }
                }
            }
            page_token = response.next_page_token;
            if page_token.is_none() {
                break;
            }
        }

        let mut member_emails: Vec<String> = members.into_iter().collect();
        member_emails.sort();
        let event = ConnectorEvent::GroupMembershipSync {
            sync_run_id: ctx.sync_run_id().to_string(),
            source_id: source.id.clone(),
            group_email: chat_space_group_id(&space.name),
            group_name: space
                .display_name
                .as_ref()
                .map(|name| format!("Google Chat: {}", name)),
            member_emails,
        };
        ctx.emit_event(event).await?;
        checkpoint.last_acl_sync_at = Some(OffsetDateTime::now_utc().to_string());
        Ok(())
    }

    async fn sync_chat_space_full(
        &self,
        source: &Source,
        ctx: &SyncContext,
        service_auth: &Arc<GoogleAuth>,
        drive_auth: &Arc<GoogleAuth>,
        reader_email: &str,
        space: &GoogleChatSpace,
        checkpoint: &mut GoogleChatSpaceCheckpoint,
    ) -> Result<()> {
        checkpoint.full_in_progress = true;
        let mut builder = GoogleChatSegmentBuilder::new(space);
        let old_segments: HashSet<String> = checkpoint
            .segments
            .iter()
            .map(|s| s.external_id.clone())
            .collect();
        let mut new_segments: Vec<GoogleChatSegmentCheckpoint> = Vec::new();
        let mut emitted_segments: HashSet<String> = HashSet::new();
        let mut page_token: Option<String> = None;
        let (drive_cutoff, _gmail_cutoff) = self.get_cutoff_date()?;
        let filter = format!("createTime > \"{}\"", drive_cutoff);

        loop {
            if ctx.is_cancelled() {
                break;
            }
            let response = self
                .chat_client
                .list_messages(
                    service_auth,
                    reader_email,
                    &space.name,
                    page_token.as_deref(),
                    Some(&filter),
                    Some("createTime asc"),
                    true,
                )
                .await?;
            for message in response.messages {
                let ready = builder.push(message)?;
                for segment in ready {
                    let checkpoint_entry = self
                        .emit_chat_segment(
                            source,
                            ctx,
                            service_auth,
                            drive_auth,
                            reader_email,
                            &segment,
                        )
                        .await?;
                    checkpoint.full_resume_after_time = Some(checkpoint_entry.end_time.clone());
                    emitted_segments.insert(checkpoint_entry.external_id.clone());
                    new_segments.push(checkpoint_entry);
                }
            }
            page_token = response.next_page_token;
            if page_token.is_none() {
                break;
            }
        }

        if let Some(segment) = builder.finish()? {
            let checkpoint_entry = self
                .emit_chat_segment(
                    source,
                    ctx,
                    service_auth,
                    drive_auth,
                    reader_email,
                    &segment,
                )
                .await?;
            checkpoint.full_resume_after_time = Some(checkpoint_entry.end_time.clone());
            emitted_segments.insert(checkpoint_entry.external_id.clone());
            new_segments.push(checkpoint_entry);
        }

        for stale in old_segments.difference(&emitted_segments) {
            ctx.emit_event(ConnectorEvent::DocumentDeleted {
                sync_run_id: ctx.sync_run_id().to_string(),
                source_id: source.id.clone(),
                document_id: stale.clone(),
            })
            .await?;
        }

        checkpoint.segments = new_segments;
        checkpoint.full_in_progress = false;
        checkpoint.full_resume_after_time = None;
        checkpoint.last_full_sync_at = Some(OffsetDateTime::now_utc().to_string());
        checkpoint.last_message_create_time =
            checkpoint.segments.last().map(|s| s.end_time.clone());
        checkpoint
            .last_event_time
            .get_or_insert_with(|| OffsetDateTime::now_utc().to_string());
        Ok(())
    }

    async fn sync_chat_space_incremental(
        &self,
        source: &Source,
        ctx: &SyncContext,
        service_auth: &Arc<GoogleAuth>,
        drive_auth: &Arc<GoogleAuth>,
        reader_email: &str,
        space: &GoogleChatSpace,
        checkpoint: &mut GoogleChatSpaceCheckpoint,
    ) -> Result<()> {
        checkpoint.incremental_in_progress = true;
        let last_event = checkpoint
            .last_event_time
            .clone()
            .ok_or_else(|| anyhow!("Missing Google Chat event watermark for {}", space.name))?;
        let filter = format!(
            "startTime=\"{}\" AND (eventTypes:\"google.workspace.chat.message.v1.created\" OR eventTypes:\"google.workspace.chat.message.v1.updated\" OR eventTypes:\"google.workspace.chat.message.v1.deleted\" OR eventTypes:\"google.workspace.chat.membership.v1.created\" OR eventTypes:\"google.workspace.chat.membership.v1.updated\" OR eventTypes:\"google.workspace.chat.membership.v1.deleted\")",
            last_event
        );
        let mut changes = GoogleChatIncrementalChanges {
            affected_times: Vec::new(),
            requires_full_rebuild: false,
            latest_event_time: last_event.clone(),
        };
        let mut page_token = checkpoint.incremental_event_page_token.clone();
        loop {
            let response = self
                .chat_client
                .list_space_events(
                    service_auth,
                    reader_email,
                    &space.name,
                    page_token.as_deref(),
                    &filter,
                )
                .await?;
            self.collect_chat_incremental_changes(&response.space_events, &mut changes);
            page_token = response.next_page_token;
            checkpoint.incremental_event_page_token = page_token.clone();
            checkpoint.pending_event_watermark = Some(changes.latest_event_time.clone());
            if page_token.is_none() {
                break;
            }
        }
        if changes.requires_full_rebuild {
            self.sync_chat_space_full(
                source,
                ctx,
                service_auth,
                drive_auth,
                reader_email,
                space,
                checkpoint,
            )
            .await?;
        } else if !changes.affected_times.is_empty() {
            self.rebuild_chat_affected_segments(
                source,
                ctx,
                service_auth,
                drive_auth,
                reader_email,
                space,
                checkpoint,
                &changes.affected_times,
            )
            .await?;
        }
        checkpoint.last_event_time = Some(changes.latest_event_time);
        checkpoint.pending_event_watermark = None;
        checkpoint.incremental_event_page_token = None;
        checkpoint.incremental_in_progress = false;
        Ok(())
    }

    fn collect_chat_incremental_changes(
        &self,
        events: &[GoogleChatSpaceEvent],
        changes: &mut GoogleChatIncrementalChanges,
    ) {
        for event in events {
            changes.latest_event_time = event.event_time.clone();
            let message_event = event.message_created.is_some()
                || event.message_updated.is_some()
                || event.message_deleted.is_some();
            if !message_event {
                continue;
            }
            if let Some(create_time) = chat_event_message_create_time(event) {
                changes.affected_times.push(create_time);
            } else {
                changes.requires_full_rebuild = true;
            }
        }
    }

    async fn rebuild_chat_affected_segments(
        &self,
        source: &Source,
        ctx: &SyncContext,
        service_auth: &Arc<GoogleAuth>,
        drive_auth: &Arc<GoogleAuth>,
        reader_email: &str,
        space: &GoogleChatSpace,
        checkpoint: &mut GoogleChatSpaceCheckpoint,
        affected_times: &[OffsetDateTime],
    ) -> Result<()> {
        let windows = self.build_chat_rebuild_windows(checkpoint, affected_times)?;
        if windows.len() > GOOGLE_CHAT_MAX_TARGETED_INCREMENTAL_WINDOWS {
            return Err(anyhow!(
                "Google Chat incremental sync touched {} conversation windows; falling back to full-space rebuild",
                windows.len()
            ));
        }

        let (drive_cutoff, _gmail_cutoff) = self.get_cutoff_date()?;
        let cutoff = parse_google_time(Some(&drive_cutoff))
            .ok_or_else(|| anyhow!("Invalid Google Chat cutoff time: {}", drive_cutoff))?;
        let mut rebuilt_segments: Vec<GoogleChatSegmentCheckpoint> = Vec::new();
        let mut stale_segment_ids: HashSet<String> = HashSet::new();
        let mut emitted_segment_ids: HashSet<String> = HashSet::new();

        for window in windows {
            stale_segment_ids.extend(window.stale_segment_ids.iter().cloned());
            let fetch_start = std::cmp::max(window.start - time::Duration::seconds(1), cutoff);
            let fetch_end = window.end + time::Duration::seconds(1);
            if fetch_end <= fetch_start {
                continue;
            }
            let filter = format!(
                "createTime > \"{}\" AND createTime < \"{}\"",
                fetch_start, fetch_end
            );
            let mut builder = GoogleChatSegmentBuilder::new(space);
            let mut page_token: Option<String> = None;
            loop {
                if ctx.is_cancelled() {
                    break;
                }
                let response = self
                    .chat_client
                    .list_messages(
                        service_auth,
                        reader_email,
                        &space.name,
                        page_token.as_deref(),
                        Some(&filter),
                        Some("createTime asc"),
                        true,
                    )
                    .await?;
                for message in response.messages {
                    let ready = builder.push(message)?;
                    for segment in ready {
                        let checkpoint_entry = self
                            .emit_chat_segment(
                                source,
                                ctx,
                                service_auth,
                                drive_auth,
                                reader_email,
                                &segment,
                            )
                            .await?;
                        emitted_segment_ids.insert(checkpoint_entry.external_id.clone());
                        rebuilt_segments.push(checkpoint_entry);
                    }
                }
                page_token = response.next_page_token;
                if page_token.is_none() {
                    break;
                }
            }
            if let Some(segment) = builder.finish()? {
                let checkpoint_entry = self
                    .emit_chat_segment(
                        source,
                        ctx,
                        service_auth,
                        drive_auth,
                        reader_email,
                        &segment,
                    )
                    .await?;
                emitted_segment_ids.insert(checkpoint_entry.external_id.clone());
                rebuilt_segments.push(checkpoint_entry);
            }
        }

        for stale in stale_segment_ids.difference(&emitted_segment_ids) {
            ctx.emit_event(ConnectorEvent::DocumentDeleted {
                sync_run_id: ctx.sync_run_id().to_string(),
                source_id: source.id.clone(),
                document_id: stale.clone(),
            })
            .await?;
        }

        let mut merged_segments: Vec<GoogleChatSegmentCheckpoint> = checkpoint
            .segments
            .iter()
            .filter(|segment| !stale_segment_ids.contains(&segment.external_id))
            .cloned()
            .collect();
        merged_segments.extend(rebuilt_segments);
        sort_chat_segment_checkpoints(&mut merged_segments);
        checkpoint.segments = merged_segments;
        checkpoint.last_message_create_time =
            checkpoint.segments.last().map(|s| s.end_time.clone());
        Ok(())
    }

    fn build_chat_rebuild_windows(
        &self,
        checkpoint: &GoogleChatSpaceCheckpoint,
        affected_times: &[OffsetDateTime],
    ) -> Result<Vec<GoogleChatRebuildWindow>> {
        let mut bounds: Vec<GoogleChatSegmentBounds> = checkpoint
            .segments
            .iter()
            .map(chat_segment_bounds)
            .collect::<Result<Vec<_>>>()?;
        bounds.sort_by_key(|bound| bound.start);

        let dead_time = time::Duration::seconds(GOOGLE_CHAT_DEAD_TIME_SECONDS);
        let mut windows = Vec::new();
        for affected_time in affected_times {
            let mut start = *affected_time;
            let mut end = *affected_time;
            let mut stale_segment_ids: HashSet<String> = HashSet::new();
            let mut changed = true;
            while changed {
                changed = false;
                for bound in &bounds {
                    if stale_segment_ids.contains(&bound.external_id) {
                        continue;
                    }
                    if bound.end >= start - dead_time && bound.start <= end + dead_time {
                        start = std::cmp::min(start, bound.start);
                        end = std::cmp::max(end, bound.end);
                        stale_segment_ids.insert(bound.external_id.clone());
                        changed = true;
                    }
                }
            }

            windows.push(GoogleChatRebuildWindow {
                start,
                end,
                stale_segment_ids,
            });
        }

        Ok(merge_chat_rebuild_windows(windows, dead_time))
    }

    async fn emit_chat_segment(
        &self,
        source: &Source,
        ctx: &SyncContext,
        service_auth: &Arc<GoogleAuth>,
        drive_auth: &Arc<GoogleAuth>,
        reader_email: &str,
        segment: &GoogleChatSegment,
    ) -> Result<GoogleChatSegmentCheckpoint> {
        let content = segment.render_content();
        let content_id = ctx.store_content(&content).await?;
        let metadata = segment.metadata();
        let permissions = DocumentPermissions {
            public: false,
            users: vec![],
            groups: vec![chat_space_group_id(&segment.space_name)],
        };
        let attributes = Some(segment.attributes());
        ctx.emit_event(ConnectorEvent::DocumentCreated {
            sync_run_id: ctx.sync_run_id().to_string(),
            source_id: source.id.clone(),
            document_id: segment.external_id.clone(),
            content_id,
            metadata,
            permissions: permissions.clone(),
            attributes,
        })
        .await?;

        let mut attachment_count = 0;
        for message in &segment.messages {
            for attachment in &message.attachments {
                self.emit_chat_attachment_metadata(
                    source,
                    ctx,
                    service_auth,
                    drive_auth,
                    reader_email,
                    segment,
                    message,
                    attachment,
                    permissions.clone(),
                )
                .await?;
                attachment_count += 1;
            }
        }

        ctx.increment_scanned(segment.messages.len() as i32).await?;
        ctx.increment_updated(1 + attachment_count).await?;
        Ok(segment.to_checkpoint())
    }

    async fn emit_chat_attachment_metadata(
        &self,
        source: &Source,
        ctx: &SyncContext,
        service_auth: &Arc<GoogleAuth>,
        drive_auth: &Arc<GoogleAuth>,
        reader_email: &str,
        segment: &GoogleChatSegment,
        message: &GoogleChatSegmentMessage,
        attachment: &GoogleChatSegmentAttachmentRef,
        permissions: DocumentPermissions,
    ) -> Result<()> {
        let document_id = attachment_external_id(&attachment.name);
        let title = attachment
            .content_name
            .clone()
            .unwrap_or_else(|| attachment.name.clone());
        let stored_content = self
            .store_chat_attachment_content(
                ctx,
                service_auth,
                drive_auth,
                reader_email,
                segment,
                message,
                attachment,
                &title,
            )
            .await?;
        let mut extra = HashMap::new();
        extra.insert(
            "parent_segment_external_id".to_string(),
            json!(segment.external_id),
        );
        extra.insert("parent_message_name".to_string(), json!(message.name));
        extra.insert("space_name".to_string(), json!(segment.space_name));
        extra.insert(
            "space_display_name".to_string(),
            json!(segment.space_display_name),
        );
        extra.insert("attachment_name".to_string(), json!(attachment.name));
        extra.insert("content_name".to_string(), json!(attachment.content_name));
        extra.insert("content_type".to_string(), json!(attachment.content_type));
        extra.insert("attachment_source".to_string(), json!(attachment.source));
        extra.insert("resource_name".to_string(), json!(attachment.resource_name));
        extra.insert("drive_file_id".to_string(), json!(attachment.drive_file_id));
        extra.insert(
            "content_extracted".to_string(),
            json!(stored_content.content_extracted),
        );
        extra.insert(
            "extraction_error".to_string(),
            json!(stored_content.extraction_error),
        );
        let metadata = DocumentMetadata {
            title: Some(title),
            author: Some(message.sender.clone()),
            created_at: Some(message.create_time),
            updated_at: message.update_time.or(Some(message.create_time)),
            content_type: Some("attachment".to_string()),
            mime_type: attachment.content_type.clone(),
            size: stored_content.size,
            url: stored_content.source_url,
            path: Some(format!(
                "/Google Chat/{}/attachments",
                segment
                    .space_display_name
                    .as_deref()
                    .unwrap_or(&segment.space_name)
            )),
            extra: Some(extra),
        };
        ctx.emit_event(ConnectorEvent::DocumentCreated {
            sync_run_id: ctx.sync_run_id().to_string(),
            source_id: source.id.clone(),
            document_id,
            content_id: stored_content.content_id,
            metadata,
            permissions,
            attributes: None,
        })
        .await?;
        Ok(())
    }

    async fn store_chat_attachment_content(
        &self,
        ctx: &SyncContext,
        service_auth: &Arc<GoogleAuth>,
        drive_auth: &Arc<GoogleAuth>,
        reader_email: &str,
        segment: &GoogleChatSegment,
        message: &GoogleChatSegmentMessage,
        attachment: &GoogleChatSegmentAttachmentRef,
        title: &str,
    ) -> Result<GoogleChatAttachmentStoredContent> {
        match self
            .extract_and_store_chat_attachment_content(
                ctx,
                service_auth,
                drive_auth,
                reader_email,
                attachment,
            )
            .await
        {
            Ok(content) => Ok(content),
            Err(e) => {
                warn!(
                    "Failed to extract Google Chat attachment {}: {:#}; indexing metadata fallback",
                    attachment.name, e
                );
                let extraction_error = format!("{:#}", e);
                let fallback = format!(
                    "Attachment: {}\nAttached to Google Chat message: {}\nParent segment: {}\nSpace: {}\nSender: {}\nMessage excerpt: {}\n\nAttachment content extraction failed: {}\n",
                    title,
                    message.name,
                    segment.external_id,
                    segment
                        .space_display_name
                        .as_deref()
                        .unwrap_or(&segment.space_name),
                    message.sender,
                    message.text.chars().take(500).collect::<String>(),
                    extraction_error
                );
                let content_id = ctx.store_content(&fallback).await?;
                Ok(GoogleChatAttachmentStoredContent {
                    content_id,
                    content_extracted: false,
                    extraction_error: Some(extraction_error),
                    source_url: None,
                    size: None,
                })
            }
        }
    }

    async fn extract_and_store_chat_attachment_content(
        &self,
        ctx: &SyncContext,
        service_auth: &Arc<GoogleAuth>,
        drive_auth: &Arc<GoogleAuth>,
        reader_email: &str,
        attachment: &GoogleChatSegmentAttachmentRef,
    ) -> Result<GoogleChatAttachmentStoredContent> {
        if let Some(resource_name) = attachment.resource_name.as_deref() {
            let data = self
                .chat_client
                .download_uploaded_attachment(service_auth, reader_email, resource_name)
                .await
                .with_context(|| {
                    format!(
                        "Failed to download uploaded Chat attachment {}",
                        attachment.name
                    )
                })?;
            let size = data.len() as u64;
            let mime_type = attachment
                .content_type
                .as_deref()
                .unwrap_or("application/octet-stream");
            let content_id = ctx
                .extract_and_store_content(data, mime_type, attachment.content_name.as_deref())
                .await
                .with_context(|| {
                    format!(
                        "Failed to extract uploaded Chat attachment {}",
                        attachment.name
                    )
                })?;
            return Ok(GoogleChatAttachmentStoredContent {
                content_id,
                content_extracted: true,
                extraction_error: None,
                source_url: None,
                size: Some(size.to_string()),
            });
        }

        if let Some(drive_file_id) = attachment.drive_file_id.as_deref() {
            let file = self
                .drive_client
                .get_file_metadata(drive_auth, reader_email, drive_file_id)
                .await
                .with_context(|| {
                    format!(
                        "Failed to get Drive metadata for Chat attachment {}",
                        attachment.name
                    )
                })?;
            let source_url = file.web_view_link.clone();
            let size = file.size.clone();
            let content = self
                .drive_client
                .get_file_content(drive_auth, reader_email, &file)
                .await
                .with_context(|| {
                    format!(
                        "Failed to download Drive file for Chat attachment {}",
                        attachment.name
                    )
                })?;
            let content_id = match content {
                FileContent::Text(text) => {
                    if text.trim().is_empty() {
                        return Err(anyhow!(
                            "Drive Chat attachment {} has no extractable text",
                            attachment.name
                        ));
                    }
                    ctx.store_content(&text).await?
                }
                FileContent::Binary {
                    data,
                    mime_type,
                    filename,
                } => {
                    ctx.extract_and_store_content(data, &mime_type, Some(&filename))
                        .await?
                }
            };
            return Ok(GoogleChatAttachmentStoredContent {
                content_id,
                content_extracted: true,
                extraction_error: None,
                source_url,
                size,
            });
        }

        Err(anyhow!(
            "Chat attachment {} has neither uploaded content nor Drive file reference",
            attachment.name
        ))
    }

    fn should_index_file(&self, file: &crate::models::GoogleDriveFile) -> bool {
        matches!(
            file.mime_type.as_str(),
            "application/vnd.google-apps.document"
                | "application/vnd.google-apps.spreadsheet"
                | "application/vnd.google-apps.presentation"
                | "text/plain"
                | "text/html"
                | "text/csv"
                | "application/pdf"
                | "application/vnd.openxmlformats-officedocument.wordprocessingml.document"
                | "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet"
                | "application/vnd.openxmlformats-officedocument.presentationml.presentation"
                | "application/msword"
                | "application/vnd.ms-excel"
                | "application/vnd.ms-powerpoint"
        )
    }

    async fn publish_deletion_event(&self, ctx: &SyncContext, document_id: &str) -> Result<()> {
        let event = ConnectorEvent::DocumentDeleted {
            sync_run_id: ctx.sync_run_id().to_string(),
            source_id: ctx.source_id().to_string(),
            document_id: document_id.to_string(),
        };
        ctx.emit_event(event).await
    }

    /// Publish a deletion event for every file in `trashed_files`. Shared by
    /// all full-traversal paths (SA-direct and DWD) to reconcile files that
    /// were trashed before their trash transition was observed by an
    /// incremental run — `changes.list` reports a trash only once, so without
    /// this a pre-trashed file would stay in the index forever.
    async fn publish_trashed_deletions(
        &self,
        ctx: &SyncContext,
        trashed_files: Vec<GoogleDriveFile>,
    ) -> Result<()> {
        for trashed in trashed_files {
            info!(
                "File {} ({}) is trashed (full traversal), publishing deletion",
                trashed.id, trashed.name
            );
            self.publish_deletion_event(ctx, &trashed.id).await?;
        }
        Ok(())
    }

    async fn get_service_credentials(&self, source_id: &str) -> Result<ServiceCredential> {
        let creds = self
            .sdk_client
            .get_credentials(source_id)
            .await
            .context("Failed to fetch credentials via SDK")?;

        // Verify it's a Google credentials record
        if creds.provider != ServiceProvider::Google {
            return Err(anyhow::anyhow!(
                "Expected Google credentials for source {}, found {:?}",
                source_id,
                creds.provider
            ));
        }

        Ok(creds)
    }

    /// Create GoogleAuth from credentials, branching on auth_type (JWT vs OAuth)
    pub async fn create_auth(
        &self,
        creds: &ServiceCredential,
        source_type: SourceType,
    ) -> Result<GoogleAuth> {
        match creds.auth_type {
            AuthType::OAuth => {
                let oauth_credentials: GoogleOAuthCredentials =
                    serde_json::from_value(creds.credentials.clone())
                        .context("Invalid Google OAuth credentials")?;
                let access_token = oauth_credentials.access_token.unwrap_or_default();
                let refresh_token = oauth_credentials.refresh_token;
                let expires_at = oauth_credentials.expires_at.unwrap_or(0);
                let user_email = oauth_credentials
                    .user_email
                    .or_else(|| creds.principal_email.clone())
                    .ok_or_else(|| anyhow::anyhow!("Missing user_email in OAuth credentials"))?;

                // Fetch connector config for OAuth client_id/secret
                let connector_config = self
                    .sdk_client
                    .get_connector_config("google")
                    .await
                    .context("Failed to fetch Google connector config for OAuth")?;

                let client_id = connector_config
                    .get("oauth_client_id")
                    .and_then(|v| v.as_str())
                    .ok_or_else(|| {
                        anyhow::anyhow!("Missing oauth_client_id in Google connector config")
                    })?
                    .to_string();

                let client_secret = connector_config
                    .get("oauth_client_secret")
                    .and_then(|v| v.as_str())
                    .ok_or_else(|| {
                        anyhow::anyhow!("Missing oauth_client_secret in Google connector config")
                    })?
                    .to_string();

                let oauth_auth = OAuthAuth::new(
                    access_token,
                    refresh_token,
                    expires_at,
                    user_email,
                    client_id,
                    client_secret,
                )?;

                Ok(GoogleAuth::OAuth(oauth_auth))
            }
            _ => {
                // Default: JWT / service account
                let sa = crate::auth::create_service_auth(creds, source_type)?;
                Ok(GoogleAuth::ServiceAccount(sa))
            }
        }
    }

    async fn get_user_email_from_source(&self, source_id: &str) -> Result<String> {
        self.sdk_client
            .get_user_email_for_source(source_id)
            .await
            .context("Failed to get user email via SDK")
    }

    pub async fn handle_webhook_notification(
        &self,
        notification: WebhookNotification,
    ) -> Result<()> {
        info!(
            "Handling webhook notification for channel {}, state: {}",
            notification.channel_id, notification.resource_state
        );

        let source_id = match &notification.source_id {
            Some(id) => id.clone(),
            None => {
                warn!(
                    "Received webhook notification without source_id token for channel {}",
                    notification.channel_id
                );
                return Ok(());
            }
        };

        match notification.resource_state.as_str() {
            "sync" => {
                debug!(
                    "Received sync message for channel: {}",
                    notification.channel_id
                );
            }
            "add" | "update" | "remove" | "trash" | "untrash" | "change" => {
                let now = Instant::now();
                let mut entry = self
                    .webhook_debounce
                    .entry(source_id.clone())
                    .or_insert_with(|| WebhookDebounce {
                        last_received: now,
                        last_event_type: notification.resource_state.clone(),
                        count: 0,
                    });
                entry.last_received = now;
                entry.last_event_type = notification.resource_state.clone();
                entry.count += 1;

                info!(
                    "Buffered webhook event for source {} (state: {}, buffered_count: {})",
                    source_id, notification.resource_state, entry.count
                );

                self.webhook_notify.notify_one();
            }
            _ => {
                debug!(
                    "Ignoring webhook notification with state: {}",
                    notification.resource_state
                );
            }
        }

        Ok(())
    }

    /// Background loop that coalesces rapid webhook notifications.
    /// Waits until 10 minutes of quiet time per source, then fires one
    /// `notify_webhook` call for all buffered events.
    pub async fn run_webhook_processor(self: &Arc<Self>) {
        const POLL_INTERVAL: Duration = Duration::from_secs(30);
        let debounce_duration =
            Duration::from_millis(self.debounce_duration_ms.load(Ordering::Relaxed));

        loop {
            tokio::select! {
                _ = self.webhook_notify.notified() => {}
                _ = tokio::time::sleep(POLL_INTERVAL) => {}
            }

            let now = Instant::now();
            let mut expired: Vec<(String, String, u32)> = Vec::new();

            // Collect expired entries
            for entry in self.webhook_debounce.iter() {
                if now.duration_since(entry.last_received) >= debounce_duration {
                    expired.push((
                        entry.key().clone(),
                        entry.last_event_type.clone(),
                        entry.count,
                    ));
                }
            }

            // Notify first, only remove on success
            for (source_id, event_type, count) in expired {
                info!(
                    "Debounce expired for source {} ({} buffered events), notifying connector-manager",
                    source_id, count
                );

                match self
                    .sdk_client
                    .notify_webhook(&source_id, &event_type)
                    .await
                {
                    Ok(sync_run_id) => {
                        self.webhook_debounce.remove(&source_id);
                        info!(
                            "Connector-manager created sync run {} for debounced webhook (source: {})",
                            sync_run_id, source_id
                        );
                    }
                    Err(e) => {
                        error!(
                            "Failed to notify connector-manager for debounced webhook (source: {}): {}",
                            source_id, e
                        );
                    }
                }
            }
        }
    }

    /// Ensure a webhook is registered for a source.
    /// No-op if webhook_url is None. Logs but never propagates errors.
    pub async fn ensure_webhook_registered(&self, source_id: &str) {
        let Some(ref webhook_url) = self.webhook_url else {
            return;
        };

        info!("Ensuring webhook registered for source {}", source_id);
        if let Err(e) = self
            .register_webhook_for_source(source_id, webhook_url.clone())
            .await
        {
            error!("Failed to register webhook for source {}: {}", source_id, e);
        }
    }

    pub async fn register_webhook_for_source(
        &self,
        source_id: &str,
        webhook_url: String,
    ) -> Result<WebhookChannelResponse> {
        // Capture old channel info before registering the new one
        let old_channel = match self.sdk_client.get_connector_state(source_id).await {
            Ok(Some(raw_state)) => {
                let state: GoogleConnectorState =
                    serde_json::from_value(raw_state).unwrap_or_else(|e| {
                        warn!(
                            "Failed to parse connector state for source {}: {}",
                            source_id, e
                        );
                        GoogleConnectorState::default()
                    });
                match (&state.webhook_channel_id, &state.webhook_resource_id) {
                    (Some(ch), Some(res)) => Some((ch.clone(), res.clone())),
                    _ => None,
                }
            }
            _ => None,
        };

        let service_creds = self.get_service_credentials(source_id).await?;
        let auth = self
            .create_auth(&service_creds, SourceType::GoogleDrive)
            .await?;
        let user_email = if let Some(oauth_email) = auth.oauth_user_email() {
            oauth_email.to_string()
        } else {
            self.get_user_email_from_source(source_id).await
                .map_err(|e| anyhow::anyhow!("Failed to get user email for source {}: {}. Make sure the source has a valid creator.", source_id, e))?
        };
        let access_token = auth.get_access_token(&user_email).await?;

        let start_page_token = self
            .drive_client
            .get_start_page_token(&access_token)
            .await?;

        let webhook_channel = WebhookChannel::new(webhook_url.clone(), source_id);

        let webhook_response = self
            .drive_client
            .register_changes_webhook(&access_token, &webhook_channel, &start_page_token)
            .await?;

        let expires_at = webhook_response
            .expiration
            .as_ref()
            .and_then(|exp| exp.parse::<i64>().ok());

        let mut webhook_state = self
            .sdk_client
            .get_connector_state(source_id)
            .await
            .ok()
            .flatten()
            .filter(|value| value.is_object())
            .unwrap_or_else(|| json!({}));
        webhook_state["webhook_channel_id"] = json!(webhook_response.id.clone());
        webhook_state["webhook_resource_id"] = json!(webhook_response.resource_id.clone());
        webhook_state["webhook_expires_at"] = json!(expires_at);
        self.sdk_client
            .save_connector_state(source_id, webhook_state)
            .await?;

        info!(
            "Successfully registered webhook for source {}: channel_id={}, resource_id={}",
            source_id, webhook_response.id, webhook_response.resource_id
        );

        // Stop old channel after the new one is active to avoid gaps in coverage
        if let Some((old_channel_id, old_resource_id)) = old_channel {
            info!(
                "Stopping old webhook channel {} for source {}",
                old_channel_id, source_id
            );
            if let Err(e) = self
                .stop_webhook_for_source(source_id, &old_channel_id, &old_resource_id)
                .await
            {
                warn!("Failed to stop old webhook channel: {}", e);
            }
        }

        Ok(webhook_response)
    }

    /// Public accessor for the Drive client (used by connector actions).
    pub fn drive_client(&self) -> &DriveClient {
        &self.drive_client
    }

    pub async fn stop_webhook_for_source(
        &self,
        source_id: &str,
        channel_id: &str,
        resource_id: &str,
    ) -> Result<()> {
        let service_creds = self.get_service_credentials(source_id).await?;
        let auth = self
            .create_auth(&service_creds, SourceType::GoogleDrive)
            .await?;
        let user_email = if let Some(oauth_email) = auth.oauth_user_email() {
            oauth_email.to_string()
        } else {
            self.get_user_email_from_source(source_id).await
                .map_err(|e| anyhow::anyhow!("Failed to get user email for source {}: {}. Make sure the source has a valid creator.", source_id, e))?
        };
        let access_token = auth.get_access_token(&user_email).await?;

        self.drive_client
            .stop_webhook_channel(&access_token, channel_id, resource_id)
            .await?;

        info!(
            "Successfully stopped webhook for source {}: channel_id={}",
            source_id, channel_id
        );
        Ok(())
    }

    async fn resolve_file_path(
        &self,
        auth: &GoogleAuth,
        user_email: &str,
        file: &crate::models::GoogleDriveFile,
    ) -> Result<String> {
        if let Some(parents) = &file.parents {
            if let Some(parent_id) = parents.first() {
                return self
                    .build_full_path(auth, user_email, parent_id, &file.name)
                    .await;
            }
        }

        // If no parents, file is in root
        Ok(format!("/{}", file.name))
    }

    async fn build_full_path(
        &self,
        auth: &GoogleAuth,
        user_email: &str,
        folder_id: &str,
        file_name: &str,
    ) -> Result<String> {
        debug!(
            "Building full path for file: {}, starting from folder: {}",
            file_name, folder_id
        );
        let mut path_components = vec![file_name.to_string()];
        let mut current_folder_id = folder_id.to_string();

        // Build path by traversing up the folder hierarchy.
        // Uses cycle detection via visited set; the iteration cap is only a safety net.
        let mut visited = HashSet::new();
        let mut _iteration: usize = 0;
        const MAX_ITERATIONS: usize = 10_000;
        loop {
            _iteration += 1;
            if _iteration >= MAX_ITERATIONS {
                warn!(
                    "Path building exceeded maximum iterations for file: {}, folder: {}",
                    file_name, folder_id
                );
                break;
            }

            // Check for cycles before processing.
            if !visited.insert(current_folder_id.clone()) {
                warn!(
                    "Cycle detected in folder ancestry for file: {}, folder: {}",
                    file_name, current_folder_id
                );
                break;
            }

            let cached_folder = self.folder_cache.get(&current_folder_id);

            let parent_folder_id: Option<String> = match cached_folder {
                Some(folder) => {
                    debug!("Found folder {} [id: {}] in cache", folder.name, folder.id);
                    path_components.push(folder.name.clone());
                    folder
                        .parents
                        .as_ref()
                        .map(|p| p.first())
                        .flatten()
                        .cloned()
                }
                None => {
                    debug!(
                        "Folder {} not found in cache, fetching metadata.",
                        current_folder_id
                    );
                    let folder_metadata = self
                        .drive_client
                        .get_folder_metadata(&auth, &user_email, &current_folder_id)
                        .await;

                    match folder_metadata {
                        Ok(folder_metadata) => {
                            let name = folder_metadata.name.clone();
                            debug!(
                                "Successfully fetched folder metadata: {} for folder: {}",
                                name, current_folder_id
                            );

                            let parent_folder_id = folder_metadata
                                .parents
                                .as_ref()
                                .map(|p| p.first())
                                .flatten()
                                .cloned();
                            debug!(
                                "Folder {} has parent: {:?}",
                                current_folder_id, parent_folder_id
                            );

                            // Cache the folder
                            self.folder_cache
                                .insert(current_folder_id.clone(), folder_metadata.into());

                            path_components.push(name);
                            parent_folder_id
                        }
                        Err(e) => {
                            warn!(
                                "Failed to get folder metadata for {}: {}",
                                current_folder_id, e
                            );
                            None
                        }
                    }
                }
            };

            if let Some(parent_id) = parent_folder_id {
                debug!("Folder {} has parent: {:?}", current_folder_id, parent_id);
                if parent_id == current_folder_id {
                    debug!("Reached root folder {}", current_folder_id);
                    break;
                }
                current_folder_id = parent_id;
            } else {
                debug!("Reached root folder {}", current_folder_id);
                break;
            }
        }

        // Reverse to get correct order (root to file)
        path_components.reverse();
        Ok(format!("/{}", path_components.join("/")))
    }

    async fn sync_gmail_for_user(
        &self,
        user_email: &str,
        service_auth: Arc<GoogleAuth>,
        ctx: &SyncContext,
        processed_threads: Arc<std::sync::Mutex<HashSet<String>>>,
        created_after: Option<&str>,
        known_groups: Arc<HashSet<String>>,
    ) -> Result<(usize, usize)> {
        info!("Processing Gmail for user: {}", user_email);

        let mut page_token: Option<String> = None;
        const BATCH_SIZE: usize = 500;

        // Track threads found for this user
        let mut user_threads: Vec<String> = Vec::new();

        // Step 1: List all threads for the user
        loop {
            debug!(
                "Listing Gmail threads for user {} with page_token: {:?}",
                user_email, page_token
            );

            let response = self
                .gmail_client
                .list_threads(
                    &service_auth,
                    &user_email,
                    Some("-in:chats"),
                    Some(BATCH_SIZE as u32),
                    page_token.as_deref(),
                    created_after,
                )
                .await
                .with_context(|| {
                    format!(
                        "Failed to list Gmail threads for user {} (page_token: {:?})",
                        user_email, page_token
                    )
                })?;

            // Collect thread IDs
            if let Some(threads) = response.threads {
                let page_thread_count = threads.len();
                debug!(
                    "Got {} threads in this page for user {}",
                    page_thread_count, user_email
                );

                for thread_info in threads {
                    user_threads.push(thread_info.id);
                }

                // Update scanned count for this page via SDK
                ctx.increment_scanned(page_thread_count as i32).await?;
            }

            // Check for cancellation
            if ctx.is_cancelled() {
                info!(
                    "Sync {} cancelled, stopping Gmail thread listing for user {}",
                    ctx.sync_run_id(),
                    user_email
                );
                break;
            }

            // Check if there are more pages
            page_token = response.next_page_token;
            if page_token.is_none() {
                break;
            }
        }

        info!(
            "Found {} Gmail threads for user {}",
            user_threads.len(),
            user_email
        );

        self.process_gmail_threads(
            user_threads,
            user_email,
            service_auth,
            ctx,
            processed_threads,
            known_groups,
        )
        .await
    }

    async fn sync_gmail_for_user_incremental(
        &self,
        user_email: &str,
        service_auth: Arc<GoogleAuth>,
        source_id: &str,
        sync_run_id: &str,
        ctx: &SyncContext,
        start_history_id: &str,
        processed_threads: Arc<std::sync::Mutex<HashSet<String>>>,
        known_groups: Arc<HashSet<String>>,
    ) -> Result<(usize, usize)> {
        info!(
            "Processing incremental Gmail sync for user {} from historyId {}",
            user_email, start_history_id
        );

        let mut changed_thread_ids = HashSet::new();
        let mut page_token: Option<String> = None;

        loop {
            let response = self
                .gmail_client
                .list_history(
                    &service_auth,
                    user_email,
                    start_history_id,
                    Some(500),
                    page_token.as_deref(),
                )
                .await?;

            if let Some(history_records) = response.history {
                for record in history_records {
                    if let Some(messages) = record.messages {
                        for msg in messages {
                            changed_thread_ids.insert(msg.thread_id);
                        }
                    }
                    if let Some(added) = record.messages_added {
                        for item in added {
                            changed_thread_ids.insert(item.message.thread_id);
                        }
                    }
                    if let Some(deleted) = record.messages_deleted {
                        for item in deleted {
                            changed_thread_ids.insert(item.message.thread_id);
                        }
                    }
                    if let Some(label_added) = record.labels_added {
                        for item in label_added {
                            changed_thread_ids.insert(item.message.thread_id);
                        }
                    }
                    if let Some(label_removed) = record.labels_removed {
                        for item in label_removed {
                            changed_thread_ids.insert(item.message.thread_id);
                        }
                    }
                }
            }

            if ctx.is_cancelled() {
                info!(
                    "Sync {} cancelled during history listing for user {}",
                    sync_run_id, user_email
                );
                break;
            }

            page_token = response.next_page_token;
            if page_token.is_none() {
                break;
            }
        }

        let thread_ids: Vec<String> = changed_thread_ids.into_iter().collect();
        info!(
            "Incremental sync found {} changed threads for user {}",
            thread_ids.len(),
            user_email
        );

        ctx.increment_scanned(thread_ids.len() as i32).await?;

        self.process_gmail_threads(
            thread_ids,
            user_email,
            service_auth,
            ctx,
            processed_threads,
            known_groups,
        )
        .await
    }

    async fn process_gmail_threads(
        &self,
        thread_ids: Vec<String>,
        user_email: &str,
        service_auth: Arc<GoogleAuth>,
        ctx: &SyncContext,
        processed_threads: Arc<std::sync::Mutex<HashSet<String>>>,
        known_groups: Arc<HashSet<String>>,
    ) -> Result<(usize, usize)> {
        let mut total_processed = 0;
        let mut total_updated = 0;
        let mut total_deduped = 0usize;
        let mut total_failed = 0usize;
        let total_listed = thread_ids.len();
        const THREAD_BATCH_SIZE: usize = 50;

        for chunk in thread_ids.chunks(THREAD_BATCH_SIZE) {
            if ctx.is_cancelled() {
                info!(
                    "Sync {} cancelled, stopping Gmail thread processing for user {}",
                    ctx.sync_run_id(),
                    user_email
                );
                break;
            }

            let mut unprocessed_threads = Vec::new();
            for thread_id in chunk {
                let already_processed = {
                    let processed_guard = processed_threads.lock().unwrap();
                    processed_guard.contains(thread_id)
                };

                if already_processed {
                    debug!(
                        "Thread {} already processed by another user, skipping",
                        thread_id
                    );
                    total_deduped += 1;
                    continue;
                }

                unprocessed_threads.push(thread_id.clone());
            }

            if unprocessed_threads.is_empty() {
                continue;
            }

            {
                let mut processed_guard = processed_threads.lock().unwrap();
                for thread_id in &unprocessed_threads {
                    processed_guard.insert(thread_id.clone());
                }
            }

            debug!("Processing batch of {} threads", unprocessed_threads.len());

            // Fetch batch with retry on 429 (up to 3 attempts with exponential backoff).
            // Each attempt drains its successes by-value into the per-thread processor
            // immediately — never accumulates GmailThreadResponse bodies across
            // attempts, which previously caused the OOM during full sync.
            let mut threads_to_fetch = unprocessed_threads;
            let max_retries = 3;
            let mut saw_rate_limit = false;
            let mut chunk_updated: usize = 0;

            for attempt in 0..=max_retries {
                if threads_to_fetch.is_empty() {
                    break;
                }

                if attempt > 0 {
                    let delay = Duration::from_secs(2u64.pow(attempt as u32));
                    warn!(
                        "Retrying {} rate-limited threads (attempt {}/{}, waiting {:?})",
                        threads_to_fetch.len(),
                        attempt,
                        max_retries,
                        delay
                    );
                    tokio::time::sleep(delay).await;
                }

                let batch_results = match self
                    .gmail_client
                    .batch_get_threads(
                        &service_auth,
                        user_email,
                        &threads_to_fetch,
                        MessageFormat::Full,
                    )
                    .await
                    .with_context(|| {
                        format!("Failed to get Gmail threads batch for user {}", user_email)
                    }) {
                    Ok(results) => results,
                    Err(e) => {
                        warn!("Failed to fetch thread batch: {}", e);
                        break;
                    }
                };

                let mut rate_limited_ids = Vec::new();
                let fetched_ids = std::mem::take(&mut threads_to_fetch);
                for (i, result) in batch_results.into_iter().enumerate() {
                    let thread_id = fetched_ids[i].clone();
                    match result {
                        BatchThreadResult::Success(response) => {
                            total_processed += 1;
                            let updated = self
                                .process_gmail_thread(
                                    &thread_id,
                                    response,
                                    user_email,
                                    &service_auth,
                                    ctx,
                                    &known_groups,
                                )
                                .await;
                            if updated {
                                total_updated += 1;
                                chunk_updated += 1;
                            }
                        }
                        BatchThreadResult::RateLimited => {
                            rate_limited_ids.push(thread_id);
                        }
                        BatchThreadResult::Failed(e) => {
                            total_failed += 1;
                            warn!("Failed to fetch thread {}: {}", thread_id, e);
                        }
                    }
                }

                if !rate_limited_ids.is_empty() {
                    saw_rate_limit = true;
                }
                threads_to_fetch = rate_limited_ids;
            }

            if !threads_to_fetch.is_empty() {
                warn!(
                    "Gave up on {} threads after {} retries for user {}",
                    threads_to_fetch.len(),
                    max_retries,
                    user_email
                );
            }

            // Push the chunk's contribution to documents_updated to the manager
            // so a mid-sync crash doesn't lose it. Per-page increment_scanned
            // already covers the scanned counter.
            if chunk_updated > 0 {
                ctx.increment_updated(chunk_updated as i32).await?;
            }

            // Adaptive backpressure: if this batch had 429s, pause before next batch
            if saw_rate_limit {
                debug!("Rate limit hit — pausing 3s before next batch");
                tokio::time::sleep(Duration::from_secs(3)).await;
            }
        }

        info!(
            "Completed Gmail processing for user {}: {} listed, {} indexed, {} updated \
            (skipped: {} deduped across users, {} failed/inaccessible)",
            user_email, total_listed, total_processed, total_updated, total_deduped, total_failed
        );

        Ok((total_processed, total_updated))
    }

    /// Process a single Gmail thread response by-value: build the GmailThread,
    /// emit the thread document and its attachments. Returns true if the
    /// thread was emitted as an update. Consumes `response` so the response
    /// body can drop as soon as the messages are moved into `gmail_thread`.
    async fn process_gmail_thread(
        &self,
        thread_id: &str,
        response: crate::gmail::GmailThreadResponse,
        user_email: &str,
        service_auth: &Arc<GoogleAuth>,
        ctx: &SyncContext,
        known_groups: &HashSet<String>,
    ) -> bool {
        let mut gmail_thread = GmailThread::new(thread_id.to_string());
        for message in response.messages {
            gmail_thread.add_message(message);
        }

        if gmail_thread.total_messages == 0 {
            debug!("Gmail thread {} has no messages, skipping", thread_id);
            return false;
        }

        let thread_url = gmail_thread.message_id.as_ref().map(|mid| {
            let clean_id = mid.trim_start_matches('<').trim_end_matches('>');
            let encoded = urlencoding::encode(clean_id);
            format!(
                "https://mail.google.com/mail/#search/rfc822msgid%3A{}",
                encoded
            )
        });

        // Extract attachments and store their content first, so the thread
        // document can carry pointers to its attachments in metadata.extra.
        //
        // Within a thread, dedup by (filename, size): the same file forwarded
        // across multiple replies would otherwise produce one document per
        // occurrence, flooding the BM25 index with copies of identical content.
        //
        // We persist the canonical RFC 822 Message-ID (not Gmail's per-mailbox
        // messageId) so the attachment can be fetched from any participating
        // user's mailbox via `messages.list?q=rfc822msgid:<id>`.
        let mut stored_attachments: Vec<(ExtractedAttachment, String, String)> = Vec::new();
        let mut seen: HashSet<(String, u64)> = HashSet::new();
        for message in &gmail_thread.messages {
            let rfc822_msgid = match self
                .gmail_client
                .get_header_value(message, "Message-ID")
                .map(|raw| {
                    raw.trim()
                        .trim_start_matches('<')
                        .trim_end_matches('>')
                        .to_string()
                })
                .filter(|s| !s.is_empty())
            {
                Some(id) => id,
                None => {
                    warn!(
                        "Gmail message {} in thread {} has no Message-ID header; \
                         skipping its attachments (cannot be fetched without canonical id)",
                        message.id, thread_id
                    );
                    continue;
                }
            };

            let attachments = self
                .gmail_client
                .extract_attachments(
                    message,
                    service_auth,
                    user_email,
                    ctx.sdk_client(),
                    ctx.sync_run_id(),
                )
                .await;

            for att in attachments {
                if att.extracted_text.trim().is_empty() {
                    continue;
                }

                if !seen.insert((att.filename.clone(), att.size)) {
                    debug!(
                        "Skipping duplicate attachment {} (size {}) in thread {}",
                        att.filename, att.size, thread_id
                    );
                    continue;
                }

                let att_content_id = match ctx.store_content(&att.extracted_text).await {
                    Ok(id) => id,
                    Err(e) => {
                        error!(
                            "Failed to store attachment content for {}: {}",
                            att.filename, e
                        );
                        continue;
                    }
                };

                stored_attachments.push((att, att_content_id, rfc822_msgid.clone()));
            }
        }

        let attachment_pointers: Vec<AttachmentPointer> = stored_attachments
            .iter()
            .map(|(att, _, rfc822_msgid)| AttachmentPointer {
                id: build_attachment_doc_id(rfc822_msgid, &att.filename, att.size),
                filename: att.filename.clone(),
                mime_type: att.mime_type.clone(),
                size: att.size,
            })
            .collect();

        let emit_result: Result<bool> = async {
            let content = gmail_thread
                .aggregate_content(&self.gmail_client, ctx.sdk_client(), ctx.sync_run_id())
                .await
                .context("aggregate content")?;
            if content.trim().is_empty() {
                debug!("Gmail thread {} has empty content, skipping", thread_id);
                return Ok(false);
            }
            let content_id = ctx.store_content(&content).await.context("store content")?;
            let event = gmail_thread
                .to_connector_event(
                    ctx.sync_run_id(),
                    ctx.source_id(),
                    &content_id,
                    known_groups,
                    user_email,
                    &attachment_pointers,
                )
                .context("build connector event")?;
            ctx.emit_event(event).await.context("emit event")?;
            Ok(true)
        }
        .await;

        let updated = match emit_result {
            Ok(true) => {
                info!("Successfully queued Gmail thread {}", thread_id);
                true
            }
            Ok(false) => false,
            Err(e) => {
                error!("Failed to process Gmail thread {}: {:#}", thread_id, e);
                false
            }
        };

        let mut att_users = Vec::new();
        let mut att_groups = Vec::new();
        let mut att_participants = gmail_thread.participants.clone();
        att_participants.insert(user_email.to_lowercase());
        for participant in &att_participants {
            if known_groups.contains(participant) {
                att_groups.push(participant.clone());
            } else {
                att_users.push(participant.clone());
            }
        }
        att_users.sort();
        att_users.dedup();
        att_groups.sort();
        att_groups.dedup();
        let att_permissions = DocumentPermissions {
            public: false,
            users: att_users,
            groups: att_groups,
        };

        for (att, att_content_id, rfc822_msgid) in stored_attachments {
            let att_doc_id = build_attachment_doc_id(&rfc822_msgid, &att.filename, att.size);

            let mut att_extra = HashMap::new();
            att_extra.insert(
                "parent_thread_id".to_string(),
                json!(gmail_thread.canonical_external_id()),
            );
            att_extra.insert("gmail_thread_id".to_string(), json!(thread_id));

            let att_metadata = DocumentMetadata {
                title: Some(att.filename.clone()),
                author: None,
                created_at: None,
                updated_at: None,
                content_type: mime_type_to_content_type(&att.mime_type),
                mime_type: Some(att.mime_type.clone()),
                size: Some(att.size.to_string()),
                url: thread_url.clone(),
                path: Some(format!("/Gmail/{}/{}", gmail_thread.subject, att.filename)),
                extra: Some(att_extra),
            };

            let att_event = ConnectorEvent::DocumentCreated {
                sync_run_id: ctx.sync_run_id().to_string(),
                source_id: ctx.source_id().to_string(),
                document_id: att_doc_id.clone(),
                content_id: att_content_id,
                metadata: att_metadata,
                permissions: att_permissions.clone(),
                attributes: Some(HashMap::new()),
            };

            match ctx.emit_event(att_event).await {
                Ok(_) => debug!(
                    "Queued attachment {} for thread {}",
                    att.filename, thread_id
                ),
                Err(e) => error!(
                    "Failed to queue attachment {} for thread {}: {}",
                    att.filename, thread_id, e
                ),
            }
        }

        updated
    }

    /// Sync group memberships if this is a service-account (domain-wide) source.
    /// OAuth single-user sources don't have Admin API access, so we skip them.
    async fn maybe_sync_groups(
        &self,
        source: &Source,
        service_creds: &ServiceCredential,
        ctx: &SyncContext,
    ) -> HashSet<String> {
        let native_source_type = match SourceType::try_from(source.source_type.as_str()) {
            Ok(source_type) => source_type,
            Err(e) => {
                warn!("Skipping group sync for unsupported source type: {}", e);
                return HashSet::new();
            }
        };
        let service_auth = match self.create_auth(service_creds, native_source_type).await {
            Ok(auth) => auth,
            Err(e) => {
                warn!("Failed to create auth for group sync: {}", e);
                return HashSet::new();
            }
        };

        // Only service-account (domain-wide) setups have Admin API access
        if service_auth.is_oauth() {
            debug!("Skipping group sync for OAuth source {}", source.id);
            return HashSet::new();
        }

        let domain = match crate::auth::get_domain_from_credentials(service_creds) {
            Ok(d) => d,
            Err(e) => {
                warn!("Failed to get domain for group sync: {}", e);
                return HashSet::new();
            }
        };

        let user_email = match ctx.get_user_email_for_source().await {
            Ok(email) => email,
            Err(e) => {
                warn!("Failed to get user email for group sync: {}", e);
                return HashSet::new();
            }
        };

        let access_token = match service_auth.get_access_token(&user_email).await {
            Ok(token) => token,
            Err(e) => {
                warn!("Failed to get access token for group sync: {}", e);
                return HashSet::new();
            }
        };

        match self
            .sync_groups(&source.id, ctx.sync_run_id(), &domain, &access_token)
            .await
        {
            Ok(group_emails) => group_emails,
            Err(e) => {
                warn!(
                    "Failed to sync group memberships: {}. Continuing with document sync.",
                    e
                );
                HashSet::new()
            }
        }
    }

    /// Best-effort group membership sync for SA-direct sources. Works only when
    /// the service account itself holds a customer-level admin role granting
    /// Directory API access (no domain-wide delegation / impersonation
    /// involved). Failures (403, missing scopes) are logged and the sync
    /// continues without group data — group-granted documents then stay visible
    /// only through group memberships synced by other sources.
    async fn maybe_sync_groups_sa_direct(
        &self,
        source: &Source,
        service_creds: &ServiceCredential,
        ctx: &SyncContext,
    ) -> HashSet<String> {
        let native_source_type = match SourceType::try_from(source.source_type.as_str()) {
            Ok(source_type) => source_type,
            Err(e) => {
                warn!(
                    "Skipping SA-direct group sync for unsupported source type: {}",
                    e
                );
                return HashSet::new();
            }
        };
        let service_auth = match self.create_auth(service_creds, native_source_type).await {
            Ok(auth) => auth,
            Err(e) => {
                warn!("Failed to create auth for SA-direct group sync: {}", e);
                return HashSet::new();
            }
        };
        if service_auth.is_oauth() {
            debug!(
                "Skipping SA-direct group sync for OAuth source {}",
                source.id
            );
            return HashSet::new();
        }
        let domain = match crate::auth::get_domain_from_credentials(service_creds) {
            Ok(d) => d,
            Err(e) => {
                warn!("Failed to get domain for SA-direct group sync: {}", e);
                return HashSet::new();
            }
        };
        let access_token = match service_auth.get_self_access_token().await {
            Ok(token) => token,
            Err(e) => {
                warn!("Failed to get SA token for group sync: {}", e);
                return HashSet::new();
            }
        };
        match self
            .sync_groups(&source.id, ctx.sync_run_id(), &domain, &access_token)
            .await
        {
            Ok(group_emails) => group_emails,
            Err(e) => {
                warn!(
                    "SA-direct group sync unavailable ({}). Group-granted documents will be \
                     visible only through group memberships synced by other sources.",
                    e
                );
                HashSet::new()
            }
        }
    }

    async fn sync_groups(
        &self,
        source_id: &str,
        sync_run_id: &str,
        domain: &str,
        access_token: &str,
    ) -> Result<HashSet<String>> {
        info!("Syncing group memberships for domain: {}", domain);

        let groups = self
            .admin_client
            .list_all_groups(access_token, domain)
            .await?;
        info!("Found {} groups in domain {}", groups.len(), domain);

        let mut group_emails: HashSet<String> = HashSet::new();
        let mut total_members = 0;
        for group in &groups {
            group_emails.insert(group.email.to_lowercase());

            let members = self
                .admin_client
                .list_all_group_members(access_token, &group.email)
                .await
                .unwrap_or_else(|e| {
                    warn!("Failed to list members for group {}: {}", group.email, e);
                    vec![]
                });

            let member_emails: Vec<String> = members
                .into_iter()
                .filter_map(|m| m.email)
                .map(|e| e.to_lowercase())
                .collect();

            total_members += member_emails.len();

            let event = ConnectorEvent::GroupMembershipSync {
                sync_run_id: sync_run_id.to_string(),
                source_id: source_id.to_string(),
                group_email: group.email.clone(),
                group_name: group.name.clone(),
                member_emails,
            };

            if let Err(e) = self
                .sdk_client
                .emit_event(sync_run_id, source_id, event)
                .await
            {
                warn!(
                    "Failed to emit group membership event for {}: {}",
                    group.email, e
                );
            }
        }

        info!(
            "Group sync complete: {} groups, {} total memberships",
            groups.len(),
            total_members
        );
        Ok(group_emails)
    }
}

/// Core ancestry-check logic — the production walker used by `is_file_in_allowed_folder`
/// and tested directly with mock lookups.
///
/// Given a file's parent IDs, an allowed set, and a function to fetch folder
/// metadata by ID, walks up the parent chain and returns true if any ancestor
/// is in the allowed set.
///
/// `lookup` returns `(name, Option<parent_id>)` for a folder ID.
/// Returns Err on error, Ok(true) if found in allowed set, Ok(false) if outside.
/// Has a safety cap of 10_000 iterations to prevent infinite loops.
pub(crate) async fn check_ancestry_static(
    file_parents: &Option<Vec<String>>,
    allowed_ids: &HashSet<String>,
    lookup: &(
         dyn Fn(String) -> Pin<Box<dyn Future<Output = Result<(String, Option<String>)>> + Send>>
             + Send
             + Sync
     ),
) -> Result<bool> {
    if allowed_ids.is_empty() {
        return Ok(true);
    }

    let mut stack: Vec<String> = Vec::new();
    if let Some(parents) = file_parents {
        for p in parents {
            stack.push(p.clone());
        }
    }

    let mut visited = HashSet::new();
    let mut iterations: usize = 0;
    const MAX_ITERATIONS: usize = 10_000;

    while let Some(cur) = stack.pop() {
        iterations += 1;
        if iterations > MAX_ITERATIONS {
            // Safety cap — treat as outside rather than infinite loop.
            return Ok(false);
        }
        if !visited.insert(cur.clone()) {
            // Cycle detected — skip, don't include.
            continue;
        }
        if allowed_ids.contains(&cur) {
            return Ok(true);
        }
        // `root` is the My Drive root alias, not a regular folder resource.
        // There is no need to fetch metadata once ancestry reaches it.
        if cur == "root" {
            continue;
        }
        let (_name, parent) = lookup(cur).await?;
        if let Some(p) = parent {
            stack.push(p);
        }
    }

    Ok(false)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;
    use tokio::sync::Semaphore;

    #[test]
    fn permits_for_bytes_rounds_up_to_64k_units() {
        assert_eq!(permits_for_bytes(0), 0);
        assert_eq!(permits_for_bytes(1), 1);
        assert_eq!(permits_for_bytes(GOOGLE_BUFFER_PERMIT_UNIT), 1);
        assert_eq!(permits_for_bytes(GOOGLE_BUFFER_PERMIT_UNIT + 1), 2);
        assert_eq!(
            permits_for_bytes(GOOGLE_MAX_BUFFERED_BYTES),
            GOOGLE_BUFFER_PERMITS as u32
        );
    }

    #[test]
    fn oversized_single_buffer_requires_more_than_full_budget() {
        assert!(permits_for_bytes(GOOGLE_MAX_BUFFERED_BYTES + 1) > GOOGLE_BUFFER_PERMITS as u32);
    }

    #[test]
    fn unknown_size_native_workspace_files_do_not_reserve_full_budget() {
        let file = crate::models::GoogleDriveFile {
            id: "doc-1".to_string(),
            name: "Doc".to_string(),
            mime_type: "application/vnd.google-apps.document".to_string(),
            web_view_link: None,
            created_time: None,
            modified_time: None,
            size: None,
            parents: None,
            shared: None,
            trashed: None,
            drive_id: None,
            inherited_permissions_disabled: None,
            permissions: None,
            owners: None,
        };

        assert_eq!(
            permits_for_unknown_size_file(&file),
            permits_for_bytes(DEFAULT_GOOGLE_DRIVE_MAX_DOWNLOAD_BYTES)
        );
        assert!(permits_for_unknown_size_file(&file) < GOOGLE_BUFFER_PERMITS as u32);
    }

    #[test]
    fn unknown_size_uploaded_files_still_reserve_full_budget() {
        let file = crate::models::GoogleDriveFile {
            id: "file-1".to_string(),
            name: "Report.xlsx".to_string(),
            mime_type: "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet"
                .to_string(),
            web_view_link: None,
            created_time: None,
            modified_time: None,
            size: None,
            parents: None,
            shared: None,
            trashed: None,
            drive_id: None,
            inherited_permissions_disabled: None,
            permissions: None,
            owners: None,
        };

        assert_eq!(
            permits_for_unknown_size_file(&file),
            GOOGLE_BUFFER_PERMITS as u32
        );
    }

    #[test]
    fn drive_content_cache_merges_permissions_for_duplicate_files() {
        let cache = DriveContentCache::default();

        let first = cache.merge_permissions(
            "drive-file-1",
            DocumentPermissions {
                public: false,
                users: vec!["alice@example.com".to_string()],
                groups: vec!["team@example.com".to_string()],
            },
        );
        assert!(!first.public);
        assert_eq!(first.users, vec!["alice@example.com"]);
        assert_eq!(first.groups, vec!["team@example.com"]);

        let merged = cache.merge_permissions(
            "drive-file-1",
            DocumentPermissions {
                public: true,
                users: vec![
                    "alice@example.com".to_string(),
                    "bob@example.com".to_string(),
                ],
                groups: vec![
                    "team@example.com".to_string(),
                    "eng@example.com".to_string(),
                ],
            },
        );

        assert!(merged.public);
        assert_eq!(merged.users, vec!["alice@example.com", "bob@example.com"]);
        assert_eq!(merged.groups, vec!["team@example.com", "eng@example.com"]);
    }

    #[test]
    fn google_chat_segment_ids_use_existing_external_id_space() {
        assert_eq!(
            segment_external_id("AAAA", "spaces/AAAA/messages/MMMM"),
            "google_chat_segment:AAAA:MMMM"
        );
        assert_eq!(chat_space_group_id("spaces/AAAA"), "google-chat-space:AAAA");
    }

    #[test]
    fn google_chat_segment_builder_splits_on_dead_time_and_truncates_message() {
        let space = GoogleChatSpace {
            name: "spaces/AAAA".to_string(),
            space_type: GoogleChatSpaceType::Space,
            display_name: Some("Engineering".to_string()),
            space_uri: None,
        };
        let mut builder = GoogleChatSegmentBuilder::new(&space);
        let first = GoogleChatMessage {
            name: "spaces/AAAA/messages/M1".to_string(),
            sender: None,
            create_time: Some("2026-06-14T10:00:00Z".to_string()),
            last_update_time: None,
            delete_time: None,
            text: Some("hello".to_string()),
            formatted_text: None,
            thread: None,
            thread_reply: None,
            attachment: vec![],
            private_message_viewer: None,
            quoted_message_metadata: None,
        };
        assert!(builder.push(first).unwrap().is_empty());

        let long_text = "x".repeat(GOOGLE_CHAT_MAX_MESSAGE_BYTES + 10);
        let second = GoogleChatMessage {
            name: "spaces/AAAA/messages/M2".to_string(),
            sender: None,
            create_time: Some("2026-06-14T11:00:01Z".to_string()),
            last_update_time: None,
            delete_time: None,
            text: Some(long_text),
            formatted_text: None,
            thread: None,
            thread_reply: None,
            attachment: vec![],
            private_message_viewer: None,
            quoted_message_metadata: None,
        };
        let ready = builder.push(second).unwrap();
        assert_eq!(ready.len(), 1);
        assert_eq!(ready[0].messages.len(), 1);
        let final_segment = builder.finish().unwrap().unwrap();
        assert_eq!(
            final_segment.truncated_message_names,
            vec!["spaces/AAAA/messages/M2"]
        );
        assert!(final_segment.messages[0].text.len() <= GOOGLE_CHAT_MAX_MESSAGE_BYTES);
    }

    #[tokio::test]
    async fn owned_buffer_permits_release_on_drop() {
        let semaphore = Arc::new(Semaphore::new(2));
        let permit = semaphore.clone().acquire_many_owned(2).await.unwrap();

        assert!(semaphore.clone().try_acquire_owned().is_err());

        drop(permit);

        assert!(semaphore.try_acquire_many_owned(2).is_ok());
    }

    // ========================================================================
    // Folder path filter logic
    // ========================================================================

    #[tokio::test]
    async fn folder_filter_matches_direct_and_nested_ancestors() {
        let allowed: HashSet<String> = ["allowed".to_string()].into_iter().collect();
        let lookup =
            |id: String| -> Pin<Box<dyn Future<Output = Result<(String, Option<String>)>> + Send>> {
                Box::pin(async move {
                    let parent = match id.as_str() {
                        "child" => Some("allowed".to_string()),
                        "outside" => Some("root".to_string()),
                        "cycle-a" => Some("cycle-b".to_string()),
                        "cycle-b" => Some("cycle-a".to_string()),
                        _ => None,
                    };
                    Ok((id, parent))
                })
            };

        assert!(
            check_ancestry_static(&Some(vec!["allowed".to_string()]), &allowed, &lookup)
                .await
                .unwrap()
        );
        assert!(
            check_ancestry_static(&Some(vec!["child".to_string()]), &allowed, &lookup)
                .await
                .unwrap()
        );
        assert!(
            !check_ancestry_static(&Some(vec!["outside".to_string()]), &allowed, &lookup)
                .await
                .unwrap()
        );
        assert!(
            !check_ancestry_static(&Some(vec!["cycle-a".to_string()]), &allowed, &lookup)
                .await
                .unwrap()
        );
    }

    #[tokio::test]
    async fn folder_filter_propagates_lookup_errors() {
        let allowed: HashSet<String> = ["allowed".to_string()].into_iter().collect();
        let lookup =
            |id: String| -> Pin<Box<dyn Future<Output = Result<(String, Option<String>)>> + Send>> {
                Box::pin(async move { Err(anyhow!("API error for {id}")) })
            };

        assert!(
            check_ancestry_static(&Some(vec!["broken".to_string()]), &allowed, &lookup)
                .await
                .is_err()
        );
    }

    #[test]
    fn drive_scope_changes_force_full_sync() {
        assert!(drive_scope_requires_full_sync(
            SyncType::Full,
            Some("all"),
            "all"
        ));
        assert!(drive_scope_requires_full_sync(
            SyncType::Incremental,
            None,
            "all"
        ));
        assert!(drive_scope_requires_full_sync(
            SyncType::Incremental,
            Some("all"),
            "drive:folder:id",
        ));
        assert!(!drive_scope_requires_full_sync(
            SyncType::Incremental,
            Some("drive:folder:id"),
            "drive:folder:id",
        ));
    }

    #[test]
    fn folder_scope_cutoff_uses_parsed_created_or_modified_times() {
        let cutoff = parse_google_time(Some("2025-01-01T00:00:00Z")).unwrap();
        let file = |mime_type: &str, created_time: &str, modified_time: &str| {
            crate::models::GoogleDriveFile {
                id: "file".to_string(),
                name: "File".to_string(),
                mime_type: mime_type.to_string(),
                web_view_link: None,
                created_time: Some(created_time.to_string()),
                modified_time: Some(modified_time.to_string()),
                size: None,
                parents: None,
                shared: None,
                trashed: None,
                drive_id: None,
                inherited_permissions_disabled: None,
                permissions: None,
                owners: None,
            }
        };

        assert!(SyncManager::pass_cutoff(
            &file(
                "application/pdf",
                "2024-01-01T00:00:00Z",
                "2025-01-02T00:00:00Z"
            ),
            cutoff,
        ));
        assert!(SyncManager::pass_cutoff(
            &file(
                "application/pdf",
                "2025-01-02T01:00:00+01:00",
                "2024-01-01T00:00:00Z"
            ),
            cutoff,
        ));
        assert!(!SyncManager::pass_cutoff(
            &file(
                "application/pdf",
                "2024-01-01T00:00:00Z",
                "2024-12-31T23:59:59Z"
            ),
            cutoff,
        ));
        assert!(SyncManager::pass_cutoff(
            &file(
                "application/vnd.google-apps.folder",
                "2020-01-01T00:00:00Z",
                "2020-01-01T00:00:00Z",
            ),
            cutoff,
        ));
    }
}

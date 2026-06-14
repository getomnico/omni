use anyhow::{Context, Result, anyhow};
use dashmap::DashMap;
use futures::{StreamExt, stream};
use omni_connector_sdk::SyncContext;
use std::collections::{HashMap, HashSet};
use std::future::Future;
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

pub(crate) fn permits_for_bytes(bytes: usize) -> u32 {
    if bytes == 0 {
        return 0;
    }

    bytes.div_ceil(GOOGLE_BUFFER_PERMIT_UNIT) as u32
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
use crate::auth::{GoogleAuth, OAuthAuth, google_max_retries};
use crate::cache::LruFolderCache;
use crate::chat::{ChatClient, GoogleChatMessage, GoogleChatSpace, GoogleChatSpaceType};
use crate::connector::build_attachment_doc_id;
use crate::drive::{DriveClient, FileContent};
use crate::gmail::{BatchThreadResult, ExtractedAttachment, GmailClient, MessageFormat};
use crate::models::{
    AttachmentPointer, GmailThread, GoogleChatSegmentCheckpoint, GoogleChatSpaceCheckpoint,
    GoogleConnectorState, GoogleSyncCheckpoint, UserFile, WebhookChannel, WebhookChannelResponse,
    WebhookNotification, mime_type_to_content_type,
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

#[derive(Debug, Clone)]
struct GoogleChatSegmentAttachmentRef {
    name: String,
    content_name: Option<String>,
    content_type: Option<String>,
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
            json!(self
                .messages
                .iter()
                .map(|m| m.name.clone())
                .collect::<Vec<_>>()),
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
            json!(self
                .space_display_name
                .as_deref()
                .unwrap_or(&self.space_name)),
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

        let known_groups = self.maybe_sync_groups(source, service_creds, ctx).await;

        // The SDK passes us the persisted state on each (re-)dispatch — we use
        // it directly instead of refetching via HTTP. On a fresh sync this is
        // None; on resume after a crash, this is the last checkpoint written
        // mid-sync.
        let existing_state = existing_state.unwrap_or_default();

        let result = match source.source_type {
            SourceType::GoogleDrive => {
                self.sync_drive_source_internal(
                    source,
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

        if result.is_ok() && source.source_type == SourceType::GoogleDrive {
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

    async fn sync_drive_for_user(
        &self,
        user_email: &str,
        service_auth: Arc<GoogleAuth>,
        source_id: &str,
        sync_run_id: &str,
        ctx: &SyncContext,
        created_after: Option<&str>,
        content_cache: Arc<DriveContentCache>,
    ) -> Result<(usize, usize)> {
        info!("Processing Drive files for user: {}", user_email);

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
                    file_batch.push(UserFile {
                        user_email: Arc::new(user_email.to_string()),
                        file,
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

        info!(
            "Completed processing user {}: {} scanned, {} updated",
            user_email, total_scanned, total_updated
        );
        Ok((total_scanned, total_updated))
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
    ) -> Result<(usize, usize)> {
        info!(
            "Processing incremental Drive sync for user {} from pageToken {}",
            user_email, start_page_token
        );

        let access_token = service_auth.get_access_token(user_email).await?;

        let mut all_changes = Vec::new();
        let mut current_token = start_page_token.to_string();

        loop {
            let response = self
                .drive_client
                .list_changes(&access_token, &current_token)
                .await?;

            all_changes.extend(response.changes);

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
            let is_removed = change.removed.unwrap_or(false);

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
                if !self.should_index_file(&file) {
                    continue;
                }

                file_batch.push(UserFile {
                    user_email: Arc::new(user_email.to_string()),
                    file,
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
        Ok((total_scanned, total_updated))
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
                let current_permissions = user_file
                    .file
                    .to_document_permissions(Some(&user_file.user_email));
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

    async fn sync_drive_source_internal(
        &self,
        source: &Source,
        service_creds: &ServiceCredential,
        sync_type: SyncType,
        existing_state: GoogleSyncCheckpoint,
        ctx: &SyncContext,
    ) -> Result<GoogleSyncCheckpoint> {
        let sync_run_id = ctx.sync_run_id();

        let service_auth = Arc::new(self.create_auth(service_creds, source.source_type).await?);

        // Calculate cutoff date for filtering
        let (drive_cutoff_date, _gmail_cutoff_date) = self.get_cutoff_date()?;
        info!("Using Drive cutoff date: {}", drive_cutoff_date);

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

        let is_incremental = matches!(sync_type, SyncType::Incremental);

        let gmail_history_ids = existing_state.gmail_history_ids.clone();
        let chat_checkpoint = existing_state.chat.clone();
        let old_page_tokens = existing_state.drive_page_tokens.unwrap_or_default();
        let can_resume_full = sync_type == SyncType::Full && ctx.is_resume();
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
        let mut errors = 0;
        let content_cache = Arc::new(DriveContentCache::default());
        let parallel_users = google_drive_parallel_users();
        info!("Processing Drive users with concurrency {}", parallel_users);

        let user_tasks = stream::iter(user_emails.iter().cloned()).map(|cur_user_email| {
            let service_auth = service_auth.clone();
            let source_id = source.id.clone();
            let sync_run_id = sync_run_id.to_string();
            let drive_cutoff_date = drive_cutoff_date.clone();
            let ctx = ctx.clone();
            let content_cache = content_cache.clone();
            let stored_page_token = old_page_tokens.get(cur_user_email.as_str()).cloned();

            async move {
                if can_resume_full && stored_page_token.is_some() {
                    info!(
                        "Skipping Drive user {} already checkpointed for sync {}",
                        cur_user_email, sync_run_id
                    );
                    return (cur_user_email, Ok((0, 0, None)));
                }

                if ctx.is_cancelled() {
                    info!(
                        "Sync {} cancelled, skipping Drive sync for user {}",
                        sync_run_id, cur_user_email
                    );
                    return (cur_user_email, Ok((0, 0, None)));
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
                let result = if use_incremental {
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
                        Some(&drive_cutoff_date),
                        content_cache.clone(),
                    )
                    .await
                };

                match result {
                    Ok((scanned, updated)) => {
                        let page_token = match self
                            .drive_client
                            .get_start_page_token_for_user(service_auth.as_ref(), &cur_user_email)
                            .await
                        {
                            Ok(token) => Some(token),
                            Err(e) => {
                                warn!(
                                    "Failed to get start page token for user {}: {}",
                                    cur_user_email, e
                                );
                                None
                            }
                        };
                        (cur_user_email, Ok((scanned, updated, page_token)))
                    }
                    Err(e) => (cur_user_email, Err(e)),
                }
            }
        });

        let mut user_results = user_tasks.buffer_unordered(parallel_users);
        while let Some((cur_user_email, result)) = user_results.next().await {
            match result {
                Ok((scanned, updated, page_token)) => {
                    total_scanned += scanned;
                    total_updated += updated;
                    info!(
                        "User {} Drive sync completed: {} scanned, {} updated",
                        cur_user_email, scanned, updated
                    );

                    if let Some(token) = page_token {
                        new_page_tokens.insert(cur_user_email.clone(), token);
                    }

                    let checkpoint_state = GoogleSyncCheckpoint {
                        gmail_history_ids: gmail_history_ids.clone(),
                        drive_page_tokens: if new_page_tokens.is_empty() {
                            None
                        } else {
                            Some(new_page_tokens.clone())
                        },
                        chat: chat_checkpoint.clone(),
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
                    error!("Failed to process Drive for user {}: {}", cur_user_email, e);
                    errors += 1;
                }
            }
        }

        info!(
            "User processing complete. Total: {} scanned, {} updated, {} errors",
            total_scanned, total_updated, errors
        );

        info!(
            "Sync completed for source {}: {} scanned, {} updated",
            source.id, total_scanned, total_updated
        );

        // Clear folder cache to free memory after sync
        self.folder_cache.clear();

        info!("Completed sync for source: {}", source.id);

        Ok(GoogleSyncCheckpoint {
            gmail_history_ids,
            drive_page_tokens: if new_page_tokens.is_empty() {
                None
            } else {
                Some(new_page_tokens)
            },
            chat: chat_checkpoint,
        })
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

        let service_auth = Arc::new(self.create_auth(service_creds, source.source_type).await?);

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
                            total_processed += processed;
                            total_updated += updated;
                            info!(
                                "User {} Gmail sync completed: {} processed, {} updated",
                                cur_user_email, processed, updated
                            );
                            true
                        }
                        Err(e) => {
                            error!("Failed to process Gmail for user {}: {}", cur_user_email, e);
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
                            chat: chat_checkpoint.clone(),
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
                    warn!(
                        "Failed to get access token for user {}: {}. This user may not have Gmail access.",
                        cur_user_email, e
                    );
                }
            }
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
            chat: chat_checkpoint,
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
                        &reader_email,
                        &space,
                        &mut space_checkpoint,
                    )
                    .await
                {
                    Ok(()) => {}
                    Err(e) => {
                        warn!("Incremental Google Chat sync failed for {}: {}. Falling back to full-space sync.", space.name, e);
                        self.sync_chat_space_full(
                            source,
                            ctx,
                            &service_auth,
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
                chat: Some(chat_checkpoint.clone()),
            };
            ctx.save_checkpoint(serde_json::to_value(&checkpoint_state)?)
                .await?;
        }

        Ok(GoogleSyncCheckpoint {
            gmail_history_ids: existing_state.gmail_history_ids,
            drive_page_tokens: existing_state.drive_page_tokens,
            chat: Some(chat_checkpoint),
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
        for user_email in user_emails {
            if ctx.is_cancelled() {
                break;
            }
            let mut page_token: Option<String> = None;
            loop {
                let response = match self
                    .chat_client
                    .list_spaces_for_user(service_auth, user_email, page_token.as_deref())
                    .await
                {
                    Ok(response) => response,
                    Err(e) => {
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
        }
        info!("Discovered {} Google Chat named spaces", spaces.len());
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
                    Some("ASC"),
                    true,
                )
                .await?;
            for message in response.messages {
                let ready = builder.push(message)?;
                for segment in ready {
                    let checkpoint_entry = self.emit_chat_segment(source, ctx, &segment).await?;
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
            let checkpoint_entry = self.emit_chat_segment(source, ctx, &segment).await?;
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
        let mut touched = false;
        let mut page_token = checkpoint.incremental_event_page_token.clone();
        let mut latest_event_time = last_event.clone();
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
            if !response.space_events.is_empty() {
                touched = true;
            }
            for event in &response.space_events {
                latest_event_time = event.event_time.clone();
            }
            page_token = response.next_page_token;
            checkpoint.incremental_event_page_token = page_token.clone();
            checkpoint.pending_event_watermark = Some(latest_event_time.clone());
            if page_token.is_none() {
                break;
            }
        }
        if touched {
            self.sync_chat_space_full(source, ctx, service_auth, reader_email, space, checkpoint)
                .await?;
        }
        checkpoint.last_event_time = Some(latest_event_time);
        checkpoint.pending_event_watermark = None;
        checkpoint.incremental_event_page_token = None;
        checkpoint.incremental_in_progress = false;
        Ok(())
    }

    async fn emit_chat_segment(
        &self,
        source: &Source,
        ctx: &SyncContext,
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
        let content = format!(
            "Attachment: {}\nAttached to Google Chat message: {}\nParent segment: {}\nSpace: {}\nSender: {}\nMessage excerpt: {}\n\nAttachment content extraction for Google Chat is not yet available in this connector build.\n",
            title,
            message.name,
            segment.external_id,
            segment.space_display_name.as_deref().unwrap_or(&segment.space_name),
            message.sender,
            message.text.chars().take(500).collect::<String>()
        );
        let content_id = ctx.store_content(&content).await?;
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
        let metadata = DocumentMetadata {
            title: Some(title),
            author: Some(message.sender.clone()),
            created_at: Some(message.create_time),
            updated_at: message.update_time.or(Some(message.create_time)),
            content_type: Some("attachment".to_string()),
            mime_type: attachment.content_type.clone(),
            size: None,
            url: None,
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
            content_id,
            metadata,
            permissions,
            attributes: None,
        })
        .await?;
        Ok(())
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
                let access_token = creds
                    .credentials
                    .get("access_token")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string();

                let refresh_token = creds
                    .credentials
                    .get("refresh_token")
                    .and_then(|v| v.as_str())
                    .ok_or_else(|| anyhow::anyhow!("Missing refresh_token in OAuth credentials"))?
                    .to_string();

                let expires_at = creds
                    .credentials
                    .get("expires_at")
                    .and_then(|v| v.as_i64())
                    .unwrap_or(0);

                let user_email = creds
                    .credentials
                    .get("user_email")
                    .and_then(|v| v.as_str())
                    .or(creds.principal_email.as_deref())
                    .ok_or_else(|| anyhow::anyhow!("Missing user_email in OAuth credentials"))?
                    .to_string();

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
        let service_auth =
            crate::auth::create_service_auth(&service_creds, SourceType::GoogleDrive)?;
        let user_email = self.get_user_email_from_source(source_id).await
            .map_err(|e| anyhow::anyhow!("Failed to get user email for source {}: {}. Make sure the source has a valid creator.", source_id, e))?;
        let access_token = service_auth.get_access_token(&user_email).await?;

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

    pub async fn stop_webhook_for_source(
        &self,
        source_id: &str,
        channel_id: &str,
        resource_id: &str,
    ) -> Result<()> {
        let service_creds = self.get_service_credentials(source_id).await?;
        let service_auth =
            crate::auth::create_service_auth(&service_creds, SourceType::GoogleDrive)?;
        let user_email = self.get_user_email_from_source(source_id).await
            .map_err(|e| anyhow::anyhow!("Failed to get user email for source {}: {}. Make sure the source has a valid creator.", source_id, e))?;
        let access_token = service_auth.get_access_token(&user_email).await?;

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

        // Build path by traversing up the folder hierarchy
        let mut depth = 0;
        loop {
            depth += 1;
            debug!(
                "Path building depth: {}, current folder: {}",
                depth, current_folder_id
            );

            // TODO: Remove this
            if depth > 50 {
                warn!(
                    "Path building depth exceeded 50 levels for file: {}, folder: {}",
                    file_name, folder_id
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
                        .get_folder_metadata(&auth, &user_email, &folder_id)
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
        let service_auth = match self.create_auth(service_creds, source.source_type).await {
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
}

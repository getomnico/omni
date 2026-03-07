use anyhow::{Context, Result};
use mailparse::{MailAddr, MailHeaderMap, ParsedMail};
use serde::{Deserialize, Serialize};
use serde_json::json;
use shared::models::{ConnectorEvent, DocumentMetadata, DocumentPermissions};
use std::collections::{HashMap, HashSet};
use time::OffsetDateTime;
use urlencoding::encode;

use crate::attachment::extract_attachments;

/// Persistent sync checkpoint for a single (source, folder) pair, stored in
/// `Source.connector_state` as `{ "folders": { "<folder>": FolderSyncState } }`.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct ImapConnectorState {
    /// Per-folder sync state keyed by folder name.
    #[serde(default)]
    pub folders: HashMap<String, FolderSyncState>,
}

/// Sync checkpoint for a single mailbox folder.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FolderSyncState {
    /// UIDVALIDITY value from the server.  If this changes the folder must be
    /// fully resynced because all UIDs have been reassigned.
    pub uid_validity: u32,
    /// UIDs of messages we have successfully indexed.
    /// Used both for new-message detection (`server_uids − indexed_uids`) and
    /// for deletion detection (`indexed_uids − server_uids`).
    #[serde(default)]
    pub indexed_uids: Vec<u32>,
    /// Parsed message snapshots keyed by IMAP UID, used to rebuild thread docs.
    #[serde(default)]
    pub messages: HashMap<u32, ParsedEmail>,
    /// UIDs that were intentionally skipped due to the `max_message_size` limit.
    /// Excluded from every subsequent `new_uids` computation so their raw bytes
    /// are not re-downloaded on every sync.  Cleared when UIDVALIDITY changes or
    /// when a full resync is requested.
    #[serde(default)]
    pub skipped_uids: HashSet<u32>,
}

impl ImapConnectorState {
    pub fn from_connector_state(state: &Option<serde_json::Value>) -> Self {
        state
            .as_ref()
            .and_then(|v| serde_json::from_value(v.clone()).ok())
            .unwrap_or_default()
    }

    pub fn to_json(&self) -> serde_json::Value {
        serde_json::to_value(self).unwrap_or_else(|_| json!({}))
    }
}

/// A parsed email message ready for indexing.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ParsedEmail {
    pub message_id: Option<String>,
    pub in_reply_to: Option<String>,
    #[serde(default)]
    pub references: Vec<String>,
    pub imap_uid: u32,
    pub folder: String,
    pub subject: String,
    pub from: String,
    pub to: Vec<String>,
    pub cc: Vec<String>,
    pub date: Option<OffsetDateTime>,
    pub body_text: String,
    #[serde(default)]
    pub flags: Vec<String>,
    pub size: usize,
}

impl ParsedEmail {
    /// Build a stable external document ID for this message.
    pub fn external_id(&self, source_id: &str) -> String {
        make_document_id(source_id, &self.folder, self.imap_uid)
    }

    /// Generate plain-text content suitable for indexing.
    pub fn generate_content(&self) -> String {
        let mut out = String::new();
        out.push_str(&format!("Subject: {}\n", self.subject));
        out.push_str(&format!("From: {}\n", self.from));
        if !self.to.is_empty() {
            out.push_str(&format!("To: {}\n", self.to.join(", ")));
        }
        if !self.cc.is_empty() {
            out.push_str(&format!("Cc: {}\n", self.cc.join(", ")));
        }
        if let Some(date) = &self.date {
            out.push_str(&format!("Date: {}\n", date));
        }
        out.push('\n');
        out.push_str(&self.body_text);
        out
    }

    pub fn thread_id(&self) -> String {
        self.references
            .first()
            .cloned()
            .or_else(|| self.in_reply_to.clone())
            .or_else(|| self.message_id.clone())
            .unwrap_or_else(|| format!("{}:{}", self.folder, self.imap_uid))
    }

    pub fn participant_emails(&self) -> Vec<String> {
        let mut seen = HashSet::new();
        let mut participants = Vec::new();

        for value in std::iter::once(self.from.as_str())
            .chain(self.to.iter().map(String::as_str))
            .chain(self.cc.iter().map(String::as_str))
        {
            if let Some(email) = extract_email_addr(value) {
                if seen.insert(email.clone()) {
                    participants.push(email);
                }
            }
        }

        participants
    }

    pub fn to_connector_event(
        &self,
        sync_run_id: String,
        source_id: String,
        content_id: String,
        account_display_name: &str,
        webmail_url_template: Option<&str>,
        user_email: Option<&str>,
    ) -> ConnectorEvent {
        build_thread_connector_event(
            &[self.clone()],
            sync_run_id,
            source_id,
            content_id,
            account_display_name,
            webmail_url_template,
            user_email,
            false,
        )
    }
}

/// Sentinel string used when an email has no Subject header.
const NO_SUBJECT_PLACEHOLDER: &str = "(no subject)";

fn urlenc(s: &str) -> String {
    s.replace('/', "%2F").replace(' ', "%20")
}

/// Build the canonical document ID for an IMAP message.
pub fn make_document_id(source_id: &str, folder: &str, uid: u32) -> String {
    format!("imap:{}:{}:{}", source_id, urlenc(folder), uid)
}

pub fn make_thread_document_id(source_id: &str, folder: &str, thread_id: &str) -> String {
    format!(
        "imap-thread:{}:{}:{}",
        source_id,
        urlenc(folder),
        encode(thread_id)
    )
}

pub fn generate_thread_content(messages: &[ParsedEmail]) -> String {
    let sorted_messages = sort_thread_messages(messages);
    let mut content_parts = Vec::new();

    if let Some(subject) = sorted_messages
        .iter()
        .find(|message| !message.subject.is_empty() && message.subject != NO_SUBJECT_PLACEHOLDER)
        .map(|message| message.subject.clone())
    {
        content_parts.push(format!("Subject: {}", subject));
        content_parts.push(String::new());
    }

    for (index, message) in sorted_messages.iter().enumerate() {
        content_parts.push(format!("=== Message {} ===", index + 1));
        content_parts.push(format!("From: {}", message.from));
        if !message.to.is_empty() {
            content_parts.push(format!("To: {}", message.to.join(", ")));
        }
        if !message.cc.is_empty() {
            content_parts.push(format!("Cc: {}", message.cc.join(", ")));
        }
        if let Some(date) = message.date {
            content_parts.push(format!("Date: {}", date));
        }
        if !message.flags.is_empty() {
            content_parts.push(format!("Flags: {}", message.flags.join(", ")));
        }
        content_parts.push(String::new());
        if !message.body_text.trim().is_empty() {
            content_parts.push(message.body_text.trim().to_string());
        }
        content_parts.push(String::new());
    }

    content_parts.join("\n")
}

pub fn build_thread_connector_event(
    messages: &[ParsedEmail],
    sync_run_id: String,
    source_id: String,
    content_id: String,
    account_display_name: &str,
    webmail_url_template: Option<&str>,
    user_email: Option<&str>,
    is_update: bool,
) -> ConnectorEvent {
    let sorted_messages = sort_thread_messages(messages);
    let first = sorted_messages
        .first()
        .expect("thread event requires at least one message");
    let last = sorted_messages
        .last()
        .expect("thread event requires at least one message");
    // derive_thread_root scans all messages for the canonical root rather than
    // relying on first.thread_id(), which is order-dependent and can return an
    // intermediate message-ID when a dateless reply-without-References sorts first.
    let thread_id = derive_thread_root(&sorted_messages);
    let document_id = make_thread_document_id(&source_id, &first.folder, &thread_id);

    let title = sorted_messages
        .iter()
        .find(|message| !message.subject.is_empty() && message.subject != NO_SUBJECT_PLACEHOLDER)
        .map(|message| message.subject.clone())
        .unwrap_or_else(|| NO_SUBJECT_PLACEHOLDER.to_string());

    let created_at = sorted_messages
        .iter()
        .filter_map(|message| message.date)
        .min();
    let updated_at = sorted_messages
        .iter()
        .filter_map(|message| message.date)
        .max();

    let mut participants = Vec::new();
    let mut seen_participants = HashSet::new();
    let mut to_values = Vec::new();
    let mut seen_to = HashSet::new();
    let mut cc_values = Vec::new();
    let mut seen_cc = HashSet::new();
    let mut flags = Vec::new();
    let mut seen_flags = HashSet::new();
    let mut message_ids = Vec::new();
    let mut seen_message_ids = HashSet::new();

    for message in &sorted_messages {
        for participant in message.participant_emails() {
            if seen_participants.insert(participant.clone()) {
                participants.push(participant);
            }
        }

        for to in &message.to {
            if seen_to.insert(to.clone()) {
                to_values.push(to.clone());
            }
        }

        for cc in &message.cc {
            if seen_cc.insert(cc.clone()) {
                cc_values.push(cc.clone());
            }
        }

        for flag in &message.flags {
            if seen_flags.insert(flag.clone()) {
                flags.push(flag.clone());
            }
        }

        if let Some(message_id) = &message.message_id {
            if seen_message_ids.insert(message_id.clone()) {
                message_ids.push(message_id.clone());
            }
        }
    }

    let mut extra: HashMap<String, serde_json::Value> = HashMap::new();
    extra.insert("folder".to_string(), json!(first.folder));
    extra.insert("from".to_string(), json!(first.from));
    extra.insert("to".to_string(), json!(to_values));
    extra.insert("cc".to_string(), json!(cc_values));
    extra.insert("thread_id".to_string(), json!(thread_id));
    extra.insert("participants".to_string(), json!(participants));
    extra.insert("message_count".to_string(), json!(sorted_messages.len()));
    extra.insert(
        "imap_uids".to_string(),
        json!(sorted_messages
            .iter()
            .map(|message| message.imap_uid)
            .collect::<Vec<_>>()),
    );
    extra.insert("message_ids".to_string(), json!(message_ids));
    extra.insert("account".to_string(), json!(account_display_name));

    let url = webmail_url_template.map(|template| {
        template
            .replace("{folder}", &encode(&first.folder))
            .replace("{uid}", &last.imap_uid.to_string())
            .replace(
                "{message_id}",
                &last
                    .message_id
                    .as_deref()
                    .map(encode)
                    .map(|value| value.into_owned())
                    .unwrap_or_default(),
            )
    });

    let metadata = DocumentMetadata {
        title: Some(title.clone()),
        author: Some(first.from.clone()),
        created_at,
        updated_at,
        content_type: Some("email_thread".to_string()),
        mime_type: Some("application/x-imap-thread".to_string()),
        size: Some(
            sorted_messages
                .iter()
                .map(|message| message.size)
                .sum::<usize>()
                .to_string(),
        ),
        url,
        path: Some(format!("{}/{}", account_display_name, first.folder)),
        extra: Some(extra),
    };

    let attributes: HashMap<String, serde_json::Value> = {
        let mut a = HashMap::new();
        a.insert("source_name".to_string(), json!(account_display_name));
        a.insert("folder".to_string(), json!(first.folder));
        a.insert("from".to_string(), json!(first.from));
        a.insert("to".to_string(), json!(to_values));
        a.insert("cc".to_string(), json!(cc_values));
        a.insert("subject".to_string(), json!(title));
        a.insert("flags".to_string(), json!(flags));
        a.insert("thread_id".to_string(), json!(thread_id));
        a.insert("message_count".to_string(), json!(sorted_messages.len()));
        if let Some(date) = created_at {
            a.insert("created_at".to_string(), json!(date.unix_timestamp()));
        }
        if let Some(date) = updated_at {
            a.insert("updated_at".to_string(), json!(date.unix_timestamp()));
        }
        a
    };

    // IMAP permissions: only the account owner (source creator) should have access.
    // Email correspondents are NOT granted access - they are just metadata participants.
    let permissions = DocumentPermissions {
        public: false,
        users: user_email
            .map(|e| vec![e.to_lowercase()])
            .unwrap_or_default(),
        groups: vec![],
    };

    if is_update {
        ConnectorEvent::DocumentUpdated {
            sync_run_id,
            source_id,
            document_id,
            content_id,
            metadata,
            permissions: Some(permissions),
            attributes: Some(attributes),
        }
    } else {
        ConnectorEvent::DocumentCreated {
            sync_run_id,
            source_id,
            document_id,
            content_id,
            metadata,
            permissions,
            attributes: Some(attributes),
        }
    }
}

/// Parse a raw RFC 2822 email message into a `ParsedEmail`.
pub fn parse_raw_email(raw: &[u8], uid: u32, folder: &str) -> Result<ParsedEmail> {
    let parsed = mailparse::parse_mail(raw).context("Failed to parse email")?;

    let headers = parsed.get_headers();

    let message_id = headers
        .get_first_value("Message-ID")
        .or_else(|| headers.get_first_value("Message-Id"));

    let subject = headers
        .get_first_value("Subject")
        .unwrap_or_else(|| NO_SUBJECT_PLACEHOLDER.to_string());

    let from = headers
        .get_first_value("From")
        .unwrap_or_else(|| "unknown".to_string());

    let to = parse_address_list(headers.get_first_value("To").as_deref().unwrap_or(""));
    let cc = parse_address_list(headers.get_first_value("Cc").as_deref().unwrap_or(""));

    let in_reply_to = headers
        .get_first_value("In-Reply-To")
        .or_else(|| headers.get_first_value("In-reply-to"));

    let references = headers
        .get_first_value("References")
        .map(|value| parse_message_id_list(&value))
        .unwrap_or_default();

    let date = headers
        .get_first_value("Date")
        .as_deref()
        .and_then(parse_email_date);

    let body_text = extract_body_text(&parsed);
    let size = raw.len();

    Ok(ParsedEmail {
        message_id,
        in_reply_to,
        references,
        imap_uid: uid,
        folder: folder.to_string(),
        subject,
        from,
        to,
        cc,
        date,
        body_text,
        flags: vec![],
        size,
    })
}

/// Extract the best available plain-text body from a parsed email,
/// including text extracted from supported attachments (PDF, DOCX, XLSX, PPTX).
fn extract_body_text(mail: &ParsedMail) -> String {
    // Collect inline text parts recursively.
    let mut plain_parts: Vec<String> = Vec::new();
    let mut html_parts: Vec<String> = Vec::new();
    collect_text_parts(mail, &mut plain_parts, &mut html_parts);

    let mut body = if !plain_parts.is_empty() {
        plain_parts.join("\n\n")
    } else if !html_parts.is_empty() {
        let combined = html_parts.join("\n\n");
        html_to_text(&combined)
    } else {
        String::new()
    };

    // Extract text from supported attachments and append.
    let attachments = extract_attachments(mail);
    for att in &attachments {
        body.push_str("\n\n");
        body.push_str(&format!("[Attachment: {}]\n", att.filename));
        body.push_str(&att.text);
    }

    body
}

fn collect_text_parts(mail: &ParsedMail, plain: &mut Vec<String>, html: &mut Vec<String>) {
    // Skip parts that are file attachments — those are handled by
    // extract_attachments() and must not be double-counted as inline body.
    if is_file_attachment(mail) {
        return;
    }

    let ct = mail.ctype.mimetype.to_ascii_lowercase();
    if ct == "text/plain" {
        if let Ok(body) = mail.get_body() {
            if !body.trim().is_empty() {
                plain.push(body);
            }
        }
    } else if ct == "text/html" {
        if let Ok(body) = mail.get_body() {
            if !body.trim().is_empty() {
                html.push(body);
            }
        }
    }
    for sub in &mail.subparts {
        collect_text_parts(sub, plain, html);
    }
}

/// Returns `true` when the MIME part carries a non-empty filename (via
/// Content-Disposition or Content-Type `name`) or has
/// `Content-Disposition: attachment`, indicating it is a file attachment rather
/// than an inline message body part.
///
/// The non-empty check mirrors [`attachment::attachment_filename`] so that the
/// two gates stay in sync: a part skipped here will always be picked up by
/// `extract_attachments`, and vice-versa.
fn is_file_attachment(mail: &ParsedMail) -> bool {
    let disposition = mail.get_content_disposition();
    if disposition.disposition == mailparse::DispositionType::Attachment {
        return true;
    }
    if disposition
        .params
        .get("filename")
        .is_some_and(|v| !v.is_empty())
    {
        return true;
    }
    mail.ctype.params.get("name").is_some_and(|v| !v.is_empty())
}

/// Column width used when rendering HTML emails to plain text.
const HTML_TEXT_WIDTH: usize = 100;

fn html_to_text(html: &str) -> String {
    html2text::from_read(html.as_bytes(), HTML_TEXT_WIDTH)
}

fn parse_address_list(value: &str) -> Vec<String> {
    if value.is_empty() {
        return vec![];
    }
    // Use the RFC 2822-aware parser from mailparse so that quoted display
    // names containing commas (e.g. `"Smith, John" <j@example.com>`) are
    // handled correctly instead of being split at the comma.
    match mailparse::addrparse(value) {
        Ok(addrs) => addrs
            .iter()
            .flat_map(|addr| match addr {
                MailAddr::Single(info) => vec![info.to_string()],
                MailAddr::Group(group) => group.addrs.iter().map(|info| info.to_string()).collect(),
            })
            .collect(),
        Err(_) => {
            // Malformed header: fall back to naive comma-split.
            value
                .split(',')
                .map(|s| s.trim().to_string())
                .filter(|s| !s.is_empty())
                .collect()
        }
    }
}

fn parse_email_date(value: &str) -> Option<OffsetDateTime> {
    mailparse::dateparse(value)
        .ok()
        .and_then(|ts| OffsetDateTime::from_unix_timestamp(ts).ok())
}

fn extract_email_addr(value: &str) -> Option<String> {
    if let Some(start) = value.find('<') {
        if let Some(end) = value[start + 1..].find('>') {
            let email = value[start + 1..start + 1 + end].trim().to_lowercase();
            if email.contains('@') {
                return Some(email);
            }
        }
    }

    let value = value.trim().to_lowercase();
    if value.contains('@') {
        Some(value)
    } else {
        None
    }
}

fn parse_message_id_list(value: &str) -> Vec<String> {
    value
        .split_whitespace()
        .filter(|part| part.starts_with('<') && part.ends_with('>'))
        .map(str::to_string)
        .collect()
}

fn sort_thread_messages(messages: &[ParsedEmail]) -> Vec<ParsedEmail> {
    let mut sorted = messages.to_vec();
    sorted.sort_by_key(|message| (message.date, message.imap_uid));
    sorted
}

/// Derive the canonical thread-root message-ID from a slice of messages.
///
/// This function is used to compute a stable `document_id` for a thread event.
/// It must return the same value regardless of the sort order of `messages`.
///
/// Strategy (in priority order):
/// 1. If any message carries a `References` header, its first element is the
///    RFC 5256-defined thread root.
/// 2. Otherwise, walk `In-Reply-To` chains within the slice to find the
///    message that is not a reply to any other message in the slice, taking
///    the one with the smallest `imap_uid` for determinism.
/// 3. Final fallback: smallest-uid message's `thread_id()`.
fn derive_thread_root(messages: &[ParsedEmail]) -> String {
    // Fast path: any References[0] is the canonical root (RFC 5256 §2.2).
    if let Some(root) = messages.iter().find_map(|m| m.references.first()) {
        return root.clone();
    }

    // Slow path: no References headers present.  Walk In-Reply-To chains to
    // find the ultimate ancestor message within the slice.
    let by_mid: HashMap<&str, u32> = messages
        .iter()
        .filter_map(|m| m.message_id.as_deref().map(|id| (id, m.imap_uid)))
        .collect();

    // A "root" in this slice is a message whose In-Reply-To either is absent
    // or points to a message not present in the slice.  Pick the one with the
    // smallest imap_uid for a stable, sync-order-independent result.
    let root = messages
        .iter()
        .filter(|m| {
            m.in_reply_to
                .as_deref()
                .map_or(true, |irt| !by_mid.contains_key(irt))
        })
        .min_by_key(|m| m.imap_uid);

    // If every message points to another in the slice (pathological cycle),
    // fall back to the smallest uid.
    root.or_else(|| messages.iter().min_by_key(|m| m.imap_uid))
        .map(|m| m.thread_id())
        .unwrap_or_default()
}

/// Resolve the canonical thread-root message-ID for an already-indexed message.
///
/// If the message carries a `References` header its first element is the root
/// (RFC 5256 §2.2).  Otherwise the function walks the `In-Reply-To` chain
/// through the stored snapshots until it reaches a message with no parent in
/// the store.  A cycle guard prevents infinite loops in malformed headers.
pub fn resolve_thread_root(
    start_uid: u32,
    messages: &HashMap<u32, ParsedEmail>,
    by_message_id: &HashMap<String, u32>,
) -> String {
    let Some(start) = messages.get(&start_uid) else {
        return format!("uid:{}", start_uid);
    };
    // References[0] is always the thread root per RFC 5256 §2.2.
    if let Some(root) = start.references.first() {
        return root.clone();
    }
    // Walk the In-Reply-To chain through stored snapshots.
    let mut current_uid = start_uid;
    let mut visited: HashSet<u32> = HashSet::new();
    loop {
        if !visited.insert(current_uid) {
            break; // cycle guard
        }
        let current = match messages.get(&current_uid) {
            Some(m) => m,
            None => break,
        };
        match current
            .in_reply_to
            .as_deref()
            .and_then(|id| by_message_id.get(id))
        {
            Some(&parent_uid) => current_uid = parent_uid,
            None => break,
        }
    }
    messages
        .get(&current_uid)
        .map(|m| m.thread_id())
        .unwrap_or_else(|| format!("uid:{}", current_uid))
}

/// Same as `resolve_thread_root` but for a NEW email that is not yet in the
/// messages map.  Uses the email's own headers to seed the lookup, then
/// delegates to `resolve_thread_root` via the parent's UID if the parent is
/// already stored.
pub fn resolve_new_email_thread_root(
    email: &ParsedEmail,
    messages: &HashMap<u32, ParsedEmail>,
    by_message_id: &HashMap<String, u32>,
) -> String {
    // References[0] is the thread root (RFC 5256 §2.2).
    if let Some(root) = email.references.first() {
        return root.clone();
    }
    // Walk through the stored parent, if any.
    if let Some(&parent_uid) = email
        .in_reply_to
        .as_deref()
        .and_then(|id| by_message_id.get(id))
    {
        return resolve_thread_root(parent_uid, messages, by_message_id);
    }
    // Root message or orphan: use the email's own thread_id.
    email.thread_id()
}

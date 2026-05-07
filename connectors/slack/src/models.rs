use chrono::{DateTime, NaiveDate, Utc};
use serde::{Deserialize, Serialize};
use serde_json::json;
use shared::models::{ConnectorEvent, DocumentAttributes, DocumentMetadata, DocumentPermissions};
use std::collections::HashMap;
use time::OffsetDateTime;

// ============================================================================
// Connector State
// ============================================================================

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct SlackConnectorState {
    #[serde(default)]
    pub channel_timestamps: HashMap<String, String>,
}

// ============================================================================
// Credentials
// ============================================================================

/// Decoded shape of `service_credentials.credentials` for Slack sources.
/// `bot_token` is required for any sync; `app_token` is required for the
/// realtime (Socket Mode) path and may be absent on sources configured for
/// scheduled-only sync.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SlackCredentials {
    pub bot_token: String,
    #[serde(default)]
    pub app_token: Option<String>,
}

// ============================================================================
// Slack Models
// ============================================================================

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SlackUserProfile {
    pub email: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SlackUser {
    pub id: String,
    pub name: String,
    pub real_name: Option<String>,
    pub is_bot: bool,
    #[serde(default)]
    pub profile: Option<SlackUserProfile>,
}

impl SlackUser {
    pub fn email(&self) -> Option<&str> {
        self.profile.as_ref()?.email.as_deref()
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SlackChannel {
    pub id: String,
    /// Public/private channels and group DMs (mpim) have a name. IMs (1:1 DMs)
    /// don't — for those we synthesize one at sync time using the partner's
    /// user_id (see `SlackChannel::display_name`).
    #[serde(default)]
    pub name: String,
    #[serde(rename = "is_channel", default)]
    pub is_public: bool,
    #[serde(default)]
    pub is_private: bool,
    /// 1:1 direct message. Has no `name`; the partner's user_id is in `user`.
    #[serde(default)]
    pub is_im: bool,
    /// Group DM (multi-person direct message). Has a synthetic name set by
    /// Slack like `mpdm-alice--bob--charlie-1`.
    #[serde(default)]
    pub is_mpim: bool,
    /// True when the bot is a member of the channel. IMs/MPIMs the bot is part
    /// of don't expose this field; default-false is fine because we treat IMs
    /// and MPIMs as implicitly joined.
    #[serde(default)]
    pub is_member: bool,
    pub num_members: Option<i32>,
    /// For IMs, the user_id of the other party.
    #[serde(default)]
    pub user: Option<String>,
}

impl SlackChannel {
    /// A name suitable for display/indexing. For IMs (which Slack doesn't name)
    /// we synthesize one from the partner's user_id. The id is included so the
    /// name is unique even if user_id resolution fails.
    pub fn display_name(&self) -> String {
        if !self.name.is_empty() {
            return self.name.clone();
        }
        if self.is_im {
            return self
                .user
                .as_deref()
                .map(|u| format!("dm-{}", u))
                .unwrap_or_else(|| format!("dm-{}", self.id));
        }
        // Fall back to the channel id so docs always have a non-empty name.
        self.id.clone()
    }

    /// Should we try to auto-join this conversation? Only relevant for public
    /// channels — the bot is implicitly a member of IMs/MPIMs it's a part of,
    /// and private channels require an invite.
    pub fn requires_join(&self) -> bool {
        !self.is_member && !self.is_private && !self.is_im && !self.is_mpim
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SlackMessage {
    #[serde(rename = "type")]
    pub msg_type: String,
    pub text: String,
    pub user: String,
    pub ts: String,
    pub thread_ts: Option<String>,
    pub reply_count: Option<i32>,
    pub attachments: Option<Vec<SlackAttachment>>,
    pub files: Option<Vec<SlackFile>>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SlackAttachment {
    pub title: Option<String>,
    pub text: Option<String>,
    pub fallback: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SlackFile {
    pub id: String,
    pub name: String,
    pub title: Option<String>,
    pub mimetype: Option<String>,
    pub size: i64,
    pub url_private: Option<String>,
    pub url_private_download: Option<String>,
    pub permalink: Option<String>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct ConversationsListResponse {
    pub ok: bool,
    pub channels: Vec<SlackChannel>,
    pub response_metadata: Option<ResponseMetadata>,
    pub error: Option<String>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct ConversationInfoResponse {
    pub ok: bool,
    pub channel: SlackChannel,
    pub error: Option<String>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct ConversationsHistoryResponse {
    pub ok: bool,
    pub messages: Vec<SlackMessage>,
    pub has_more: bool,
    pub response_metadata: Option<ResponseMetadata>,
    pub error: Option<String>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct UsersListResponse {
    pub ok: bool,
    pub members: Vec<SlackUser>,
    pub response_metadata: Option<ResponseMetadata>,
    pub error: Option<String>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct ConversationsMembersResponse {
    pub ok: bool,
    pub members: Vec<String>,
    pub response_metadata: Option<ResponseMetadata>,
    pub error: Option<String>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct AuthTestResponse {
    pub ok: bool,
    pub url: String,
    pub team: String,
    pub user: String,
    pub team_id: String,
    pub user_id: String,
    pub bot_id: Option<String>,
    pub is_enterprise_install: bool,
    pub error: Option<String>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct ResponseMetadata {
    pub next_cursor: Option<String>,
}

#[derive(Debug, Clone)]
pub struct MessageGroup {
    pub channel_id: String,
    pub channel_name: String,
    pub date: NaiveDate,
    pub messages: Vec<(SlackMessage, String)>, // (message, author_name)
    pub is_thread: bool,
    pub thread_ts: Option<String>,
    pub part: Option<usize>,
}

/// Structured attributes for Slack messages, used for filtering and faceting.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SlackMessageAttributes {
    pub channel_name: String,
    pub is_thread: bool,
}

impl SlackMessageAttributes {
    pub fn into_attributes(self) -> DocumentAttributes {
        let mut attrs = HashMap::new();
        attrs.insert("channel_name".into(), json!(self.channel_name));
        attrs.insert("is_thread".into(), json!(self.is_thread));
        attrs
    }
}

/// Structured attributes for Slack files, used for filtering and faceting.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SlackFileAttributes {
    pub channel_name: String,
}

impl SlackFileAttributes {
    pub fn into_attributes(self) -> DocumentAttributes {
        let mut attrs = HashMap::new();
        attrs.insert("channel_name".into(), json!(self.channel_name));
        attrs
    }
}

impl MessageGroup {
    pub fn new(
        channel_id: String,
        channel_name: String,
        date: NaiveDate,
        is_thread: bool,
        thread_ts: Option<String>,
    ) -> Self {
        Self {
            channel_id,
            channel_name,
            date,
            messages: Vec::new(),
            is_thread,
            thread_ts,
            part: None,
        }
    }

    pub fn add_message(&mut self, message: SlackMessage, author_name: String) {
        self.messages.push((message, author_name));
    }

    pub fn message_count(&self) -> usize {
        self.messages.len()
    }

    pub fn content_size(&self) -> usize {
        self.messages
            .iter()
            .map(|(msg, author)| msg.text.len() + author.len() + 20) // rough estimate
            .sum()
    }

    pub fn should_split(&self) -> bool {
        self.message_count() >= 100 || self.content_size() >= 50_000
    }

    pub fn to_document_content(&self) -> String {
        let mut content = String::new();

        for (message, author) in &self.messages {
            let timestamp = DateTime::from_timestamp(
                message
                    .ts
                    .split('.')
                    .next()
                    .unwrap_or("0")
                    .parse::<i64>()
                    .unwrap_or(0),
                0,
            )
            .unwrap_or_else(|| DateTime::<Utc>::MIN_UTC);

            content.push_str(&format!(
                "{} [{}]: {}\n\n",
                author,
                timestamp.format("%H:%M"),
                message.text
            ));
        }

        content.trim().to_string()
    }

    pub fn to_connector_event(
        &self,
        sync_run_id: String,
        source_id: String,
        content_id: String,
        group_email: &str,
    ) -> ConnectorEvent {
        let title = if self.is_thread {
            format!("Thread in #{} - {}", self.channel_name, self.date)
        } else {
            format!("#{} - {}", self.channel_name, self.date)
        };

        let authors: Vec<String> = self
            .messages
            .iter()
            .map(|(_, author)| author.clone())
            .collect::<std::collections::HashSet<_>>()
            .into_iter()
            .collect();

        let first_ts = self
            .messages
            .first()
            .map(|(msg, _)| msg.ts.clone())
            .unwrap_or_default();
        let last_ts = self
            .messages
            .last()
            .map(|(msg, _)| msg.ts.clone())
            .unwrap_or_default();

        let created_at = DateTime::from_timestamp(
            first_ts
                .split('.')
                .next()
                .unwrap_or("0")
                .parse::<i64>()
                .unwrap_or(0),
            0,
        )
        .map(|dt| {
            OffsetDateTime::from_unix_timestamp(dt.timestamp())
                .unwrap_or_else(|_| OffsetDateTime::UNIX_EPOCH)
        });

        let updated_at = DateTime::from_timestamp(
            last_ts
                .split('.')
                .next()
                .unwrap_or("0")
                .parse::<i64>()
                .unwrap_or(0),
            0,
        )
        .map(|dt| {
            OffsetDateTime::from_unix_timestamp(dt.timestamp())
                .unwrap_or_else(|_| OffsetDateTime::UNIX_EPOCH)
        });

        let mut extra = HashMap::new();

        // Store Slack-specific hierarchical data (non-attribute fields only)
        let mut slack_metadata = HashMap::new();
        slack_metadata.insert("channel_id".to_string(), serde_json::json!(self.channel_id));
        slack_metadata.insert(
            "message_count".to_string(),
            serde_json::json!(self.message_count()),
        );
        slack_metadata.insert("authors".to_string(), serde_json::json!(authors));
        slack_metadata.insert("date".to_string(), serde_json::json!(self.date.to_string()));
        if let Some(thread_ts) = &self.thread_ts {
            slack_metadata.insert("thread_ts".to_string(), serde_json::json!(thread_ts));
        }
        extra.insert("slack".to_string(), serde_json::json!(slack_metadata));

        let document_id = if self.is_thread {
            format!(
                "slack_thread_{}_{}",
                self.channel_id,
                self.thread_ts.as_ref().unwrap()
            )
        } else {
            match self.part {
                Some(n) => format!("slack_channel_{}_{}_p{}", self.channel_id, self.date, n),
                None => format!("slack_channel_{}_{}", self.channel_id, self.date),
            }
        };

        let metadata = DocumentMetadata {
            title: Some(title),
            author: Some(if authors.len() == 1 {
                authors[0].clone()
            } else {
                "Multiple authors".to_string()
            }),
            created_at,
            updated_at,
            content_type: Some("message".to_string()),
            mime_type: Some("text/plain".to_string()),
            size: Some(self.content_size().to_string()),
            url: Some(format!(
                "slack://channel/{}/archive/{}",
                self.channel_id, self.date
            )),
            path: Some(format!("#{}", self.channel_name)), // Display channel as path
            extra: Some(extra),
        };

        // Permissions reference the channel-as-group; the group's membership
        // is synced via a separate `GroupMembershipSync` event so it's not
        // duplicated across every doc in the channel.
        let permissions = DocumentPermissions {
            public: false,
            users: vec![],
            groups: vec![group_email.to_string()],
        };

        let attributes = self.to_attributes().into_attributes();

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

    pub fn to_update_event(
        &self,
        sync_run_id: String,
        source_id: String,
        content_id: String,
        group_email: &str,
    ) -> ConnectorEvent {
        let event = self.to_connector_event(sync_run_id, source_id, content_id, group_email);
        if let ConnectorEvent::DocumentCreated {
            sync_run_id,
            source_id,
            document_id,
            content_id,
            metadata,
            permissions,
            attributes,
        } = event
        {
            ConnectorEvent::DocumentUpdated {
                sync_run_id,
                source_id,
                document_id,
                content_id,
                metadata,
                permissions: Some(permissions),
                attributes,
            }
        } else {
            event
        }
    }

    pub fn to_attributes(&self) -> SlackMessageAttributes {
        SlackMessageAttributes {
            channel_name: self.channel_name.clone(),
            is_thread: self.is_thread,
        }
    }
}

impl SlackFile {
    pub fn to_connector_event(
        &self,
        sync_run_id: String,
        source_id: String,
        channel_id: String,
        channel_name: String,
        content_id: String,
        group_email: &str,
    ) -> ConnectorEvent {
        let document_id = format!("slack_file_{}", self.id);

        let mut extra = HashMap::new();

        // Store Slack-specific file metadata (non-attribute fields only)
        let mut slack_metadata = HashMap::new();
        slack_metadata.insert("channel_id".to_string(), json!(channel_id.clone()));
        slack_metadata.insert("file_name".to_string(), json!(self.name));
        slack_metadata.insert("file_id".to_string(), json!(self.id));
        extra.insert("slack".to_string(), json!(slack_metadata));

        let metadata = DocumentMetadata {
            title: self.title.clone().or_else(|| Some(self.name.clone())),
            author: None,
            created_at: None,
            updated_at: None,
            content_type: None,
            mime_type: self.mimetype.clone(),
            size: Some(self.size.to_string()),
            url: self.permalink.clone(),
            path: Some(format!("#{}/{}", channel_name, self.name)),
            extra: Some(extra),
        };

        let permissions = DocumentPermissions {
            public: false,
            users: vec![],
            groups: vec![group_email.to_string()],
        };

        let attributes = SlackFileAttributes {
            channel_name: channel_name.clone(),
        }
        .into_attributes();

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

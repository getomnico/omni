use chrono::{DateTime, NaiveDate, Utc};
use serde::{Deserialize, Serialize};
use shared::models::{ConnectorEvent, DocumentMetadata, DocumentPermissions};
use sqlx::types::time::OffsetDateTime;
use std::collections::HashMap;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SlackUser {
    pub id: String,
    pub name: String,
    pub real_name: Option<String>,
    pub is_bot: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SlackChannel {
    pub id: String,
    pub name: String,
    #[serde(rename = "is_channel")]
    pub is_public: bool,
    pub is_private: bool,
    pub is_member: bool,
    pub num_members: Option<i32>,
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

    pub fn to_connector_event(&self, sync_run_id: String, source_id: String) -> ConnectorEvent {
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
        extra.insert(
            "channel_id".to_string(),
            serde_json::Value::String(self.channel_id.clone()),
        );
        extra.insert(
            "channel_name".to_string(),
            serde_json::Value::String(self.channel_name.clone()),
        );
        extra.insert(
            "message_count".to_string(),
            serde_json::Value::Number(self.message_count().into()),
        );
        extra.insert(
            "authors".to_string(),
            serde_json::Value::Array(
                authors
                    .iter()
                    .map(|a| serde_json::Value::String(a.clone()))
                    .collect(),
            ),
        );
        extra.insert(
            "date".to_string(),
            serde_json::Value::String(self.date.to_string()),
        );
        extra.insert(
            "is_thread".to_string(),
            serde_json::Value::Bool(self.is_thread),
        );
        if let Some(thread_ts) = &self.thread_ts {
            extra.insert(
                "thread_ts".to_string(),
                serde_json::Value::String(thread_ts.clone()),
            );
        }

        let document_id = if self.is_thread {
            format!(
                "slack_thread_{}_{}",
                self.channel_id,
                self.thread_ts.as_ref().unwrap()
            )
        } else {
            format!("slack_channel_{}_{}", self.channel_id, self.date)
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
            mime_type: Some("text/plain".to_string()),
            size: Some(self.content_size().to_string()),
            url: Some(format!(
                "slack://channel/{}/archive/{}",
                self.channel_id, self.date
            )),
            parent_id: None,
            extra: Some(extra),
        };

        let permissions = DocumentPermissions {
            public: false,
            users: vec![],
            groups: vec![self.channel_id.clone()],
        };

        ConnectorEvent::DocumentCreated {
            sync_run_id,
            source_id,
            document_id,
            content: self.to_document_content(),
            metadata,
            permissions,
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
        content: String,
    ) -> ConnectorEvent {
        let document_id = format!("slack_file_{}", self.id);

        let metadata = DocumentMetadata {
            title: self.title.clone().or_else(|| Some(self.name.clone())),
            author: None,
            created_at: None,
            updated_at: None,
            mime_type: self.mimetype.clone(),
            size: Some(self.size.to_string()),
            url: self.permalink.clone(),
            parent_id: Some(channel_id.clone()),
            extra: Some({
                let mut extra = HashMap::new();
                extra.insert(
                    "channel_id".to_string(),
                    serde_json::Value::String(channel_id.clone()),
                );
                extra.insert(
                    "channel_name".to_string(),
                    serde_json::Value::String(channel_name),
                );
                extra.insert(
                    "file_name".to_string(),
                    serde_json::Value::String(self.name.clone()),
                );
                extra
            }),
        };

        let permissions = DocumentPermissions {
            public: false,
            users: vec![],
            groups: vec![channel_id],
        };

        ConnectorEvent::DocumentCreated {
            sync_run_id,
            source_id,
            document_id,
            content,
            metadata,
            permissions,
        }
    }
}

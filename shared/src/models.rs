use pgvector::Vector;
use serde::{Deserialize, Serialize};
use serde_json::Value as JsonValue;
use sqlx::types::time::OffsetDateTime;
use sqlx::FromRow;
use std::collections::HashMap;

#[derive(Debug, Clone, Copy, Serialize, Deserialize, sqlx::Type, PartialEq)]
#[sqlx(type_name = "text", rename_all = "lowercase")]
pub enum UserRole {
    Admin,
    User,
    Viewer,
}

#[derive(Debug, Clone, Serialize, Deserialize, FromRow)]
pub struct User {
    pub id: String,
    pub email: String,
    pub password_hash: String,
    pub full_name: Option<String>,
    pub avatar_url: Option<String>,
    pub role: UserRole,
    pub is_active: bool,
    pub created_at: OffsetDateTime,
    pub updated_at: OffsetDateTime,
    pub last_login_at: Option<OffsetDateTime>,
}

#[derive(Debug, Clone, Serialize, Deserialize, FromRow)]
pub struct Source {
    pub id: String,
    pub name: String,
    pub source_type: SourceType,
    pub config: JsonValue,
    pub is_active: bool,
    pub last_sync_at: Option<OffsetDateTime>,
    pub sync_status: Option<String>,
    pub sync_error: Option<String>,
    pub created_at: OffsetDateTime,
    pub updated_at: OffsetDateTime,
    pub created_by: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, FromRow)]
pub struct Document {
    pub id: String,
    pub source_id: String,
    pub external_id: String,
    pub title: String,
    pub content: Option<String>,
    pub content_type: Option<String>,
    pub file_size: Option<i64>,
    pub file_extension: Option<String>,
    pub url: Option<String>,
    pub parent_id: Option<String>,
    pub metadata: JsonValue,
    pub permissions: JsonValue,
    pub created_at: OffsetDateTime,
    pub updated_at: OffsetDateTime,
    pub last_indexed_at: OffsetDateTime,
}

#[derive(Debug, Clone, Serialize, Deserialize, FromRow)]
pub struct Embedding {
    pub id: String,
    pub document_id: String,
    pub chunk_index: i32,
    pub chunk_text: String,
    pub embedding: Vector,
    pub model_name: String,
    pub created_at: OffsetDateTime,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, sqlx::Type, PartialEq)]
#[sqlx(type_name = "varchar", rename_all = "snake_case")]
pub enum SourceType {
    GoogleDrive,
    Gmail,
    Confluence,
    Jira,
    Slack,
    Github,
    LocalFiles,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, sqlx::Type, PartialEq)]
#[sqlx(type_name = "text", rename_all = "lowercase")]
pub enum OAuthProvider {
    Google,
    Slack,
    Atlassian,
    Github,
    Microsoft,
}

#[derive(Debug, Clone, Serialize, Deserialize, FromRow)]
pub struct OAuthCredentials {
    pub id: String,
    pub source_id: String,
    pub provider: OAuthProvider,
    pub client_id: Option<String>,
    pub client_secret: Option<String>,
    pub access_token: Option<String>,
    pub refresh_token: Option<String>,
    pub token_type: Option<String>,
    pub expires_at: Option<OffsetDateTime>,
    pub scopes: Option<Vec<String>>,
    pub metadata: JsonValue,
    pub created_at: OffsetDateTime,
    pub updated_at: OffsetDateTime,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DocumentMetadata {
    pub title: Option<String>,
    pub author: Option<String>,
    pub created_at: Option<OffsetDateTime>,
    pub updated_at: Option<OffsetDateTime>,
    pub mime_type: Option<String>,
    pub size: Option<String>,
    pub url: Option<String>,
    pub parent_id: Option<String>,
    pub extra: Option<HashMap<String, JsonValue>>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DocumentPermissions {
    pub public: bool,
    pub users: Vec<String>,
    pub groups: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ConnectorEvent {
    DocumentCreated {
        source_id: String,
        document_id: String,
        content: String,
        metadata: DocumentMetadata,
        permissions: DocumentPermissions,
    },
    DocumentUpdated {
        source_id: String,
        document_id: String,
        content: String,
        metadata: DocumentMetadata,
        permissions: Option<DocumentPermissions>,
    },
    DocumentDeleted {
        source_id: String,
        document_id: String,
    },
}

impl ConnectorEvent {
    pub fn source_id(&self) -> &str {
        match self {
            ConnectorEvent::DocumentCreated { source_id, .. } => source_id,
            ConnectorEvent::DocumentUpdated { source_id, .. } => source_id,
            ConnectorEvent::DocumentDeleted { source_id, .. } => source_id,
        }
    }

    pub fn document_id(&self) -> &str {
        match self {
            ConnectorEvent::DocumentCreated { document_id, .. } => document_id,
            ConnectorEvent::DocumentUpdated { document_id, .. } => document_id,
            ConnectorEvent::DocumentDeleted { document_id, .. } => document_id,
        }
    }
}

#[derive(Debug, Clone)]
pub struct DocumentChunk {
    pub text: String,
    pub index: i32,
}

impl Document {
    /// Chunk document content into smaller pieces for embedding generation
    pub fn chunk_content(&self, max_chunk_size: usize) -> Vec<DocumentChunk> {
        let content = match &self.content {
            Some(content) => content,
            None => return vec![], // No content to chunk
        };

        if content.len() <= max_chunk_size {
            return vec![DocumentChunk {
                text: content.clone(),
                index: 0,
            }];
        }

        let mut chunks = Vec::new();
        let mut current_pos = 0;
        let mut chunk_index = 0;

        while current_pos < content.len() {
            let end_pos = std::cmp::min(current_pos + max_chunk_size, content.len());

            // Try to break at sentence boundaries for better semantic coherence
            let chunk_text = if end_pos < content.len() {
                // Look for sentence endings within the last 100 characters
                let search_start = std::cmp::max(current_pos, end_pos.saturating_sub(100));
                let search_slice = &content[search_start..end_pos];

                if let Some(sentence_end) = search_slice.rfind('.') {
                    let actual_end = search_start + sentence_end + 1;
                    content[current_pos..actual_end].to_string()
                } else if let Some(paragraph_end) = search_slice.rfind('\n') {
                    let actual_end = search_start + paragraph_end + 1;
                    content[current_pos..actual_end].to_string()
                } else {
                    // Fallback to character boundary
                    content[current_pos..end_pos].to_string()
                }
            } else {
                content[current_pos..end_pos].to_string()
            };

            let chunk_end = current_pos + chunk_text.len();
            chunks.push(DocumentChunk {
                text: chunk_text,
                index: chunk_index,
            });

            current_pos = chunk_end;
            chunk_index += 1;

            // Prevent infinite loops
            if current_pos >= content.len() {
                break;
            }
        }

        chunks
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FacetValue {
    pub value: String,
    pub count: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Facet {
    pub name: String,
    pub values: Vec<FacetValue>,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, sqlx::Type, PartialEq)]
#[sqlx(type_name = "text", rename_all = "lowercase")]
pub enum EventStatus {
    Pending,
    Processing,
    Completed,
    Failed,
    #[serde(rename = "dead_letter")]
    DeadLetter,
}

#[derive(Debug, Clone, Serialize, Deserialize, FromRow)]
pub struct ConnectorEventQueueItem {
    pub id: String,
    pub source_id: String,
    pub event_type: String,
    pub payload: JsonValue,
    pub status: EventStatus,
    pub retry_count: i32,
    pub max_retries: i32,
    pub created_at: OffsetDateTime,
    pub processed_at: Option<OffsetDateTime>,
    pub error_message: Option<String>,
}

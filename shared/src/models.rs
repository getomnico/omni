use pgvector::Vector;
use serde::{Deserialize, Serialize};
use serde_json::Value as JsonValue;
use sqlx::types::time::OffsetDateTime;
use sqlx::FromRow;
use std::collections::HashMap;

#[derive(Debug, Clone, Copy, Serialize, Deserialize, sqlx::Type, PartialEq)]
#[sqlx(type_name = "varchar", rename_all = "lowercase")]
pub enum UserRole {
    Admin,
    User,
    Viewer,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, sqlx::Type, PartialEq)]
#[sqlx(type_name = "varchar", rename_all = "snake_case")]
pub enum AuthMethod {
    Password,
    MagicLink,
    Both,
}

#[derive(Debug, Clone, Serialize, Deserialize, FromRow)]
pub struct User {
    pub id: String,
    pub email: String,
    #[sqlx(default)]
    pub password_hash: Option<String>,
    pub full_name: Option<String>,
    pub avatar_url: Option<String>,
    pub role: UserRole,
    pub is_active: bool,
    pub auth_method: AuthMethod,
    pub domain: Option<String>,
    #[serde(with = "time::serde::iso8601")]
    pub created_at: OffsetDateTime,
    #[serde(with = "time::serde::iso8601")]
    pub updated_at: OffsetDateTime,
    #[serde(with = "time::serde::iso8601::option")]
    pub last_login_at: Option<OffsetDateTime>,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, sqlx::Type, PartialEq)]
#[sqlx(type_name = "varchar", rename_all = "lowercase")]
#[serde(rename_all = "lowercase")]
pub enum UserFilterMode {
    All,
    Whitelist,
    Blacklist,
}

#[derive(Debug, Clone, Serialize, Deserialize, FromRow)]
pub struct Source {
    pub id: String,
    pub name: String,
    pub source_type: SourceType,
    pub config: JsonValue,
    pub is_active: bool,
    pub is_deleted: bool,
    #[serde(with = "time::serde::iso8601::option")]
    pub last_sync_at: Option<OffsetDateTime>,
    pub sync_status: Option<String>,
    pub sync_error: Option<String>,
    pub user_filter_mode: UserFilterMode,
    pub user_whitelist: Option<JsonValue>,
    pub user_blacklist: Option<JsonValue>,
    #[serde(with = "time::serde::iso8601")]
    pub created_at: OffsetDateTime,
    #[serde(with = "time::serde::iso8601")]
    pub updated_at: OffsetDateTime,
    pub created_by: String,
}

impl Source {
    pub fn get_user_whitelist(&self) -> Vec<String> {
        self.user_whitelist
            .as_ref()
            .and_then(|list| list.as_array())
            .map(|array| {
                array
                    .iter()
                    .filter_map(|v| v.as_str().map(|s| s.to_string()))
                    .collect()
            })
            .unwrap_or_default()
    }

    pub fn get_user_blacklist(&self) -> Vec<String> {
        self.user_blacklist
            .as_ref()
            .and_then(|list| list.as_array())
            .map(|array| {
                array
                    .iter()
                    .filter_map(|v| v.as_str().map(|s| s.to_string()))
                    .collect()
            })
            .unwrap_or_default()
    }

    pub fn should_index_user(&self, user_email: &str) -> bool {
        match self.user_filter_mode {
            UserFilterMode::All => true,
            UserFilterMode::Whitelist => {
                self.get_user_whitelist().contains(&user_email.to_string())
            }
            UserFilterMode::Blacklist => {
                !self.get_user_blacklist().contains(&user_email.to_string())
            }
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, FromRow)]
pub struct Document {
    pub id: String,
    pub source_id: String,
    pub external_id: String,
    pub title: String,
    pub content_id: Option<String>, // Content blob ID in content_blobs table
    pub content_type: Option<String>,
    pub file_size: Option<i64>,
    pub file_extension: Option<String>,
    pub url: Option<String>,
    pub metadata: JsonValue,
    pub permissions: JsonValue,
    pub attributes: JsonValue, // Structured key-value attributes for filtering
    #[serde(with = "time::serde::iso8601")]
    pub created_at: OffsetDateTime,
    #[serde(with = "time::serde::iso8601")]
    pub updated_at: OffsetDateTime,
    #[serde(with = "time::serde::iso8601")]
    pub last_indexed_at: OffsetDateTime,
}

#[derive(Debug, Clone, Serialize, Deserialize, FromRow)]
pub struct Embedding {
    pub id: String,
    pub document_id: String,
    pub chunk_index: i32,
    pub chunk_start_offset: i32, // Character start offset in original document
    pub chunk_end_offset: i32,   // Character end offset in original document
    pub embedding: Vector,
    pub model_name: String,
    #[serde(with = "time::serde::iso8601")]
    pub created_at: OffsetDateTime,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, sqlx::Type, PartialEq, Hash)]
#[sqlx(type_name = "varchar", rename_all = "snake_case")]
#[serde(rename_all = "snake_case")]
pub enum SourceType {
    GoogleDrive,
    Gmail,
    Confluence,
    Jira,
    Slack,
    Github,
    LocalFiles,
    FileSystem,
    Web,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, sqlx::Type, PartialEq)]
#[sqlx(type_name = "varchar", rename_all = "lowercase")]
#[serde(rename_all = "lowercase")]
pub enum ServiceProvider {
    Google,
    Slack,
    Atlassian,
    Github,
    Microsoft,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, sqlx::Type, PartialEq)]
#[sqlx(type_name = "varchar", rename_all = "snake_case")]
#[serde(rename_all = "snake_case")]
pub enum AuthType {
    Jwt,
    ApiKey,
    BasicAuth,
    BearerToken,
    BotToken,
}

#[derive(Debug, Clone, Serialize, Deserialize, FromRow)]
pub struct ServiceCredentials {
    pub id: String,
    pub source_id: String,
    pub provider: ServiceProvider,
    pub auth_type: AuthType,
    pub principal_email: Option<String>,
    pub credentials: JsonValue,
    pub config: JsonValue,
    #[serde(with = "time::serde::iso8601::option")]
    pub expires_at: Option<OffsetDateTime>,
    #[serde(with = "time::serde::iso8601::option")]
    pub last_validated_at: Option<OffsetDateTime>,
    #[serde(with = "time::serde::iso8601")]
    pub created_at: OffsetDateTime,
    #[serde(with = "time::serde::iso8601")]
    pub updated_at: OffsetDateTime,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConfluenceSourceConfig {
    pub base_url: String,
    #[serde(default)]
    pub space_filters: Option<Vec<String>>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct JiraSourceConfig {
    pub base_url: String,
    #[serde(default)]
    pub project_filters: Option<Vec<String>>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct DocumentMetadata {
    pub title: Option<String>,
    pub author: Option<String>,
    #[serde(with = "time::serde::iso8601::option")]
    pub created_at: Option<OffsetDateTime>,
    #[serde(with = "time::serde::iso8601::option")]
    pub updated_at: Option<OffsetDateTime>,
    pub mime_type: Option<String>,
    pub size: Option<String>,
    pub url: Option<String>,
    pub path: Option<String>, // Generic display path for hierarchical context
    pub extra: Option<HashMap<String, JsonValue>>, // Connector-specific metadata
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DocumentPermissions {
    pub public: bool,
    pub users: Vec<String>,
    pub groups: Vec<String>,
}

/// Structured attributes for filtering and faceting.
/// Stored as JSONB, indexed by ParadeDB for FTS and filtering.
/// NOT included in embeddings - only textual content is embedded.
pub type DocumentAttributes = HashMap<String, JsonValue>;

/// Attribute filter for search queries.
/// Supports exact match, multi-value OR, and range queries.
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(untagged)]
pub enum AttributeFilter {
    /// Single value exact match
    Exact(JsonValue),
    /// Multiple values (OR match)
    AnyOf(Vec<JsonValue>),
    /// Range query (for dates, numbers)
    Range {
        #[serde(skip_serializing_if = "Option::is_none")]
        gte: Option<JsonValue>,
        #[serde(skip_serializing_if = "Option::is_none")]
        lte: Option<JsonValue>,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ConnectorEvent {
    DocumentCreated {
        sync_run_id: String,
        source_id: String,
        document_id: String,
        content_id: String,
        metadata: DocumentMetadata,
        permissions: DocumentPermissions,
        #[serde(default)]
        attributes: Option<DocumentAttributes>,
    },
    DocumentUpdated {
        sync_run_id: String,
        source_id: String,
        document_id: String,
        content_id: String,
        metadata: DocumentMetadata,
        permissions: Option<DocumentPermissions>,
        #[serde(default)]
        attributes: Option<DocumentAttributes>,
    },
    DocumentDeleted {
        sync_run_id: String,
        source_id: String,
        document_id: String,
    },
}

impl ConnectorEvent {
    pub fn sync_run_id(&self) -> &str {
        match self {
            ConnectorEvent::DocumentCreated { sync_run_id, .. } => sync_run_id,
            ConnectorEvent::DocumentUpdated { sync_run_id, .. } => sync_run_id,
            ConnectorEvent::DocumentDeleted { sync_run_id, .. } => sync_run_id,
        }
    }

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

// Note: Document chunking is now handled by the indexer service
// which fetches content from LOB storage and uses the ContentChunker utility

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

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChunkResult {
    pub document_id: String,
    pub similarity_score: f32,
    pub chunk_start_offset: i32,
    pub chunk_end_offset: i32,
    pub chunk_index: i32,
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
    pub sync_run_id: String,
    pub source_id: String,
    pub event_type: String,
    pub payload: JsonValue,
    pub status: EventStatus,
    pub retry_count: i32,
    pub max_retries: i32,
    #[serde(with = "time::serde::iso8601")]
    pub created_at: OffsetDateTime,
    #[serde(with = "time::serde::iso8601::option")]
    pub processed_at: Option<OffsetDateTime>,
    pub error_message: Option<String>,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, sqlx::Type, PartialEq)]
#[sqlx(type_name = "varchar", rename_all = "lowercase")]
pub enum SyncType {
    Full,
    Incremental,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, sqlx::Type, PartialEq)]
#[sqlx(type_name = "varchar", rename_all = "lowercase")]
pub enum SyncStatus {
    Running,
    Completed,
    Failed,
    Cancelled,
}

#[derive(Debug, Clone, Serialize, Deserialize, FromRow)]
pub struct SyncRun {
    pub id: String,
    pub source_id: String,
    pub sync_type: SyncType,
    #[serde(with = "time::serde::iso8601::option")]
    pub started_at: Option<OffsetDateTime>,
    #[serde(with = "time::serde::iso8601::option")]
    pub completed_at: Option<OffsetDateTime>,
    pub status: SyncStatus,
    pub documents_scanned: i32,
    pub documents_processed: i32,
    pub documents_updated: i32,
    pub error_message: Option<String>,
    #[serde(with = "time::serde::iso8601")]
    pub created_at: OffsetDateTime,
    #[serde(with = "time::serde::iso8601")]
    pub updated_at: OffsetDateTime,
}

#[derive(Debug, Clone, Serialize, Deserialize, FromRow)]
pub struct WebhookChannel {
    pub id: String,
    pub source_id: String,
    pub channel_id: String,
    pub resource_id: String,
    pub resource_uri: Option<String>,
    pub webhook_url: String,
    #[serde(with = "time::serde::iso8601::option")]
    pub expires_at: Option<OffsetDateTime>,
    #[serde(with = "time::serde::iso8601")]
    pub created_at: OffsetDateTime,
    #[serde(with = "time::serde::iso8601")]
    pub updated_at: OffsetDateTime,
}

#[derive(Debug, Clone, Serialize, Deserialize, FromRow)]
pub struct ApprovedDomain {
    pub id: String,
    pub domain: String,
    pub approved_by: String,
    #[serde(with = "time::serde::iso8601")]
    pub created_at: OffsetDateTime,
    #[serde(with = "time::serde::iso8601")]
    pub updated_at: OffsetDateTime,
}

#[derive(Debug, Clone, Serialize, Deserialize, FromRow)]
pub struct MagicLink {
    pub id: String,
    pub email: String,
    pub token_hash: String,
    pub expires_at: OffsetDateTime,
    #[serde(with = "time::serde::iso8601::option")]
    pub used_at: Option<OffsetDateTime>,
    #[serde(with = "time::serde::iso8601")]
    pub created_at: OffsetDateTime,
    pub user_id: Option<String>,
}

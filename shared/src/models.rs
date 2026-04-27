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

#[derive(Debug, Clone, Copy, Serialize, Deserialize, sqlx::Type, PartialEq, Eq)]
#[sqlx(type_name = "text", rename_all = "lowercase")]
#[serde(rename_all = "lowercase")]
pub enum SourceScope {
    /// Admin-set-up source shared across the org. Sync uses the org credential;
    /// MCP write tools require per-user credentials granted via OAuth.
    Org,
    /// Personal source owned by `created_by`. The single credential row covers
    /// both reads and writes.
    User,
}

#[derive(Debug, Clone, Serialize, Deserialize, FromRow)]
pub struct Source {
    pub id: String,
    pub name: String,
    pub source_type: SourceType,
    pub config: JsonValue,
    pub is_active: bool,
    pub is_deleted: bool,
    pub scope: SourceScope,
    pub user_filter_mode: UserFilterMode,
    pub user_whitelist: Option<JsonValue>,
    pub user_blacklist: Option<JsonValue>,
    pub connector_state: Option<JsonValue>,
    pub sync_interval_seconds: Option<i32>,
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
    pub dimensions: i16,
    #[serde(with = "time::serde::iso8601")]
    pub created_at: OffsetDateTime,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, sqlx::Type, PartialEq, Eq, Hash)]
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
    Notion,
    Hubspot,
    OneDrive,
    SharePoint,
    Outlook,
    OutlookCalendar,
    MsTeams,
    Fireflies,
    Imap,
    Clickup,
    Linear,
    PaperlessNgx,
    Nextcloud,
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
    Notion,
    Hubspot,
    Fireflies,
    Imap,
    Clickup,
    Linear,
    #[sqlx(rename = "paperless_ngx")]
    #[serde(rename = "paperless_ngx")]
    PaperlessNgx,
    Nextcloud,
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
    #[sqlx(rename = "oauth")]
    #[serde(rename = "oauth")]
    OAuth,
}

#[derive(Debug, Clone, Serialize, Deserialize, FromRow)]
pub struct ServiceCredentials {
    pub id: String,
    pub source_id: String,
    /// `Some` => per-user credential for an org-wide source (used for write tools by that user).
    /// `None` => org-wide credential (used for sync and reads).
    pub user_id: Option<String>,
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

#[derive(Debug, Clone, Serialize, Deserialize, FromRow)]
pub struct ConnectorConfigRow {
    pub provider: String,
    pub config: JsonValue,
    #[serde(with = "time::serde::iso8601")]
    pub updated_at: OffsetDateTime,
    pub updated_by: Option<String>,
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
    #[serde(default, with = "time::serde::rfc3339::option")]
    pub created_at: Option<OffsetDateTime>,
    #[serde(default, with = "time::serde::rfc3339::option")]
    pub updated_at: Option<OffsetDateTime>,
    pub content_type: Option<String>,
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

#[derive(Debug, Clone, Serialize, Deserialize, FromRow)]
pub struct Group {
    pub id: String,
    pub source_id: String,
    pub email: String,
    pub display_name: Option<String>,
    pub description: Option<String>,
    #[serde(with = "time::serde::iso8601")]
    pub synced_at: OffsetDateTime,
}

#[derive(Debug, Clone, Serialize, Deserialize, FromRow)]
pub struct GroupMembership {
    pub id: String,
    pub group_id: String,
    pub member_email: String,
    pub role: Option<String>,
    #[serde(with = "time::serde::iso8601")]
    pub synced_at: OffsetDateTime,
}

/// Structured attributes for filtering and faceting.
/// Stored as JSONB, indexed by ParadeDB for FTS and filtering.
/// NOT included in embeddings - only textual content is embedded.
pub type DocumentAttributes = HashMap<String, JsonValue>;

/// Attribute filter for search queries.
/// Supports exact match, multi-value OR, and range queries.
#[derive(Debug, Clone)]
pub struct DateFilter {
    pub after: Option<OffsetDateTime>,
    pub before: Option<OffsetDateTime>,
}

#[derive(Debug, Clone, PartialEq, Deserialize, Serialize)]
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
pub struct SearchOperator {
    pub operator: String,
    pub attribute_key: String,
    #[serde(default = "default_search_operator_value_type")]
    pub value_type: String, // "person", "text", "datetime"
}

fn default_search_operator_value_type() -> String {
    "text".to_string()
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ActionDefinition {
    pub name: String,
    pub description: String,
    pub input_schema: JsonValue,
    #[serde(default = "default_action_mode")]
    pub mode: String,
}

fn default_action_mode() -> String {
    "write".to_string()
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct McpResourceDefinition {
    pub uri_template: String,
    pub name: String,
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default)]
    pub mime_type: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct McpPromptArgument {
    pub name: String,
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default)]
    pub required: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct McpPromptDefinition {
    pub name: String,
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default)]
    pub arguments: Vec<McpPromptArgument>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConnectorManifest {
    pub name: String,
    pub display_name: String,
    pub version: String,
    pub sync_modes: Vec<SyncType>,
    pub connector_id: String,
    pub connector_url: String,
    #[serde(default)]
    pub source_types: Vec<SourceType>,
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default)]
    pub actions: Vec<ActionDefinition>,
    #[serde(default)]
    pub search_operators: Vec<SearchOperator>,
    #[serde(default)]
    pub read_only: bool,
    #[serde(default)]
    pub extra_schema: Option<JsonValue>,
    #[serde(default)]
    pub attributes_schema: Option<JsonValue>,
    #[serde(default)]
    pub mcp_enabled: bool,
    #[serde(default)]
    pub resources: Vec<McpResourceDefinition>,
    #[serde(default)]
    pub prompts: Vec<McpPromptDefinition>,
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
    GroupMembershipSync {
        sync_run_id: String,
        source_id: String,
        group_email: String,
        group_name: Option<String>,
        member_emails: Vec<String>,
    },
}

impl ConnectorEvent {
    pub fn sync_run_id(&self) -> &str {
        match self {
            ConnectorEvent::DocumentCreated { sync_run_id, .. } => sync_run_id,
            ConnectorEvent::DocumentUpdated { sync_run_id, .. } => sync_run_id,
            ConnectorEvent::DocumentDeleted { sync_run_id, .. } => sync_run_id,
            ConnectorEvent::GroupMembershipSync { sync_run_id, .. } => sync_run_id,
        }
    }

    pub fn source_id(&self) -> &str {
        match self {
            ConnectorEvent::DocumentCreated { source_id, .. } => source_id,
            ConnectorEvent::DocumentUpdated { source_id, .. } => source_id,
            ConnectorEvent::DocumentDeleted { source_id, .. } => source_id,
            ConnectorEvent::GroupMembershipSync { source_id, .. } => source_id,
        }
    }

    pub fn document_id(&self) -> &str {
        match self {
            ConnectorEvent::DocumentCreated { document_id, .. } => document_id,
            ConnectorEvent::DocumentUpdated { document_id, .. } => document_id,
            ConnectorEvent::DocumentDeleted { document_id, .. } => document_id,
            ConnectorEvent::GroupMembershipSync { group_email, .. } => group_email,
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
    #[serde(skip_serializing_if = "Option::is_none")]
    pub count: Option<i64>,
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

#[derive(Debug, Clone, Copy, Serialize, Deserialize, sqlx::Type, PartialEq, Eq, Hash)]
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

/// Type/mode of a sync run. Serializes as a lowercase string on the wire
/// (`"full"`, `"incremental"`, `"realtime"`).
///
/// TODO: the Python (`sdk/python/omni_connector/models.py`) and TypeScript
/// (`sdk/typescript/src/models.ts`) SDKs currently expose this enum as
/// `SyncMode` with only `FULL` and `INCREMENTAL`. They should be renamed to
/// `SyncType` and grow a `REALTIME` variant to match the Rust canonical name.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, sqlx::Type, PartialEq, Eq, Hash)]
#[sqlx(type_name = "varchar", rename_all = "lowercase")]
#[serde(rename_all = "lowercase")]
pub enum SyncType {
    Full,
    Incremental,
    Realtime,
}

impl std::fmt::Display for SyncType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            SyncType::Full => write!(f, "full"),
            SyncType::Incremental => write!(f, "incremental"),
            SyncType::Realtime => write!(f, "realtime"),
        }
    }
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

/// Request sent from connector-manager to connectors to trigger a sync.
/// Connectors fetch their own source config and credentials from the database.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SyncRequest {
    pub sync_run_id: String,
    pub source_id: String,
    pub sync_mode: SyncType,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_sync_at: Option<String>,
}

/// Response from connector after receiving a sync request.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SyncResponse {
    pub status: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
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

#[derive(Debug, Clone, Serialize, Deserialize, FromRow)]
pub struct Person {
    pub id: String,
    pub email: String,
    pub display_name: Option<String>,
    pub given_name: Option<String>,
    pub surname: Option<String>,
    pub avatar_url: Option<String>,
    pub job_title: Option<String>,
    pub department: Option<String>,
    pub division: Option<String>,
    pub company_name: Option<String>,
    pub office_location: Option<String>,
    pub city: Option<String>,
    pub state: Option<String>,
    pub country: Option<String>,
    pub employee_id: Option<String>,
    pub employee_type: Option<String>,
    pub cost_center: Option<String>,
    pub manager_id: Option<String>,
    pub is_active: bool,
    pub metadata: JsonValue,
    pub external_id: Option<String>,
    #[serde(with = "time::serde::iso8601")]
    pub created_at: OffsetDateTime,
    #[serde(with = "time::serde::iso8601")]
    pub updated_at: OffsetDateTime,
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn make_source(
        filter_mode: UserFilterMode,
        whitelist: Option<JsonValue>,
        blacklist: Option<JsonValue>,
    ) -> Source {
        Source {
            id: "src-1".to_string(),
            name: "Test".to_string(),
            source_type: SourceType::Web,
            config: json!({}),
            is_active: true,
            is_deleted: false,
            scope: SourceScope::User,
            user_filter_mode: filter_mode,
            user_whitelist: whitelist,
            user_blacklist: blacklist,
            connector_state: None,
            sync_interval_seconds: None,
            created_at: OffsetDateTime::now_utc(),
            updated_at: OffsetDateTime::now_utc(),
            created_by: "admin".to_string(),
        }
    }

    #[test]
    fn test_should_index_user_all_mode() {
        let source = make_source(UserFilterMode::All, None, None);
        assert!(source.should_index_user("anyone@example.com"));
        assert!(source.should_index_user(""));
    }

    #[test]
    fn test_should_index_user_whitelist() {
        let source = make_source(
            UserFilterMode::Whitelist,
            Some(json!(["alice@corp.com", "bob@corp.com"])),
            None,
        );
        assert!(source.should_index_user("alice@corp.com"));
        assert!(source.should_index_user("bob@corp.com"));
        assert!(!source.should_index_user("eve@corp.com"));
    }

    #[test]
    fn test_should_index_user_blacklist() {
        let source = make_source(
            UserFilterMode::Blacklist,
            None,
            Some(json!(["blocked@corp.com"])),
        );
        assert!(!source.should_index_user("blocked@corp.com"));
        assert!(source.should_index_user("allowed@corp.com"));
    }

    #[test]
    fn test_get_user_whitelist_none() {
        let source = make_source(UserFilterMode::All, None, None);
        assert!(source.get_user_whitelist().is_empty());
    }

    #[test]
    fn test_get_user_whitelist_valid() {
        let source = make_source(
            UserFilterMode::Whitelist,
            Some(json!(["a@b.com", "c@d.com"])),
            None,
        );
        assert_eq!(
            source.get_user_whitelist(),
            vec!["a@b.com".to_string(), "c@d.com".to_string()]
        );
    }

    #[test]
    fn test_get_user_blacklist_valid() {
        let source = make_source(UserFilterMode::Blacklist, None, Some(json!(["x@y.com"])));
        assert_eq!(source.get_user_blacklist(), vec!["x@y.com".to_string()]);
    }

    #[test]
    fn test_attribute_filter_exact_string_deserialization() {
        let filter: AttributeFilter = serde_json::from_value(json!("engineering")).unwrap();
        assert!(matches!(filter, AttributeFilter::Exact(_)));
    }

    #[test]
    fn test_attribute_filter_exact_number_deserialization() {
        let filter: AttributeFilter = serde_json::from_value(json!(42)).unwrap();
        assert!(matches!(filter, AttributeFilter::Exact(_)));
    }

    #[test]
    fn test_attribute_filter_exact_round_trips() {
        let original = AttributeFilter::Exact(json!("team-a"));
        let serialized = serde_json::to_value(&original).unwrap();
        let deserialized: AttributeFilter = serde_json::from_value(serialized).unwrap();
        if let AttributeFilter::Exact(v) = deserialized {
            assert_eq!(v, json!("team-a"));
        } else {
            panic!("Expected Exact variant");
        }
    }

    #[test]
    fn test_attribute_filter_any_of_serializes_as_array() {
        let filter = AttributeFilter::AnyOf(vec![json!("a"), json!("b")]);
        let serialized = serde_json::to_value(&filter).unwrap();
        assert!(serialized.is_array());
        assert_eq!(serialized.as_array().unwrap().len(), 2);
    }

    #[test]
    fn test_attribute_filter_range_serializes_with_gte_lte() {
        let filter = AttributeFilter::Range {
            gte: Some(json!(10)),
            lte: Some(json!(100)),
        };
        let serialized = serde_json::to_value(&filter).unwrap();
        assert_eq!(serialized["gte"], json!(10));
        assert_eq!(serialized["lte"], json!(100));
    }

    #[test]
    fn test_connector_event_accessors() {
        let event = ConnectorEvent::DocumentCreated {
            sync_run_id: "run-1".to_string(),
            source_id: "src-1".to_string(),
            document_id: "doc-1".to_string(),
            content_id: "cnt-1".to_string(),
            metadata: DocumentMetadata::default(),
            permissions: DocumentPermissions {
                public: false,
                users: vec![],
                groups: vec![],
            },
            attributes: None,
        };
        assert_eq!(event.sync_run_id(), "run-1");
        assert_eq!(event.source_id(), "src-1");
        assert_eq!(event.document_id(), "doc-1");

        let deleted = ConnectorEvent::DocumentDeleted {
            sync_run_id: "run-2".to_string(),
            source_id: "src-2".to_string(),
            document_id: "doc-2".to_string(),
        };
        assert_eq!(deleted.sync_run_id(), "run-2");
        assert_eq!(deleted.source_id(), "src-2");
        assert_eq!(deleted.document_id(), "doc-2");
    }
}

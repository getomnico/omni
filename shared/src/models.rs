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
    pub embedding: Vector,
    pub model_name: String,
    pub created_at: OffsetDateTime,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, sqlx::Type, PartialEq)]
#[sqlx(type_name = "text", rename_all = "lowercase")]
pub enum SourceType {
    Google,
    Slack,
    Confluence,
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

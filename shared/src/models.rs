use pgvector::Vector;
use serde::{Deserialize, Serialize};
use sqlx::types::time::OffsetDateTime;
use sqlx::FromRow;

#[derive(Debug, Clone, Serialize, Deserialize, FromRow)]
pub struct User {
    pub id: String,
    pub email: String,
    pub password_hash: String,
    pub full_name: Option<String>,
    pub avatar_url: Option<String>,
    pub role: String,
    pub is_active: bool,
    pub created_at: OffsetDateTime,
    pub updated_at: OffsetDateTime,
    pub last_login_at: Option<OffsetDateTime>,
}

#[derive(Debug, Clone, Serialize, Deserialize, FromRow)]
pub struct Source {
    pub id: String,
    pub name: String,
    pub source_type: String,
    pub config: serde_json::Value,
    pub oauth_credentials: Option<serde_json::Value>,
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
    pub metadata: serde_json::Value,
    pub permissions: serde_json::Value,
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
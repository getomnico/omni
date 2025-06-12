use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DocumentMetadata {
    pub title: Option<String>,
    pub author: Option<String>,
    pub created_at: Option<DateTime<Utc>>,
    pub updated_at: Option<DateTime<Utc>>,
    pub mime_type: Option<String>,
    pub size: Option<i64>,
    pub url: Option<String>,
    pub parent_id: Option<String>,
    pub extra: HashMap<String, serde_json::Value>,
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

use serde::{Deserialize, Serialize};
use shared::models::{ConnectorEvent, DocumentMetadata, DocumentPermissions};
use std::collections::HashMap;
use sqlx::types::time::OffsetDateTime;
use chrono::{DateTime, Utc};


#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GoogleDriveFile {
    pub id: String,
    pub name: String,
    #[serde(rename = "mimeType")]
    pub mime_type: String,
    #[serde(rename = "webViewLink")]
    pub web_view_link: Option<String>,
    #[serde(rename = "createdTime")]
    pub created_time: Option<String>,
    #[serde(rename = "modifiedTime")]
    pub modified_time: Option<String>,
    pub size: Option<String>,
    pub parents: Option<Vec<String>>,
    pub shared: Option<bool>,
    pub permissions: Option<Vec<Permission>>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Permission {
    pub id: String,
    #[serde(rename = "type")]
    pub permission_type: String,
    #[serde(rename = "emailAddress")]
    pub email_address: Option<String>,
    pub role: String,
}

impl GoogleDriveFile {
    pub fn to_connector_event(self, source_id: String, content: String) -> ConnectorEvent {
        let mut users = Vec::new();

        if let Some(file_permissions) = &self.permissions {
            for perm in file_permissions {
                if let Some(email) = &perm.email_address {
                    users.push(email.clone());
                }
            }
        }

        let mut extra = HashMap::new();
        extra.insert("file_id".to_string(), serde_json::json!(self.id));
        extra.insert("shared".to_string(), serde_json::json!(self.shared.unwrap_or(false)));

        let metadata = DocumentMetadata {
            title: Some(self.name.clone()),
            author: None,
            created_at: self.created_time.as_ref().and_then(|t| {
                t.parse::<DateTime<Utc>>()
                    .ok()
                    .map(|dt| OffsetDateTime::from_unix_timestamp(dt.timestamp()).unwrap())
            }),
            updated_at: self.modified_time.as_ref().and_then(|t| {
                t.parse::<DateTime<Utc>>()
                    .ok()
                    .map(|dt| OffsetDateTime::from_unix_timestamp(dt.timestamp()).unwrap())
            }),
            mime_type: Some(self.mime_type.clone()),
            size: self.size.clone(),
            url: self.web_view_link.clone(),
            parent_id: self.parents.as_ref().and_then(|p| p.first().cloned()),
            extra: Some(extra),
        };

        let permissions = DocumentPermissions {
            public: false,
            users,
            groups: vec![],
        };

        ConnectorEvent::DocumentCreated {
            source_id,
            document_id: self.id.clone(),
            content,
            metadata,
            permissions,
        }
    }
}

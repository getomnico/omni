use serde::{Deserialize, Serialize};
use shared::models::{ConnectorEvent, DocumentMetadata, DocumentPermissions};
use sqlx::types::time::OffsetDateTime;
use std::collections::HashMap;
use std::path::PathBuf;
use std::time::SystemTime;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FilesystemFile {
    pub path: PathBuf,
    pub name: String,
    pub size: u64,
    pub mime_type: String,
    pub created_time: Option<SystemTime>,
    pub modified_time: Option<SystemTime>,
    pub is_directory: bool,
    pub permissions: FilesystemPermissions,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FilesystemPermissions {
    pub readable: bool,
    pub writable: bool,
    pub executable: bool,
}

impl FilesystemFile {
    pub fn to_connector_event(
        self,
        sync_run_id: String,
        source_id: String,
        content_id: String,
    ) -> ConnectorEvent {
        let mut extra = HashMap::new();
        extra.insert(
            "is_directory".to_string(),
            serde_json::json!(self.is_directory),
        );
        extra.insert(
            "readable".to_string(),
            serde_json::json!(self.permissions.readable),
        );
        extra.insert(
            "writable".to_string(),
            serde_json::json!(self.permissions.writable),
        );
        extra.insert(
            "executable".to_string(),
            serde_json::json!(self.permissions.executable),
        );

        let metadata = DocumentMetadata {
            title: Some(self.name.clone()),
            author: None,
            created_at: self.created_time.and_then(|t| {
                t.duration_since(SystemTime::UNIX_EPOCH)
                    .ok()
                    .and_then(|d| OffsetDateTime::from_unix_timestamp(d.as_secs() as i64).ok())
            }),
            updated_at: self.modified_time.and_then(|t| {
                t.duration_since(SystemTime::UNIX_EPOCH)
                    .ok()
                    .and_then(|d| OffsetDateTime::from_unix_timestamp(d.as_secs() as i64).ok())
            }),
            mime_type: Some(self.mime_type.clone()),
            size: Some(self.size.to_string()),
            url: None,
            path: Some(self.path.to_string_lossy().to_string()),
            extra: Some(extra),
        };

        // For filesystem, we'll use basic read permissions
        // In the future, this could be enhanced to map actual filesystem permissions
        let permissions = DocumentPermissions {
            public: false,
            users: vec![], // TODO: Map filesystem users/groups to users
            groups: vec![],
        };

        ConnectorEvent::DocumentCreated {
            sync_run_id,
            source_id,
            document_id: self.path.to_string_lossy().to_string(),
            content_id,
            metadata,
            permissions,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FilesystemSource {
    pub id: String,
    pub name: String,
    pub base_path: PathBuf,
    pub scan_interval_seconds: u64,
    pub file_extensions: Option<Vec<String>>,
    pub exclude_patterns: Option<Vec<String>>,
    pub max_file_size_bytes: Option<u64>,
}

impl FilesystemSource {
    pub fn should_include_file(&self, file_path: &PathBuf) -> bool {
        // Check file extension filter
        if let Some(extensions) = &self.file_extensions {
            if let Some(ext) = file_path.extension() {
                let ext_str = ext.to_string_lossy().to_lowercase();
                if !extensions.iter().any(|e| e.to_lowercase() == ext_str) {
                    return false;
                }
            } else {
                // No extension, skip if we have extension filters
                return false;
            }
        }

        // Check exclude patterns
        if let Some(patterns) = &self.exclude_patterns {
            let path_str = file_path.to_string_lossy();
            for pattern in patterns {
                if path_str.contains(pattern) {
                    return false;
                }
            }
        }

        true
    }
}

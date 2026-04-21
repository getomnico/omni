use omni_connector_sdk::{ConnectorEvent, DocumentMetadata, DocumentPermissions};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::PathBuf;
use std::time::SystemTime;
use time::OffsetDateTime;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FileSystemFile {
    pub path: PathBuf,
    pub name: String,
    pub size: u64,
    pub mime_type: String,
    pub created_time: Option<SystemTime>,
    pub modified_time: Option<SystemTime>,
    pub is_directory: bool,
    pub permissions: FileSystemPermissions,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FileSystemPermissions {
    pub readable: bool,
    pub writable: bool,
    pub executable: bool,
}

impl FileSystemFile {
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
            content_type: None,
            mime_type: Some(self.mime_type.clone()),
            size: Some(self.size.to_string()),
            url: None,
            path: Some(self.path.to_string_lossy().to_string()),
            extra: Some(extra),
        };

        // For filesystem, we'll use basic read permissions
        // In the future, this could be enhanced to map actual filesystem permissions
        let permissions = DocumentPermissions {
            public: true,
            users: vec![],
            groups: vec![],
        };

        ConnectorEvent::DocumentCreated {
            sync_run_id,
            source_id,
            document_id: self.path.to_string_lossy().to_string(),
            content_id,
            metadata,
            permissions,
            attributes: None,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FileSystemConfig {
    pub base_path: PathBuf,
    #[serde(default)]
    pub file_extensions: Option<Vec<String>>,
    #[serde(default)]
    pub exclude_patterns: Option<Vec<String>>,
    #[serde(default)]
    pub max_file_size_bytes: Option<u64>,
}

#[derive(Debug, Clone)]
pub struct FileSystemSource {
    pub name: String,
    pub base_path: PathBuf,
    pub file_extensions: Option<Vec<String>>,
    pub exclude_patterns: Option<Vec<String>>,
    pub max_file_size_bytes: Option<u64>,
}

impl FileSystemConfig {
    pub fn into_source(self, name: String) -> FileSystemSource {
        FileSystemSource {
            name,
            base_path: self.base_path,
            file_extensions: self.file_extensions,
            exclude_patterns: self.exclude_patterns,
            max_file_size_bytes: self.max_file_size_bytes,
        }
    }
}

impl FileSystemSource {
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

#[cfg(test)]
mod tests {
    use super::*;
    use omni_connector_sdk::ConnectorEvent;
    use std::path::PathBuf;
    use tempfile::TempDir;

    fn create_text_file(dir: &TempDir, name: &str, content: &str) -> PathBuf {
        let path = dir.path().join(name);
        std::fs::write(&path, content).unwrap();
        path
    }

    fn make_file(path: PathBuf, mime_type: &str) -> FileSystemFile {
        let metadata = std::fs::metadata(&path).unwrap();
        FileSystemFile {
            name: path.file_name().unwrap().to_string_lossy().to_string(),
            size: metadata.len(),
            mime_type: mime_type.to_string(),
            created_time: metadata.created().ok(),
            modified_time: metadata.modified().ok(),
            is_directory: false,
            permissions: FileSystemPermissions {
                readable: true,
                writable: true,
                executable: false,
            },
            path,
        }
    }

    #[test]
    fn test_connector_event_has_public_permissions() {
        let dir = TempDir::new().unwrap();
        let path = create_text_file(&dir, "test.txt", "content");
        let file = make_file(path, "text/plain");

        let event = file.to_connector_event(
            "sync-1".to_string(),
            "source-1".to_string(),
            "content-1".to_string(),
        );

        match event {
            ConnectorEvent::DocumentCreated { permissions, .. } => {
                assert!(permissions.public, "Permissions should be public");
            }
            _ => panic!("Expected DocumentCreated event"),
        }
    }
}

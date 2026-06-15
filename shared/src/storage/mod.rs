pub mod factory;
pub mod gc;
pub mod postgres;
pub mod s3;

use async_trait::async_trait;
use std::collections::HashMap;
use thiserror::Error;

#[derive(Debug, Error)]
pub enum StorageError {
    #[error("Storage error: {0}")]
    Backend(String),
    #[error("Content not found: {0}")]
    NotFound(String),
    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),
    #[error("Configuration error: {0}")]
    Config(String),
}

#[derive(Debug, Clone)]
pub struct ContentMetadata {
    pub content_type: Option<String>,
    pub size_bytes: i64,
    pub sha256_hash: String,
}

#[async_trait]
pub trait ObjectStorage: Send + Sync {
    /// Store content and return the content ID
    async fn store_content(
        &self,
        content: &[u8],
        prefix: Option<&str>,
    ) -> Result<String, StorageError>;

    /// Store content with optional content type and optional prefix for hierarchical organization
    async fn store_content_with_type(
        &self,
        content: &[u8],
        content_type: Option<&str>,
        prefix: Option<&str>,
    ) -> Result<String, StorageError>;

    /// Retrieve content by content ID
    async fn get_content(&self, content_id: &str) -> Result<Vec<u8>, StorageError>;

    /// Delete content by content ID
    async fn delete_content(&self, content_id: &str) -> Result<(), StorageError>;

    /// Store content as string (convenience method)
    async fn store_text(
        &self,
        content: &str,
        prefix: Option<&str>,
    ) -> Result<String, StorageError> {
        self.store_content_with_type(content.as_bytes(), Some("text/plain"), prefix)
            .await
    }

    /// Retrieve content as string (convenience method)
    async fn get_text(&self, content_id: &str) -> Result<String, StorageError> {
        let bytes = self.get_content(content_id).await?;
        Ok(String::from_utf8_lossy(&bytes).to_string())
    }

    /// Get content size without loading the full content
    async fn get_content_size(&self, content_id: &str) -> Result<i64, StorageError>;

    /// Batch fetch content for multiple content IDs efficiently
    async fn batch_get_text(
        &self,
        content_ids: Vec<String>,
    ) -> Result<HashMap<String, String>, StorageError>;

    /// Get content metadata without loading the content itself
    async fn get_content_metadata(&self, content_id: &str)
    -> Result<ContentMetadata, StorageError>;

    /// Find content by SHA256 hash (for deduplication)
    async fn find_by_hash(&self, sha256_hash: &str) -> Result<Option<String>, StorageError>;
}

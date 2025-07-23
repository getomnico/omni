use crate::utils::generate_ulid;
use sha2::{Digest, Sha256};
use sqlx::{PgPool, Row};
use std::collections::HashMap;

#[derive(Debug, Clone)]
pub struct ContentStorage {
    pool: PgPool,
}

#[derive(Debug, thiserror::Error)]
pub enum ContentStorageError {
    #[error("Database error: {0}")]
    Database(#[from] sqlx::Error),
    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),
    #[error("Content not found")]
    NotFound,
}

#[derive(Debug, Clone)]
pub struct ContentMetadata {
    pub content_type: Option<String>,
    pub size_bytes: i64,
    pub sha256_hash: String,
}

impl ContentStorage {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }

    /// Store content in content_blobs table using BYTEA (TOAST-backed) and return the content ID
    pub async fn store_content(&self, content: &[u8]) -> Result<String, ContentStorageError> {
        self.store_content_with_type(content, None).await
    }

    /// Store content with optional content type
    pub async fn store_content_with_type(
        &self,
        content: &[u8],
        content_type: Option<&str>,
    ) -> Result<String, ContentStorageError> {
        let content_id = generate_ulid();
        let size_bytes = content.len() as i64;

        // Generate SHA256 hash for potential deduplication
        let mut hasher = Sha256::new();
        hasher.update(content);
        let hash = format!("{:x}", hasher.finalize());

        sqlx::query(
            r#"
            INSERT INTO content_blobs (id, content, content_type, size_bytes, sha256_hash)
            VALUES ($1, $2, $3, $4, $5)
            "#,
        )
        .bind(&content_id)
        .bind(content)
        .bind(content_type)
        .bind(size_bytes)
        .bind(&hash)
        .execute(&self.pool)
        .await?;

        Ok(content_id)
    }

    /// Retrieve content from content_blobs table by content ID
    pub async fn get_content(&self, content_id: &str) -> Result<Vec<u8>, ContentStorageError> {
        let result: Option<Vec<u8>> =
            sqlx::query_scalar("SELECT content FROM content_blobs WHERE id = $1")
                .bind(content_id)
                .fetch_optional(&self.pool)
                .await?;

        result.ok_or(ContentStorageError::NotFound)
    }

    /// Delete content from content_blobs table by content ID
    pub async fn delete_content(&self, content_id: &str) -> Result<(), ContentStorageError> {
        let rows_affected = sqlx::query("DELETE FROM content_blobs WHERE id = $1")
            .bind(content_id)
            .execute(&self.pool)
            .await?
            .rows_affected();

        if rows_affected == 0 {
            return Err(ContentStorageError::NotFound);
        }

        Ok(())
    }

    /// Store content as string (convenience method)
    pub async fn store_text(&self, content: String) -> Result<String, ContentStorageError> {
        self.store_content_with_type(content.as_bytes(), Some("text/plain"))
            .await
    }

    /// Retrieve content as string (convenience method)
    pub async fn get_text(&self, content_id: &str) -> Result<String, ContentStorageError> {
        let bytes = self.get_content(content_id).await?;
        Ok(String::from_utf8_lossy(&bytes).to_string())
    }

    /// Get content size without loading the full content
    pub async fn get_content_size(&self, content_id: &str) -> Result<i64, ContentStorageError> {
        let size: Option<i64> =
            sqlx::query_scalar("SELECT size_bytes FROM content_blobs WHERE id = $1")
                .bind(content_id)
                .fetch_optional(&self.pool)
                .await?;

        size.ok_or(ContentStorageError::NotFound)
    }

    /// Batch fetch content for multiple content IDs efficiently
    pub async fn batch_get_text(
        &self,
        content_ids: Vec<String>,
    ) -> Result<HashMap<String, String>, ContentStorageError> {
        if content_ids.is_empty() {
            return Ok(HashMap::new());
        }

        let mut results = HashMap::new();

        // Process in chunks to avoid overwhelming the database
        const CHUNK_SIZE: usize = 50;
        for chunk in content_ids.chunks(CHUNK_SIZE) {
            let placeholders = (1..=chunk.len())
                .map(|i| format!("${}", i))
                .collect::<Vec<_>>()
                .join(",");

            let query = format!(
                "SELECT id, content FROM content_blobs WHERE id IN ({})",
                placeholders
            );

            let mut query_builder = sqlx::query(&query);
            for content_id in chunk {
                query_builder = query_builder.bind(content_id);
            }

            let rows = query_builder.fetch_all(&self.pool).await?;

            for row in rows {
                let id: String = row.get("id");
                let content: Vec<u8> = row.get("content");
                let content_str = String::from_utf8_lossy(&content).to_string();
                results.insert(id, content_str);
            }
        }

        Ok(results)
    }

    /// Get content metadata without loading the content itself
    pub async fn get_content_metadata(
        &self,
        content_id: &str,
    ) -> Result<ContentMetadata, ContentStorageError> {
        let result: Option<(Option<String>, i64, String)> = sqlx::query_as(
            "SELECT content_type, size_bytes, sha256_hash FROM content_blobs WHERE id = $1",
        )
        .bind(content_id)
        .fetch_optional(&self.pool)
        .await?;

        match result {
            Some((content_type, size_bytes, sha256_hash)) => Ok(ContentMetadata {
                content_type,
                size_bytes,
                sha256_hash,
            }),
            None => Err(ContentStorageError::NotFound),
        }
    }

    /// Find content by SHA256 hash (for deduplication)
    pub async fn find_by_hash(
        &self,
        sha256_hash: &str,
    ) -> Result<Option<String>, ContentStorageError> {
        let result: Option<String> =
            sqlx::query_scalar("SELECT id FROM content_blobs WHERE sha256_hash = $1 LIMIT 1")
                .bind(sha256_hash)
                .fetch_optional(&self.pool)
                .await?;

        Ok(result)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_environment::TestEnvironment;

    #[tokio::test]
    async fn test_content_storage() {
        let env = TestEnvironment::new().await.unwrap();
        let content_storage = ContentStorage::new(env.db_pool.pool().clone());

        // Test storing and retrieving content
        let test_content = b"Hello, World! This is a test content.";
        let content_id = content_storage.store_content(test_content).await.unwrap();

        let retrieved_content = content_storage.get_content(&content_id).await.unwrap();
        assert_eq!(test_content, retrieved_content.as_slice());

        // Test content size
        let size = content_storage.get_content_size(&content_id).await.unwrap();
        assert_eq!(size, test_content.len() as i64);

        // Test text convenience methods
        let text_content = "This is a text content";
        let text_content_id = content_storage.store_text(text_content).await.unwrap();

        let retrieved_text = content_storage.get_text(&text_content_id).await.unwrap();
        assert_eq!(text_content, retrieved_text);

        // Test content metadata
        let metadata = content_storage
            .get_content_metadata(&text_content_id)
            .await
            .unwrap();
        assert_eq!(metadata.content_type, Some("text/plain".to_string()));
        assert_eq!(metadata.size_bytes, text_content.len() as i64);

        // Test deletion
        content_storage.delete_content(&content_id).await.unwrap();

        // Verify content is deleted
        let result = content_storage.get_content(&content_id).await;
        assert!(matches!(result, Err(ContentStorageError::NotFound)));
    }

    #[tokio::test]
    async fn test_batch_get_text() {
        let env = TestEnvironment::new().await.unwrap();
        let content_storage = ContentStorage::new(env.db_pool.pool().clone());

        // Store multiple pieces of content
        let content1 = "First document content";
        let content2 = "Second document content";
        let content3 = "Third document content";

        let content_id1 = content_storage.store_text(content1).await.unwrap();
        let content_id2 = content_storage.store_text(content2).await.unwrap();
        let content_id3 = content_storage.store_text(content3).await.unwrap();

        // Batch fetch all content
        let content_ids = vec![
            content_id1.clone(),
            content_id2.clone(),
            content_id3.clone(),
        ];
        let results = content_storage.batch_get_text(content_ids).await.unwrap();

        // Verify all content is retrieved correctly
        assert_eq!(results.len(), 3);
        assert_eq!(results.get(&content_id1).unwrap(), content1);
        assert_eq!(results.get(&content_id2).unwrap(), content2);
        assert_eq!(results.get(&content_id3).unwrap(), content3);

        // Test with empty content IDs list
        let empty_results = content_storage.batch_get_text(vec![]).await.unwrap();
        assert!(empty_results.is_empty());

        // Test with non-existent content ID mixed in
        let fake_id = "01234567890123456789012345".to_string();
        let mixed_ids = vec![content_id1.clone(), fake_id.clone(), content_id2.clone()];
        let mixed_results = content_storage.batch_get_text(mixed_ids).await.unwrap();
        assert_eq!(mixed_results.len(), 2);
        assert_eq!(mixed_results.get(&content_id1).unwrap(), content1);
        assert_eq!(mixed_results.get(&content_id2).unwrap(), content2);
        assert!(!mixed_results.contains_key(&fake_id));
    }

    #[tokio::test]
    async fn test_content_deduplication() {
        let env = TestEnvironment::new().await.unwrap();
        let content_storage = ContentStorage::new(env.db_pool.pool().clone());

        // Store the same content twice
        let content = "This is duplicate content";
        let content_id1 = content_storage.store_text(content).await.unwrap();
        let content_id2 = content_storage.store_text(content).await.unwrap();

        // They should have different IDs but same hash
        assert_ne!(content_id1, content_id2);

        let metadata1 = content_storage
            .get_content_metadata(&content_id1)
            .await
            .unwrap();
        let metadata2 = content_storage
            .get_content_metadata(&content_id2)
            .await
            .unwrap();
        assert_eq!(metadata1.sha256_hash, metadata2.sha256_hash);

        // Test finding by hash
        let found_id = content_storage
            .find_by_hash(&metadata1.sha256_hash)
            .await
            .unwrap();
        assert!(found_id.is_some());
    }
}

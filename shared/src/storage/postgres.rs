use super::{ContentMetadata, ObjectStorage, StorageError};
use crate::utils::generate_ulid;
use async_trait::async_trait;
use sha2::{Digest, Sha256};
use sqlx::{PgPool, Row};
use std::collections::HashMap;

#[derive(Debug, Clone)]
pub struct PostgresStorage {
    pool: PgPool,
}

impl PostgresStorage {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }
}

#[async_trait]
impl ObjectStorage for PostgresStorage {
    async fn store_content(
        &self,
        content: &[u8],
        _prefix: Option<&str>,
    ) -> Result<String, StorageError> {
        self.store_content_with_type(content, None, _prefix).await
    }

    async fn store_content_with_type(
        &self,
        content: &[u8],
        content_type: Option<&str>,
        _prefix: Option<&str>,
    ) -> Result<String, StorageError> {
        let content_id = generate_ulid();
        let size_bytes = content.len() as i64;

        // Generate SHA256 hash for potential deduplication
        let mut hasher = Sha256::new();
        hasher.update(content);
        let hash = format!("{:x}", hasher.finalize());

        sqlx::query(
            r#"
            INSERT INTO content_blobs (id, content, content_type, size_bytes, sha256_hash, storage_backend)
            VALUES ($1, $2, $3, $4, $5, $6)
            "#,
        )
        .bind(&content_id)
        .bind(content)
        .bind(content_type)
        .bind(size_bytes)
        .bind(&hash)
        .bind("postgres")
        .execute(&self.pool)
        .await
        .map_err(|e| StorageError::Backend(format!("Failed to store content: {}", e)))?;

        Ok(content_id)
    }

    async fn get_content(&self, content_id: &str) -> Result<Vec<u8>, StorageError> {
        let result: Option<Vec<u8>> =
            sqlx::query_scalar("SELECT content FROM content_blobs WHERE id = $1")
                .bind(content_id)
                .fetch_optional(&self.pool)
                .await
                .map_err(|e| StorageError::Backend(format!("Failed to get content: {}", e)))?;

        result.ok_or_else(|| StorageError::NotFound(content_id.to_string()))
    }

    async fn delete_content(&self, content_id: &str) -> Result<(), StorageError> {
        let rows_affected = sqlx::query("DELETE FROM content_blobs WHERE id = $1")
            .bind(content_id)
            .execute(&self.pool)
            .await
            .map_err(|e| StorageError::Backend(format!("Failed to delete content: {}", e)))?
            .rows_affected();

        if rows_affected == 0 {
            return Err(StorageError::NotFound(content_id.to_string()));
        }

        Ok(())
    }

    async fn get_content_size(&self, content_id: &str) -> Result<i64, StorageError> {
        let size: Option<i64> =
            sqlx::query_scalar("SELECT size_bytes FROM content_blobs WHERE id = $1")
                .bind(content_id)
                .fetch_optional(&self.pool)
                .await
                .map_err(|e| StorageError::Backend(format!("Failed to get content size: {}", e)))?;

        size.ok_or_else(|| StorageError::NotFound(content_id.to_string()))
    }

    async fn batch_get_text(
        &self,
        content_ids: Vec<String>,
    ) -> Result<HashMap<String, String>, StorageError> {
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

            let rows = query_builder
                .fetch_all(&self.pool)
                .await
                .map_err(|e| StorageError::Backend(format!("Failed to batch get text: {}", e)))?;

            for row in rows {
                let id: String = row.get("id");
                let content: Vec<u8> = row.get("content");
                let content_str = String::from_utf8_lossy(&content).to_string();
                results.insert(id, content_str);
            }
        }

        Ok(results)
    }

    async fn get_content_metadata(
        &self,
        content_id: &str,
    ) -> Result<ContentMetadata, StorageError> {
        let result: Option<(Option<String>, i64, String)> = sqlx::query_as(
            "SELECT content_type, size_bytes, sha256_hash FROM content_blobs WHERE id = $1",
        )
        .bind(content_id)
        .fetch_optional(&self.pool)
        .await
        .map_err(|e| StorageError::Backend(format!("Failed to get content metadata: {}", e)))?;

        match result {
            Some((content_type, size_bytes, sha256_hash)) => Ok(ContentMetadata {
                content_type,
                size_bytes,
                sha256_hash,
            }),
            None => Err(StorageError::NotFound(content_id.to_string())),
        }
    }

    async fn find_by_hash(&self, sha256_hash: &str) -> Result<Option<String>, StorageError> {
        let result: Option<String> =
            sqlx::query_scalar("SELECT id FROM content_blobs WHERE sha256_hash = $1 LIMIT 1")
                .bind(sha256_hash)
                .fetch_optional(&self.pool)
                .await
                .map_err(|e| StorageError::Backend(format!("Failed to find by hash: {}", e)))?;

        Ok(result)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_environment::TestEnvironment;

    #[tokio::test]
    async fn test_postgres_storage() {
        let env = TestEnvironment::new().await.unwrap();
        let storage = PostgresStorage::new(env.db_pool.pool().clone());

        // Test storing and retrieving content
        let test_content = b"Hello, World! This is a test content.";
        let content_id = storage.store_content(test_content, None).await.unwrap();

        let retrieved_content = storage.get_content(&content_id).await.unwrap();
        assert_eq!(test_content, retrieved_content.as_slice());

        // Test content size
        let size = storage.get_content_size(&content_id).await.unwrap();
        assert_eq!(size, test_content.len() as i64);

        // Test text convenience methods
        let text_content = "This is a text content";
        let text_content_id = storage.store_text(text_content, None).await.unwrap();

        let retrieved_text = storage.get_text(&text_content_id).await.unwrap();
        assert_eq!(text_content, retrieved_text);

        // Test content metadata
        let metadata = storage
            .get_content_metadata(&text_content_id)
            .await
            .unwrap();
        assert_eq!(metadata.content_type, Some("text/plain".to_string()));
        assert_eq!(metadata.size_bytes, text_content.len() as i64);

        // Test deletion
        storage.delete_content(&content_id).await.unwrap();

        // Verify content is deleted
        let result = storage.get_content(&content_id).await;
        assert!(matches!(result, Err(StorageError::NotFound(_))));
    }

    #[tokio::test]
    async fn test_batch_get_text() {
        let env = TestEnvironment::new().await.unwrap();
        let storage = PostgresStorage::new(env.db_pool.pool().clone());

        // Store multiple pieces of content
        let content1 = "First document content";
        let content2 = "Second document content";
        let content3 = "Third document content";

        let content_id1 = storage.store_text(content1, None).await.unwrap();
        let content_id2 = storage.store_text(content2, None).await.unwrap();
        let content_id3 = storage.store_text(content3, None).await.unwrap();

        // Batch fetch all content
        let content_ids = vec![
            content_id1.clone(),
            content_id2.clone(),
            content_id3.clone(),
        ];
        let results = storage.batch_get_text(content_ids).await.unwrap();

        // Verify all content is retrieved correctly
        assert_eq!(results.len(), 3);
        assert_eq!(results.get(&content_id1).unwrap(), content1);
        assert_eq!(results.get(&content_id2).unwrap(), content2);
        assert_eq!(results.get(&content_id3).unwrap(), content3);

        // Test with empty content IDs list
        let empty_results = storage.batch_get_text(vec![]).await.unwrap();
        assert!(empty_results.is_empty());

        // Test with non-existent content ID mixed in
        let fake_id = "01234567890123456789012345".to_string();
        let mixed_ids = vec![content_id1.clone(), fake_id.clone(), content_id2.clone()];
        let mixed_results = storage.batch_get_text(mixed_ids).await.unwrap();
        assert_eq!(mixed_results.len(), 2);
        assert_eq!(mixed_results.get(&content_id1).unwrap(), content1);
        assert_eq!(mixed_results.get(&content_id2).unwrap(), content2);
        assert!(!mixed_results.contains_key(&fake_id));
    }

    #[tokio::test]
    async fn test_content_deduplication() {
        let env = TestEnvironment::new().await.unwrap();
        let storage = PostgresStorage::new(env.db_pool.pool().clone());

        // Store the same content twice
        let content = "This is duplicate content";
        let content_id1 = storage.store_text(content, None).await.unwrap();
        let content_id2 = storage.store_text(content, None).await.unwrap();

        // They should have different IDs but same hash
        assert_ne!(content_id1, content_id2);

        let metadata1 = storage.get_content_metadata(&content_id1).await.unwrap();
        let metadata2 = storage.get_content_metadata(&content_id2).await.unwrap();
        assert_eq!(metadata1.sha256_hash, metadata2.sha256_hash);

        // Test finding by hash
        let found_id = storage.find_by_hash(&metadata1.sha256_hash).await.unwrap();
        assert!(found_id.is_some());
    }
}

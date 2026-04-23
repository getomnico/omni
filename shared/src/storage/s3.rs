use super::{ContentMetadata, ObjectStorage, StorageError};
use crate::utils::generate_ulid;
use async_trait::async_trait;
use aws_sdk_s3::{error::SdkError, primitives::ByteStream, Client as S3Client};
use bytes::Bytes;
use futures_util::future;
use sha2::{Digest, Sha256};
use sqlx::{PgPool, Row};
use std::collections::HashMap;
use tracing::{debug, warn};

#[derive(Debug, Clone)]
pub struct S3Storage {
    client: S3Client,
    bucket: String,
    pool: PgPool,
}

impl S3Storage {
    pub async fn new(
        bucket: String,
        region: Option<String>,
        endpoint: Option<String>,
        pool: PgPool,
    ) -> Result<Self, StorageError> {
        let mut config_loader = aws_config::from_env();

        if let Some(region) = region {
            config_loader = config_loader.region(aws_config::Region::new(region));
        }

        let config = config_loader.load().await;

        // Support custom endpoint for LocalStack/MinIO
        if let Some(endpoint_url) = endpoint {
            let s3_config = aws_sdk_s3::config::Builder::from(&config)
                .endpoint_url(endpoint_url)
                .force_path_style(true)
                .build();

            return Ok(Self {
                client: S3Client::from_conf(s3_config),
                bucket,
                pool,
            });
        }

        Ok(Self {
            client: S3Client::new(&config),
            bucket,
            pool,
        })
    }

    fn generate_key(&self, prefix: Option<&str>) -> String {
        let ulid = generate_ulid();
        match prefix {
            Some(p) => {
                let trimmed = p.trim_matches('/');
                if trimmed.is_empty() {
                    ulid
                } else {
                    format!("{}/{}", trimmed, ulid)
                }
            }
            None => ulid,
        }
    }

    fn compute_hash(&self, content: &[u8]) -> String {
        let mut hasher = Sha256::new();
        hasher.update(content);
        format!("{:x}", hasher.finalize())
    }
}

#[async_trait]
impl ObjectStorage for S3Storage {
    async fn store_content(
        &self,
        content: &[u8],
        prefix: Option<&str>,
    ) -> Result<String, StorageError> {
        self.store_content_with_type(content, None, prefix).await
    }

    async fn store_content_with_type(
        &self,
        content: &[u8],
        content_type: Option<&str>,
        prefix: Option<&str>,
    ) -> Result<String, StorageError> {
        let size_bytes = content.len() as i64;
        let hash = self.compute_hash(content);

        // Content-address: reuse existing blob when hash matches. Skip both the
        // S3 upload and the metadata row when a blob for this hash already exists.
        // Under concurrent writes a small bounded number of duplicates may slip
        // through; they are cleaned up by the orphan GC.
        let existing: Option<String> =
            sqlx::query_scalar("SELECT id FROM content_blobs WHERE sha256_hash = $1 LIMIT 1")
                .bind(&hash)
                .fetch_optional(&self.pool)
                .await
                .map_err(|e| {
                    StorageError::Backend(format!("Failed to lookup content by hash: {}", e))
                })?;

        if let Some(id) = existing {
            return Ok(id);
        }

        let content_id = generate_ulid(); // Internal ID
        let storage_key = self.generate_key(prefix); // S3 key

        // 1. Upload to S3
        let byte_stream = ByteStream::from(Bytes::copy_from_slice(content));

        let mut put_request = self
            .client
            .put_object()
            .bucket(&self.bucket)
            .key(&storage_key)
            .body(byte_stream)
            .metadata("sha256", &hash)
            .metadata("size_bytes", size_bytes.to_string());

        if let Some(ct) = content_type {
            put_request = put_request.content_type(ct);
        }

        let res = put_request.send().await;

        if let Err(e) = res {
            match e {
                SdkError::ServiceError(err) => {
                    debug!("Service error when uploading to S3: {:?}", err);
                    debug!("Raw response: {:?}", err.raw())
                }
                _ => {
                    debug!("Error uploading to S3: {}", e);
                }
            }
        }

        debug!(
            "Stored content in S3: bucket={}, key={}",
            self.bucket, storage_key
        );

        // 2. Store metadata in Postgres
        sqlx::query(
            r#"
            INSERT INTO content_blobs (id, content, content_type, size_bytes, sha256_hash, storage_backend, storage_key)
            VALUES ($1, NULL, $2, $3, $4, 's3', $5)
            "#,
        )
        .bind(&content_id)
        .bind(content_type)
        .bind(size_bytes)
        .bind(&hash)
        .bind(&storage_key)
        .execute(&self.pool)
        .await
        .map_err(|e| StorageError::Backend(format!("Failed to store metadata in Postgres: {}", e)))?;

        debug!(
            "Stored metadata in Postgres: id={}, storage_key={}",
            content_id, storage_key
        );

        Ok(content_id)
    }

    async fn get_content(&self, content_id: &str) -> Result<Vec<u8>, StorageError> {
        // 1. Get storage_key from Postgres metadata
        let storage_key: Option<String> = sqlx::query_scalar(
            "SELECT storage_key FROM content_blobs WHERE id = $1 AND storage_backend = 's3'",
        )
        .bind(content_id)
        .fetch_optional(&self.pool)
        .await
        .map_err(|e| {
            StorageError::Backend(format!("Failed to fetch metadata from Postgres: {}", e))
        })?;

        let storage_key =
            storage_key.ok_or_else(|| StorageError::NotFound(content_id.to_string()))?;

        // 2. Fetch from S3 using storage_key
        let response = self
            .client
            .get_object()
            .bucket(&self.bucket)
            .key(&storage_key)
            .send()
            .await
            .map_err(|e| {
                if e.to_string().contains("NoSuchKey") {
                    StorageError::NotFound(content_id.to_string())
                } else {
                    StorageError::Backend(format!("Failed to get content from S3: {}", e))
                }
            })?;

        let bytes = response
            .body
            .collect()
            .await
            .map_err(|e| StorageError::Backend(format!("Failed to read S3 response body: {}", e)))?
            .into_bytes();

        Ok(bytes.to_vec())
    }

    async fn delete_content(&self, content_id: &str) -> Result<(), StorageError> {
        // 1. Get storage_key from Postgres metadata
        let storage_key: Option<String> = sqlx::query_scalar(
            "SELECT storage_key FROM content_blobs WHERE id = $1 AND storage_backend = 's3'",
        )
        .bind(content_id)
        .fetch_optional(&self.pool)
        .await
        .map_err(|e| {
            StorageError::Backend(format!("Failed to fetch metadata from Postgres: {}", e))
        })?;

        let storage_key =
            storage_key.ok_or_else(|| StorageError::NotFound(content_id.to_string()))?;

        // 2. Delete from S3
        self.client
            .delete_object()
            .bucket(&self.bucket)
            .key(&storage_key)
            .send()
            .await
            .map_err(|e| {
                StorageError::Backend(format!("Failed to delete content from S3: {}", e))
            })?;

        debug!(
            "Deleted content from S3: bucket={}, key={}",
            self.bucket, storage_key
        );

        // 3. Delete metadata from Postgres
        let rows_affected = sqlx::query("DELETE FROM content_blobs WHERE id = $1")
            .bind(content_id)
            .execute(&self.pool)
            .await
            .map_err(|e| {
                StorageError::Backend(format!("Failed to delete metadata from Postgres: {}", e))
            })?
            .rows_affected();

        if rows_affected == 0 {
            return Err(StorageError::NotFound(content_id.to_string()));
        }

        Ok(())
    }

    async fn get_content_size(&self, content_id: &str) -> Result<i64, StorageError> {
        // Fetch size from Postgres metadata (more efficient than S3 HEAD request)
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

        // 1. Fetch storage_keys from Postgres in batch
        let placeholders = (1..=content_ids.len())
            .map(|i| format!("${}", i))
            .collect::<Vec<_>>()
            .join(",");

        let query = format!(
            "SELECT id, storage_key FROM content_blobs WHERE id IN ({}) AND storage_backend = 's3'",
            placeholders
        );

        let mut query_builder = sqlx::query(&query);
        for content_id in &content_ids {
            query_builder = query_builder.bind(content_id);
        }

        let rows = query_builder
            .fetch_all(&self.pool)
            .await
            .map_err(|e| StorageError::Backend(format!("Failed to batch fetch metadata: {}", e)))?;

        let mut id_to_storage_key = HashMap::new();
        for row in rows {
            let id: String = row.get("id");
            let storage_key: String = row.get("storage_key");
            id_to_storage_key.insert(id, storage_key);
        }

        // 2. Fetch content from S3 concurrently
        let mut results = HashMap::new();
        let futures: Vec<_> = id_to_storage_key
            .into_iter()
            .map(|(content_id, storage_key)| {
                let client = self.client.clone();
                let bucket = self.bucket.clone();
                async move {
                    let response = client
                        .get_object()
                        .bucket(&bucket)
                        .key(&storage_key)
                        .send()
                        .await;

                    match response {
                        Ok(resp) => {
                            let bytes = resp.body.collect().await.ok()?.into_bytes();
                            let content_str = String::from_utf8_lossy(&bytes).to_string();
                            Some((content_id, content_str))
                        }
                        Err(e) => {
                            warn!("Failed to fetch content {} from S3: {}", content_id, e);
                            None
                        }
                    }
                }
            })
            .collect();

        let fetched = future::join_all(futures).await;

        for result in fetched {
            if let Some((id, content)) = result {
                results.insert(id, content);
            }
        }

        Ok(results)
    }

    async fn get_content_metadata(
        &self,
        content_id: &str,
    ) -> Result<ContentMetadata, StorageError> {
        // Fetch metadata from Postgres (more efficient than S3 HEAD request)
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
        // With Postgres metadata, we can efficiently query by hash
        let result: Option<String> =
            sqlx::query_scalar("SELECT id FROM content_blobs WHERE sha256_hash = $1 AND storage_backend = 's3' LIMIT 1")
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

    async fn create_test_storage() -> (S3Storage, TestEnvironment) {
        let env = TestEnvironment::new().await.unwrap();
        let bucket = "test-omni-content";
        let region = "us-east-1";
        let storage = S3Storage::new(
            bucket.to_string(),
            Some(region.to_string()),
            Some(env.s3_endpoint.clone()),
            env.db_pool.pool().clone(),
        )
        .await
        .unwrap();
        let _ = storage.client.create_bucket().bucket(bucket).send().await;
        (storage, env)
    }

    #[tokio::test]
    async fn test_s3_storage_basic_operations() {
        let (storage, _env) = create_test_storage().await;

        // Test storing and retrieving content without prefix
        let test_content = b"Hello, S3! This is a test content.";
        let content_id = storage.store_content(test_content, None).await.unwrap();

        let retrieved_content = storage.get_content(&content_id).await.unwrap();
        assert_eq!(test_content, retrieved_content.as_slice());

        // Test content size
        let size = storage.get_content_size(&content_id).await.unwrap();
        assert_eq!(size, test_content.len() as i64);

        // Test deletion
        storage.delete_content(&content_id).await.unwrap();

        // Verify content is deleted
        let result = storage.get_content(&content_id).await;
        assert!(matches!(result, Err(StorageError::NotFound(_))));
    }

    #[tokio::test]
    async fn test_s3_storage_with_content_type() {
        let (storage, _env) = create_test_storage().await;

        let text_content = "This is a text content";
        let content_id = storage.store_text(text_content, None).await.unwrap();

        let retrieved_text = storage.get_text(&content_id).await.unwrap();
        assert_eq!(text_content, retrieved_text);

        let metadata = storage.get_content_metadata(&content_id).await.unwrap();
        assert_eq!(metadata.content_type, Some("text/plain".to_string()));
        assert_eq!(metadata.size_bytes, text_content.len() as i64);

        // Cleanup
        storage.delete_content(&content_id).await.unwrap();
    }

    #[tokio::test]
    async fn test_s3_batch_get_text() {
        let (storage, _env) = create_test_storage().await;

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

        // Cleanup
        storage.delete_content(&content_id1).await.unwrap();
        storage.delete_content(&content_id2).await.unwrap();
        storage.delete_content(&content_id3).await.unwrap();
    }

    #[tokio::test]
    async fn test_s3_storage_with_prefix() {
        let (storage, _env) = create_test_storage().await;

        // Test with hierarchical prefix like {date}/{sync_run_id}
        let prefix = "2025-10/01ABC123DEF456";
        let test_content = b"Content with prefix";
        let content_id = storage
            .store_content(test_content, Some(prefix))
            .await
            .unwrap();

        // Verify content can be retrieved
        let retrieved_content = storage.get_content(&content_id).await.unwrap();
        assert_eq!(test_content, retrieved_content.as_slice());

        // Test with text content and prefix
        let text_content = "Text content with prefix";
        let text_prefix = "2025-10/01XYZ789ABC012";
        let text_content_id = storage
            .store_text(text_content, Some(text_prefix))
            .await
            .unwrap();

        let retrieved_text = storage.get_text(&text_content_id).await.unwrap();
        assert_eq!(text_content, retrieved_text);

        // Test with empty prefix (should behave like None)
        let empty_prefix = "";
        let content_id_empty = storage
            .store_text("No prefix", Some(empty_prefix))
            .await
            .unwrap();
        let retrieved_empty = storage.get_text(&content_id_empty).await.unwrap();
        assert_eq!("No prefix", retrieved_empty);

        // Test with prefix containing leading/trailing slashes
        let messy_prefix = "/2025-10/sync-run-123/";
        let content_id_messy = storage
            .store_text("Messy prefix", Some(messy_prefix))
            .await
            .unwrap();
        let retrieved_messy = storage.get_text(&content_id_messy).await.unwrap();
        assert_eq!("Messy prefix", retrieved_messy);

        // Cleanup
        storage.delete_content(&content_id).await.unwrap();
        storage.delete_content(&text_content_id).await.unwrap();
        storage.delete_content(&content_id_empty).await.unwrap();
        storage.delete_content(&content_id_messy).await.unwrap();
    }
}

use super::{postgres::PostgresStorage, s3::S3Storage, ObjectStorage, StorageError};
use sqlx::PgPool;
use std::sync::Arc;
use tracing::info;

#[derive(Debug, Clone, PartialEq)]
pub enum StorageBackend {
    Postgres,
    S3,
}

impl StorageBackend {
    pub fn from_env() -> Self {
        let backend = std::env::var("STORAGE_BACKEND")
            .unwrap_or_else(|_| "postgres".to_string())
            .to_lowercase();

        match backend.as_str() {
            "s3" => StorageBackend::S3,
            "postgres" | _ => StorageBackend::Postgres,
        }
    }
}

pub struct StorageFactory;

impl StorageFactory {
    /// Create storage backend from environment variables
    ///
    /// Environment variables:
    /// - STORAGE_BACKEND: "postgres" (default) or "s3"
    /// - S3_BUCKET: Required if STORAGE_BACKEND=s3
    /// - S3_REGION: Optional, defaults to AWS default behavior
    /// - S3_ENDPOINT: Optional, for LocalStack/MinIO
    pub async fn from_env(pool: PgPool) -> Result<Arc<dyn ObjectStorage>, StorageError> {
        let backend = StorageBackend::from_env();

        match backend {
            StorageBackend::Postgres => {
                info!("Initializing PostgreSQL storage backend");
                Ok(Arc::new(PostgresStorage::new(pool)))
            }
            StorageBackend::S3 => {
                info!("Initializing S3 storage backend");
                let bucket = std::env::var("S3_BUCKET").map_err(|_| {
                    StorageError::Config(
                        "S3_BUCKET environment variable is required when STORAGE_BACKEND=s3"
                            .to_string(),
                    )
                })?;

                let region = std::env::var("S3_REGION").ok();
                let endpoint = std::env::var("S3_ENDPOINT").ok();

                if let Some(ref endpoint_url) = endpoint {
                    info!(
                        "Using S3 storage with custom endpoint: bucket={}, endpoint={}",
                        bucket, endpoint_url
                    );
                } else {
                    info!("Using S3 storage: bucket={}", bucket);
                }

                let s3_storage = S3Storage::new(bucket, region, endpoint, pool).await?;
                Ok(Arc::new(s3_storage))
            }
        }
    }

    /// Create storage backend with explicit configuration
    pub async fn create(
        backend: StorageBackend,
        pool: Option<PgPool>,
        bucket: Option<String>,
        region: Option<String>,
        endpoint: Option<String>,
    ) -> Result<Arc<dyn ObjectStorage>, StorageError> {
        match backend {
            StorageBackend::Postgres => {
                let pool = pool.ok_or_else(|| {
                    StorageError::Config(
                        "PgPool is required for PostgreSQL storage backend".to_string(),
                    )
                })?;
                Ok(Arc::new(PostgresStorage::new(pool)))
            }
            StorageBackend::S3 => {
                let bucket = bucket.ok_or_else(|| {
                    StorageError::Config(
                        "Bucket name is required for S3 storage backend".to_string(),
                    )
                })?;
                let pool = pool.ok_or_else(|| {
                    StorageError::Config(
                        "PgPool is required for S3 storage backend (for metadata storage)"
                            .to_string(),
                    )
                })?;
                let s3_storage = S3Storage::new(bucket, region, endpoint, pool).await?;
                Ok(Arc::new(s3_storage))
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_environment::TestEnvironment;

    #[tokio::test]
    async fn test_factory_postgres() {
        // Set environment to use Postgres
        std::env::set_var("STORAGE_BACKEND", "postgres");

        let env = TestEnvironment::new().await.unwrap();
        let storage = StorageFactory::from_env(env.db_pool.pool().clone())
            .await
            .unwrap();

        // Test basic operations
        let content = b"test content";
        let content_id = storage.store_content(content, None).await.unwrap();
        let retrieved = storage.get_content(&content_id).await.unwrap();
        assert_eq!(content, retrieved.as_slice());

        // Cleanup
        storage.delete_content(&content_id).await.unwrap();
    }

    #[tokio::test]
    async fn test_factory_s3_without_config() {
        // Set environment to use S3 without required config
        std::env::set_var("STORAGE_BACKEND", "s3");
        std::env::remove_var("S3_BUCKET");

        let env = TestEnvironment::new().await.unwrap();
        let result = StorageFactory::from_env(env.db_pool.pool().clone()).await;

        // Should fail because S3_BUCKET is not set
        assert!(matches!(result, Err(StorageError::Config(_))));

        // Cleanup
        std::env::set_var("STORAGE_BACKEND", "postgres");
    }

    #[tokio::test]
    async fn test_factory_explicit_creation() {
        let env = TestEnvironment::new().await.unwrap();

        // Create PostgreSQL storage explicitly
        let storage = StorageFactory::create(
            StorageBackend::Postgres,
            Some(env.db_pool.pool().clone()),
            None,
            None,
            None,
        )
        .await
        .unwrap();

        // Test basic operations
        let content = b"test content";
        let content_id = storage.store_content(content, None).await.unwrap();
        let retrieved = storage.get_content(&content_id).await.unwrap();
        assert_eq!(content, retrieved.as_slice());

        // Cleanup
        storage.delete_content(&content_id).await.unwrap();
    }
}

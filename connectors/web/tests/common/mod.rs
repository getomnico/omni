use anyhow::Result;
use redis::Client as RedisClient;
use shared::db::repositories::SyncRunRepository;
use shared::test_environment::TestEnvironment;
use sqlx::PgPool;

/// Test fixture for Web connector integration tests
pub struct WebConnectorTestFixture {
    pub test_env: TestEnvironment,
}

impl WebConnectorTestFixture {
    /// Create a new test fixture with all dependencies
    pub async fn new() -> Result<Self> {
        let test_env = TestEnvironment::new().await?;
        Ok(Self { test_env })
    }

    /// Get the database pool
    pub fn pool(&self) -> &PgPool {
        self.test_env.db_pool.pool()
    }

    /// Get the Redis client
    pub fn redis_client(&self) -> RedisClient {
        self.test_env.redis_client.clone()
    }

    /// Get the SyncRunRepository for testing sync operations
    pub fn sync_run_repo(&self) -> SyncRunRepository {
        SyncRunRepository::new(self.pool())
    }

    /// Create a test user and return the user ID
    pub async fn create_test_user(&self, email: &str) -> Result<String> {
        let user_id = shared::utils::generate_ulid();

        sqlx::query(
            "INSERT INTO users (id, email, full_name, role, password_hash) VALUES ($1, $2, $3, $4, $5)",
        )
        .bind(&user_id)
        .bind(email)
        .bind("Test User")
        .bind("admin")
        .bind("hashed_password")
        .execute(self.pool())
        .await?;

        Ok(user_id)
    }

    /// Create a test web source and return the source ID
    pub async fn create_test_source(&self, name: &str, user_id: &str) -> Result<String> {
        let source_id = shared::utils::generate_ulid();

        sqlx::query(
            "INSERT INTO sources (id, name, source_type, is_active, created_by, config) VALUES ($1, $2, $3, $4, $5, $6)",
        )
        .bind(&source_id)
        .bind(name)
        .bind(shared::models::SourceType::Web)
        .bind(true)
        .bind(user_id)
        .bind(serde_json::json!({"root_url": "https://example.com"}))
        .execute(self.pool())
        .await?;

        Ok(source_id)
    }

    /// Create a sync run with specific status and timing
    pub async fn create_sync_run(
        &self,
        source_id: &str,
        sync_type: shared::models::SyncType,
        status: shared::models::SyncStatus,
        started_at: sqlx::types::time::OffsetDateTime,
    ) -> Result<String> {
        let sync_run_id = shared::utils::generate_ulid();

        sqlx::query(
            "INSERT INTO sync_runs (id, source_id, sync_type, status, started_at) VALUES ($1, $2, $3, $4, $5)",
        )
        .bind(&sync_run_id)
        .bind(source_id)
        .bind(sync_type)
        .bind(status)
        .bind(started_at)
        .execute(self.pool())
        .await?;

        Ok(sync_run_id)
    }

    /// Create a completed sync run
    pub async fn create_completed_sync_run(
        &self,
        source_id: &str,
        sync_type: shared::models::SyncType,
        completed_at: sqlx::types::time::OffsetDateTime,
        documents_processed: i32,
        documents_updated: i32,
    ) -> Result<String> {
        let sync_run_id = shared::utils::generate_ulid();

        sqlx::query(
            "INSERT INTO sync_runs (id, source_id, sync_type, status, completed_at, documents_processed, documents_updated)
             VALUES ($1, $2, $3, $4, $5, $6, $7)",
        )
        .bind(&sync_run_id)
        .bind(source_id)
        .bind(sync_type)
        .bind(shared::models::SyncStatus::Completed)
        .bind(completed_at)
        .bind(documents_processed)
        .bind(documents_updated)
        .execute(self.pool())
        .await?;

        Ok(sync_run_id)
    }
}

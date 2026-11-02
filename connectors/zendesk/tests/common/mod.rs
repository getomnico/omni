use std::sync::Arc;
use std::sync::atomic::AtomicBool;

use anyhow::Result;
use omni_connector_manager::{
    AppState, config::ConnectorManagerConfig, create_app, remote_mcp::gateway::RemoteMcpGateway,
    sync_manager::SyncManager,
};
use omni_connector_sdk::{SdkClient, SyncContext};
use serde_json::{Value as JsonValue, json};
use shared::db::repositories::SyncRunRepository;
use shared::models::{SourceType, SyncRun, SyncStatus, SyncType};
use shared::storage::ObjectStorage;
use shared::storage::postgres::PostgresStorage;
use shared::test_environment::TestEnvironment;
use shared::{DatabaseConfig, RedisConfig};
use sqlx::{PgPool, Row};
use tokio::net::TcpListener;

/// Real connector-manager over testcontainer Postgres/Redis, so events, content and checkpoints
/// go through the same SDK endpoints as in production.
pub struct Fixture {
    pub test_env: TestEnvironment,
    pub sdk_client: SdkClient,
    _server: tokio::task::JoinHandle<()>,
}

impl Fixture {
    pub async fn new() -> Result<Self> {
        // SAFETY: set before any other thread reads them, same as the other connector harnesses.
        unsafe {
            std::env::set_var(
                "ENCRYPTION_KEY",
                "test_master_key_that_is_long_enough_32_chars",
            );
            std::env::set_var("ENCRYPTION_SALT", "test_salt_16_chars");
            std::env::set_var("CONNECTOR_HOST_NAME", "localhost");
            std::env::set_var("PORT", "0");
        }
        let test_env = TestEnvironment::new().await?;
        let config = ConnectorManagerConfig {
            database: DatabaseConfig {
                database_url: "postgresql://test:test@localhost/test".to_string(),
                max_connections: 5,
                acquire_timeout_seconds: 3,
                require_ssl: false,
            },
            redis: RedisConfig {
                redis_url: "redis://localhost".to_string(),
            },
            port: 0,
            max_concurrent_syncs: 10,
            max_concurrent_syncs_per_type: 3,
            scheduler_interval_seconds: 30,
            stale_sync_timeout_minutes: 10,
            extraction_concurrency: 2,
            extraction_retry_after_seconds: 30,
            sync_backoff_base_seconds: 30,
            sync_backoff_max_seconds: 3600,
            sync_max_consecutive_failures: 10,
        };
        let redis_client = redis::Client::open(config.redis.redis_url.clone())?;
        let sync_manager = Arc::new(SyncManager::new(
            &test_env.db_pool,
            config.clone(),
            redis_client.clone(),
        ));
        let content_storage: Arc<dyn ObjectStorage> =
            Arc::new(PostgresStorage::new(test_env.db_pool.pool().clone()));
        let remote_mcp_gateway = Arc::new(RemoteMcpGateway::new(
            test_env.db_pool.clone(),
            redis_client.clone(),
        )?);
        let app = create_app(AppState {
            db_pool: test_env.db_pool.clone(),
            redis_client,
            extraction_semaphore: Arc::new(tokio::sync::Semaphore::new(
                config.extraction_concurrency,
            )),
            config,
            sync_manager,
            remote_mcp_gateway,
            content_storage,
        });
        let listener = TcpListener::bind("127.0.0.1:0").await?;
        let addr = listener.local_addr()?;
        let server = tokio::spawn(async move {
            axum::serve(listener, app).await.ok();
        });
        Ok(Self {
            test_env,
            sdk_client: SdkClient::new(&format!("http://{addr}")),
            _server: server,
        })
    }

    pub fn pool(&self) -> &PgPool {
        self.test_env.db_pool.pool()
    }

    /// Seeds a Zendesk source with a run of the given mode; returns `(source_id, sync_run_id)`.
    pub async fn seed_run(&self, sync_type: SyncType) -> Result<(String, String)> {
        let user_id = shared::utils::generate_ulid();
        sqlx::query(
            "INSERT INTO users (id, email, full_name, role, password_hash) VALUES ($1, $2, $3, $4, $5)",
        )
        .bind(&user_id)
        .bind(format!("{user_id}@example.com"))
        .bind("Test Admin")
        .bind("admin")
        .bind("hashed_password")
        .execute(self.pool())
        .await?;

        let source_id = shared::utils::generate_ulid();
        sqlx::query(
            "INSERT INTO sources (id, name, source_type, is_active, created_by, config) VALUES ($1, $2, $3, $4, $5, $6)",
        )
        .bind(&source_id)
        .bind("Zendesk")
        .bind(SourceType::Zendesk)
        .bind(true)
        .bind(&user_id)
        .bind(json!({"subdomain": "example"}))
        .execute(self.pool())
        .await?;
        let sync_run_id = self.new_run(&source_id, sync_type).await?;
        Ok((source_id, sync_run_id))
    }

    pub async fn new_run(&self, source_id: &str, sync_type: SyncType) -> Result<String> {
        let sync_run_id = shared::utils::generate_ulid();
        sqlx::query(
            "INSERT INTO sync_runs (id, source_id, sync_type, status, started_at) VALUES ($1, $2, $3, $4, NOW())",
        )
        .bind(&sync_run_id)
        .bind(source_id)
        .bind(sync_type)
        .bind(SyncStatus::Running)
        .execute(self.pool())
        .await?;
        Ok(sync_run_id)
    }

    pub fn context(
        &self,
        source_id: &str,
        sync_run_id: &str,
        sync_type: SyncType,
        is_resume: bool,
        cancelled: Arc<AtomicBool>,
    ) -> SyncContext {
        SyncContext::new_with_resume(
            self.sdk_client.clone(),
            sync_run_id.to_string(),
            source_id.to_string(),
            SourceType::Zendesk,
            sync_type,
            is_resume,
            cancelled,
        )
    }

    pub async fn events(&self, source_id: &str) -> Result<Vec<JsonValue>> {
        let rows = sqlx::query(
            "SELECT payload FROM connector_events_queue WHERE source_id = $1 ORDER BY created_at, id",
        )
        .bind(source_id)
        .fetch_all(self.pool())
        .await?;
        Ok(rows
            .iter()
            .map(|row| row.get::<JsonValue, _>("payload"))
            .collect())
    }

    pub async fn sync_run(&self, sync_run_id: &str) -> Result<SyncRun> {
        SyncRunRepository::new(self.pool())
            .find_by_id(sync_run_id)
            .await
            .map_err(|e| anyhow::anyhow!("{e}"))?
            .ok_or_else(|| anyhow::anyhow!("sync run {sync_run_id} not found"))
    }

    pub async fn content(&self, content_id: &str) -> Result<String> {
        PostgresStorage::new(self.pool().clone())
            .get_text(content_id)
            .await
            .map_err(|e| anyhow::anyhow!("{e}"))
    }
}

use anyhow::Result;
use omni_connector_manager::{
    config::ConnectorManagerConfig, create_app as create_cm_app,
    sync_manager::SyncManager as CMSyncManager, AppState as CMAppState,
};
use shared::db::repositories::SyncRunRepository;
use shared::storage::postgres::PostgresStorage;
use shared::test_environment::TestEnvironment;
use shared::{DatabaseConfig, RedisConfig, SdkClient};
use sqlx::PgPool;
use std::collections::HashMap;
use std::sync::Arc;
use tokio::net::TcpListener;

pub struct SlackConnectorTestFixture {
    pub test_env: TestEnvironment,
    pub sdk_client: SdkClient,
    _server_handle: tokio::task::JoinHandle<()>,
}

impl SlackConnectorTestFixture {
    pub async fn new() -> Result<Self> {
        rustls::crypto::ring::default_provider()
            .install_default()
            .ok();

        std::env::set_var(
            "ENCRYPTION_KEY",
            "test_master_key_that_is_long_enough_32_chars",
        );
        std::env::set_var("ENCRYPTION_SALT", "test_salt_16_chars");

        let test_env = TestEnvironment::new().await?;

        let cm_config = ConnectorManagerConfig {
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
            connector_urls: HashMap::new(),
            max_concurrent_syncs: 10,
            max_concurrent_syncs_per_type: 3,
            scheduler_interval_seconds: 30,
            stale_sync_timeout_minutes: 10,
        };

        let cm_sync_manager = Arc::new(CMSyncManager::new(&test_env.db_pool, cm_config.clone()));

        let content_storage: Arc<dyn shared::ObjectStorage> =
            Arc::new(PostgresStorage::new(test_env.db_pool.pool().clone()));

        let cm_state = CMAppState {
            db_pool: test_env.db_pool.clone(),
            config: cm_config,
            sync_manager: cm_sync_manager,
            content_storage,
        };

        let cm_app = create_cm_app(cm_state);
        let listener = TcpListener::bind("127.0.0.1:0").await?;
        let addr = listener.local_addr()?;

        let server_handle = tokio::spawn(async move {
            axum::serve(listener, cm_app).await.ok();
        });

        let sdk_client = SdkClient::new(&format!("http://{}", addr));

        tokio::time::sleep(tokio::time::Duration::from_millis(50)).await;

        Ok(Self {
            test_env,
            sdk_client,
            _server_handle: server_handle,
        })
    }

    pub fn pool(&self) -> &PgPool {
        self.test_env.db_pool.pool()
    }

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

    pub async fn create_test_source(&self, name: &str, user_id: &str) -> Result<String> {
        let source_id = shared::utils::generate_ulid();

        sqlx::query(
            "INSERT INTO sources (id, name, source_type, is_active, created_by, config) VALUES ($1, $2, $3, $4, $5, $6)",
        )
        .bind(&source_id)
        .bind(name)
        .bind(shared::models::SourceType::Slack)
        .bind(true)
        .bind(user_id)
        .bind(serde_json::json!({}))
        .execute(self.pool())
        .await?;

        Ok(source_id)
    }

    pub async fn create_test_credentials(&self, source_id: &str, bot_token: &str) -> Result<()> {
        let cred_id = shared::utils::generate_ulid();
        let credentials = serde_json::json!({ "bot_token": bot_token });

        let creds_repo = shared::ServiceCredentialsRepo::new(self.pool().clone())?;
        let creds = shared::models::ServiceCredentials {
            id: cred_id,
            source_id: source_id.to_string(),
            provider: shared::models::ServiceProvider::Slack,
            auth_type: shared::models::AuthType::BotToken,
            principal_email: None,
            credentials,
            config: serde_json::json!({}),
            expires_at: None,
            last_validated_at: None,
            created_at: time::OffsetDateTime::now_utc(),
            updated_at: time::OffsetDateTime::now_utc(),
        };
        creds_repo.create(creds).await?;

        Ok(())
    }

    pub async fn create_sync_run(&self, source_id: &str) -> Result<String> {
        let sync_run_id = shared::utils::generate_ulid();

        sqlx::query(
            "INSERT INTO sync_runs (id, source_id, sync_type, status, started_at) VALUES ($1, $2, $3, $4, NOW())",
        )
        .bind(&sync_run_id)
        .bind(source_id)
        .bind(shared::models::SyncType::Full)
        .bind(shared::models::SyncStatus::Running)
        .execute(self.pool())
        .await?;

        Ok(sync_run_id)
    }

    pub async fn get_queued_events(&self, source_id: &str) -> Result<Vec<serde_json::Value>> {
        let rows = sqlx::query(
            "SELECT payload FROM connector_events_queue WHERE source_id = $1 ORDER BY created_at",
        )
        .bind(source_id)
        .fetch_all(self.pool())
        .await?;

        let events: Vec<serde_json::Value> = rows
            .iter()
            .map(|row| {
                use sqlx::Row;
                row.get::<serde_json::Value, _>("payload")
            })
            .collect();

        Ok(events)
    }

    pub async fn get_sync_run(&self, sync_run_id: &str) -> Result<Option<shared::models::SyncRun>> {
        let sync_run_repo = SyncRunRepository::new(self.pool());
        sync_run_repo
            .find_by_id(sync_run_id)
            .await
            .map_err(|e| anyhow::anyhow!("{}", e))
    }

    pub async fn get_connector_state(&self, source_id: &str) -> Result<Option<serde_json::Value>> {
        let row = sqlx::query("SELECT connector_state FROM sources WHERE id = $1")
            .bind(source_id)
            .fetch_optional(self.pool())
            .await?;

        Ok(row
            .map(|r| {
                use sqlx::Row;
                r.get::<Option<serde_json::Value>, _>("connector_state")
            })
            .flatten())
    }
}

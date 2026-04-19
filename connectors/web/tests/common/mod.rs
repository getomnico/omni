use anyhow::Result;
use std::future::Future;
use std::sync::atomic::AtomicBool;
use std::time::{Duration, Instant};

use omni_connector_manager::{
    config::ConnectorManagerConfig, create_app as create_cm_app,
    sync_manager::SyncManager as CMSyncManager, AppState as CMAppState,
};
use omni_connector_sdk::SyncContext;
use omni_web_connector::config::WebSourceConfig;
use omni_web_connector::models::WebConnectorState;
use omni_web_connector::sync::{PageSource, SyncManager};
use shared::db::repositories::SyncRunRepository;
use shared::models::SyncType;
use shared::storage::postgres::PostgresStorage;
use shared::test_environment::TestEnvironment;
use shared::{DatabaseConfig, RedisConfig, SdkClient};
use sqlx::PgPool;
use std::sync::Arc;
use tokio::net::TcpListener;

/// Test fixture for Web connector integration tests
pub struct WebConnectorTestFixture {
    pub test_env: TestEnvironment,
    pub sdk_client: SdkClient,
    _server_handle: tokio::task::JoinHandle<()>,
}

impl WebConnectorTestFixture {
    /// Create a new test fixture with all dependencies including connector-manager
    pub async fn new() -> Result<Self> {
        std::env::set_var(
            "ENCRYPTION_KEY",
            "test_master_key_that_is_long_enough_32_chars",
        );
        std::env::set_var("ENCRYPTION_SALT", "test_salt_16_chars");
        std::env::set_var("CONNECTOR_HOST_NAME", "localhost");
        std::env::set_var("PORT", "0");

        let test_env = TestEnvironment::new().await?;

        // Create connector-manager config for testing
        // The database config here won't be used since we pass db_pool directly
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
            port: 0, // Not used since we bind to a random port
            max_concurrent_syncs: 10,
            max_concurrent_syncs_per_type: 3,
            scheduler_interval_seconds: 30,
            stale_sync_timeout_minutes: 10,
        };

        // Create connector-manager sync manager
        let redis_client = redis::Client::open(cm_config.redis.redis_url.clone())?;

        let cm_sync_manager = Arc::new(CMSyncManager::new(
            &test_env.db_pool,
            cm_config.clone(),
            redis_client.clone(),
        ));

        // Create content storage
        let content_storage: Arc<dyn shared::ObjectStorage> =
            Arc::new(PostgresStorage::new(test_env.db_pool.pool().clone()));

        // Create connector-manager app state
        let cm_state = CMAppState {
            db_pool: test_env.db_pool.clone(),
            redis_client,
            config: cm_config,
            sync_manager: cm_sync_manager,
            content_storage,
        };

        // Create connector-manager app
        let cm_app = create_cm_app(cm_state);

        // Bind to a random available port
        let listener = TcpListener::bind("127.0.0.1:0").await?;
        let addr = listener.local_addr()?;

        // Spawn the server in a background task
        let server_handle = tokio::spawn(async move {
            axum::serve(listener, cm_app).await.ok();
        });

        // Create SDK client pointing to the test server
        let sdk_client = SdkClient::new(&format!("http://{}", addr));

        // Wait a moment for the server to be ready
        tokio::time::sleep(tokio::time::Duration::from_millis(50)).await;

        Ok(Self {
            test_env,
            sdk_client,
            _server_handle: server_handle,
        })
    }

    /// Get the database pool
    pub fn pool(&self) -> &PgPool {
        self.test_env.db_pool.pool()
    }

    /// Get the SyncRunRepository for testing sync operations
    pub fn sync_run_repo(&self) -> SyncRunRepository {
        SyncRunRepository::new(self.pool())
    }

    /// Create a SyncManager with the SDK client for integration testing
    pub fn create_sync_manager(&self, page_source: Arc<dyn PageSource>) -> SyncManager {
        SyncManager::with_page_source(self.sdk_client.clone(), page_source)
    }

    /// Load the persisted `WebConnectorState` for a source (returns `None`
    /// when no state has been saved yet, e.g. before the first sync).
    pub async fn load_web_state(&self, source_id: &str) -> Result<Option<WebConnectorState>> {
        match self.get_connector_state(source_id).await? {
            Some(value) => Ok(Some(serde_json::from_value(value)?)),
            None => Ok(None),
        }
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
    pub async fn create_test_source(
        &self,
        name: &str,
        user_id: &str,
        root_url: &str,
    ) -> Result<String> {
        let source_id = shared::utils::generate_ulid();

        sqlx::query(
            "INSERT INTO sources (id, name, source_type, is_active, created_by, config) VALUES ($1, $2, $3, $4, $5, $6)",
        )
        .bind(&source_id)
        .bind(name)
        .bind(shared::models::SourceType::Web)
        .bind(true)
        .bind(user_id)
        .bind(serde_json::json!({"root_url": root_url, "max_depth": 2, "max_pages": 100}))
        .execute(self.pool())
        .await?;

        Ok(source_id)
    }

    /// Create a sync run for testing
    pub async fn create_sync_run(&self, source_id: &str) -> Result<String> {
        let sync_run_id = shared::utils::generate_ulid();

        sqlx::query(
            "INSERT INTO sync_runs (id, source_id, sync_type, status, started_at) VALUES ($1, $2, $3, $4, NOW())",
        )
        .bind(&sync_run_id)
        .bind(source_id)
        .bind(SyncType::Full)
        .bind(shared::models::SyncStatus::Running)
        .execute(self.pool())
        .await?;

        Ok(sync_run_id)
    }

    /// Build the config + `SyncContext` pair that a sync expects — the same
    /// wiring the SDK performs in its `/sync` handler, pulled into tests so
    /// they can drive `SyncManager::run_sync` directly without going through
    /// HTTP.
    pub async fn build_sync_context(
        &self,
        sync_run_id: &str,
        source_id: &str,
    ) -> Result<(WebSourceConfig, Option<WebConnectorState>, SyncContext)> {
        let source = self
            .sdk_client
            .get_source(source_id)
            .await
            .map_err(|e| anyhow::anyhow!("{}", e))?;
        let config = WebSourceConfig::from_json(&source.config)?;
        let prior_state = match source.connector_state {
            Some(value) => Some(serde_json::from_value::<WebConnectorState>(value)?),
            None => None,
        };
        let ctx = SyncContext::new(
            self.sdk_client.clone(),
            sync_run_id.to_string(),
            source_id.to_string(),
            source.source_type,
            SyncType::Full,
            Arc::new(AtomicBool::new(false)),
        );
        Ok((config, prior_state, ctx))
    }

    /// Create an incremental sync run for testing
    pub async fn create_incremental_sync_run(&self, source_id: &str) -> Result<String> {
        let sync_run_id = shared::utils::generate_ulid();

        sqlx::query(
            "INSERT INTO sync_runs (id, source_id, sync_type, status, started_at) VALUES ($1, $2, $3, $4, NOW())",
        )
        .bind(&sync_run_id)
        .bind(source_id)
        .bind(SyncType::Incremental)
        .bind(shared::models::SyncStatus::Running)
        .execute(self.pool())
        .await?;

        Ok(sync_run_id)
    }

    /// Get connector state for a source via SDK
    pub async fn get_connector_state(&self, source_id: &str) -> Result<Option<serde_json::Value>> {
        self.sdk_client
            .get_connector_state(source_id)
            .await
            .map_err(|e| anyhow::anyhow!("{}", e))
    }

    /// Get queued events for a source
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

    /// Get the sync run status
    pub async fn get_sync_run(&self, sync_run_id: &str) -> Result<Option<shared::models::SyncRun>> {
        let sync_run_repo = SyncRunRepository::new(self.pool());
        sync_run_repo
            .find_by_id(sync_run_id)
            .await
            .map_err(|e| anyhow::anyhow!("{}", e))
    }
}

pub async fn poll_until<F, Fut>(f: F, timeout: Duration) -> Result<()>
where
    F: Fn() -> Fut,
    Fut: Future<Output = Result<bool>>,
{
    let deadline = Instant::now() + timeout;
    while Instant::now() < deadline {
        if f().await? {
            return Ok(());
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    Err(anyhow::anyhow!("Timed out waiting for condition"))
}

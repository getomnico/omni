use anyhow::Result;
use axum::{routing::post, Json, Router};
use omni_connector_manager::{config::ConnectorManagerConfig, create_app, AppState};
use omni_google_connector::sync::SyncManager;
use redis::{AsyncCommands, Client as RedisClient};
use shared::db::repositories::SyncRunRepository;
use shared::models::{ConnectorManifest, SourceType, SyncResponse, SyncType};
use shared::storage::postgres::PostgresStorage;
use shared::test_environment::TestEnvironment;
use shared::{ObjectStorage, SdkClient};
use sqlx::PgPool;
use std::sync::Arc;
use tokio::net::TcpListener;

const TEST_SOURCE_ID: &str = "01JGF7V3E0Y2R1X8P5Q7W9T4N7";

/// Test fixture for Google connector integration tests
pub struct GoogleConnectorTestFixture {
    pub test_env: TestEnvironment,
    pub sync_manager: Arc<SyncManager>,
    _cm_server_handle: tokio::task::JoinHandle<()>,
    _mock_connector_handle: tokio::task::JoinHandle<()>,
}

impl GoogleConnectorTestFixture {
    /// Create a new test fixture with all dependencies
    pub async fn new() -> Result<Self> {
        std::env::set_var(
            "ENCRYPTION_KEY",
            "test_master_key_that_is_long_enough_32_chars",
        );
        std::env::set_var("ENCRYPTION_SALT", "test_salt_16_chars");

        let test_env = TestEnvironment::new().await?;

        // Update the seeded source to be a Google Drive source
        seed_google_drive_source(test_env.db_pool.pool()).await?;

        // Start a mock Google connector that accepts /sync requests
        let mock_connector_app = Router::new().route(
            "/sync",
            post(|| async {
                Json(SyncResponse {
                    status: "accepted".to_string(),
                    message: None,
                })
            }),
        );
        let mock_connector_listener = TcpListener::bind("127.0.0.1:0").await?;
        let mock_connector_port = mock_connector_listener.local_addr()?.port();
        let mock_connector_url = format!("http://127.0.0.1:{}", mock_connector_port);
        let mock_connector_handle = tokio::spawn(async move {
            axum::serve(mock_connector_listener, mock_connector_app)
                .await
                .unwrap();
        });

        // Set up connector-manager server
        let config = ConnectorManagerConfig {
            database: test_env.database_config(),
            redis: test_env.redis_config(),
            port: 0,
            max_concurrent_syncs: 2,
            max_concurrent_syncs_per_type: 3,
            scheduler_interval_seconds: 600,
            stale_sync_timeout_minutes: 1,
        };

        let content_storage: Arc<dyn ObjectStorage> =
            Arc::new(PostgresStorage::new(test_env.db_pool.pool().clone()));

        let redis_client = redis::Client::open(config.redis.redis_url.clone())?;

        // Register mock connector in Redis so connector manager can find it
        let manifest = ConnectorManifest {
            name: "google".to_string(),
            display_name: "Google".to_string(),
            version: "1.0.0".to_string(),
            sync_modes: vec![SyncType::Full],
            connector_id: "google".to_string(),
            connector_url: mock_connector_url,
            source_types: vec![SourceType::GoogleDrive, SourceType::Gmail],
            description: None,
            actions: vec![],
            search_operators: vec![],
            read_only: false,
            extra_schema: None,
            attributes_schema: None,
            mcp_enabled: false,
            resources: vec![],
            prompts: vec![],
        };
        let manifest_json = serde_json::to_string(&manifest)?;
        let mut redis_conn = redis_client.get_multiplexed_async_connection().await?;
        let _: () = redis_conn
            .set_ex("connector:manifest:google", &manifest_json, 600)
            .await?;

        let cm_sync_manager = Arc::new(omni_connector_manager::sync_manager::SyncManager::new(
            &test_env.db_pool,
            config.clone(),
            redis_client.clone(),
        ));

        let app_state = AppState {
            db_pool: test_env.db_pool.clone(),
            redis_client,
            config,
            sync_manager: cm_sync_manager,
            content_storage,
        };

        let app = create_app(app_state);

        let listener = TcpListener::bind("127.0.0.1:0").await?;
        let port = listener.local_addr()?.port();
        let cm_server_handle = tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });
        tokio::time::sleep(tokio::time::Duration::from_millis(50)).await;

        let sdk_client = SdkClient::new(&format!("http://127.0.0.1:{}", port));
        let admin_client = Arc::new(omni_google_connector::admin::AdminClient::new());

        let sync_manager = Arc::new(SyncManager::new(
            test_env.redis_client.clone(),
            test_env.mock_ai_server.base_url.clone(),
            admin_client,
            sdk_client,
            None,
        ));

        Ok(Self {
            test_env,
            sync_manager,
            _cm_server_handle: cm_server_handle,
            _mock_connector_handle: mock_connector_handle,
        })
    }

    pub fn pool(&self) -> &PgPool {
        self.test_env.db_pool.pool()
    }

    pub fn redis_client(&self) -> RedisClient {
        self.test_env.redis_client.clone()
    }

    pub fn sync_run_repo(&self) -> SyncRunRepository {
        SyncRunRepository::new(self.pool())
    }

    pub fn source_id(&self) -> &str {
        TEST_SOURCE_ID
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

    pub async fn create_test_source(
        &self,
        name: &str,
        source_type: shared::models::SourceType,
        user_id: &str,
    ) -> Result<String> {
        let source_id = shared::utils::generate_ulid();

        sqlx::query(
            "INSERT INTO sources (id, name, source_type, is_active, created_by) VALUES ($1, $2, $3, $4, $5)",
        )
        .bind(&source_id)
        .bind(name)
        .bind(source_type)
        .bind(true)
        .bind(user_id)
        .execute(self.pool())
        .await?;

        Ok(source_id)
    }

    pub async fn create_sync_run(
        &self,
        source_id: &str,
        sync_type: SyncType,
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

    pub async fn create_completed_sync_run(
        &self,
        source_id: &str,
        sync_type: SyncType,
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

async fn seed_google_drive_source(pool: &PgPool) -> Result<()> {
    sqlx::query(
        r#"
        UPDATE sources SET source_type = 'google_drive', name = 'Test Google Drive Source'
        WHERE id = $1
        "#,
    )
    .bind(TEST_SOURCE_ID)
    .execute(pool)
    .await?;

    Ok(())
}

pub mod mock_connector;

use anyhow::Result;
use mock_connector::MockConnector;
use omni_connector_manager::{
    config::ConnectorManagerConfig, create_app, sync_manager::SyncManager, AppState,
};
use redis::{AsyncCommands, Client as RedisClient};
use shared::models::{ConnectorManifest, SourceType, SyncType};
use shared::storage::postgres::PostgresStorage;
use shared::test_environment::TestEnvironment;
use shared::ObjectStorage;
use std::sync::Arc;

pub const TEST_SOURCE_ID: &str = "01JGF7V3E0Y2R1X8P5Q7W9T4N7";

pub struct TestFixture {
    pub state: AppState,
    pub app: axum::Router,
    pub mock_connector: MockConnector,
    #[allow(dead_code)]
    test_env: TestEnvironment,
}

pub async fn setup_test_fixture() -> Result<TestFixture> {
    std::env::set_var(
        "ENCRYPTION_KEY",
        "test_master_key_that_is_long_enough_32_chars",
    );
    std::env::set_var("ENCRYPTION_SALT", "test_salt_16_chars");

    let test_env = TestEnvironment::new().await?;
    let mock_connector = MockConnector::start().await?;

    let config = ConnectorManagerConfig {
        database: test_env.database_config(),
        redis: test_env.redis_config(),
        port: 0,
        max_concurrent_syncs: 2,
        max_concurrent_syncs_per_type: 3,
        scheduler_interval_seconds: 600,
        stale_sync_timeout_minutes: 1,
    };

    let redis_client = RedisClient::open(config.redis.redis_url.clone())?;

    // Register mock connector in Redis so the manager can find it
    let manifest = ConnectorManifest {
        name: "filesystem".to_string(),
        display_name: "Filesystem".to_string(),
        version: "1.0.0".to_string(),
        sync_modes: vec![SyncType::Full],
        connector_id: "filesystem".to_string(),
        connector_url: mock_connector.base_url.clone(),
        source_types: vec![SourceType::LocalFiles],
        description: None,
        actions: vec![],
        search_operators: vec![],
        read_only: false,
        extra_schema: None,
        attributes_schema: None,
        mcp_enabled: false,
        prompts: vec![],
        resources: vec![],
    };
    let manifest_json = serde_json::to_string(&manifest)?;
    let mut redis_conn = redis_client.get_multiplexed_async_connection().await?;
    let _: () = redis_conn
        .set_ex("connector:manifest:filesystem", &manifest_json, 600)
        .await?;

    let content_storage: Arc<dyn ObjectStorage> =
        Arc::new(PostgresStorage::new(test_env.db_pool.pool().clone()));

    let sync_manager = Arc::new(SyncManager::new(
        &test_env.db_pool,
        config.clone(),
        redis_client.clone(),
    ));

    let app_state = AppState {
        db_pool: test_env.db_pool.clone(),
        redis_client,
        config,
        sync_manager,
        content_storage,
    };

    let app = create_app(app_state.clone());

    Ok(TestFixture {
        state: app_state,
        app,
        mock_connector,
        test_env,
    })
}

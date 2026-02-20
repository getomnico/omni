pub mod mock_atlassian;

use anyhow::Result;
use mock_atlassian::MockAtlassianApi;
use omni_connector_manager::{config::ConnectorManagerConfig, create_app, AppState};
use shared::db::repositories::service_credentials::ServiceCredentialsRepo;
use shared::models::{AuthType, ServiceCredentials, ServiceProvider, SourceType};
use shared::storage::postgres::PostgresStorage;
use shared::test_environment::TestEnvironment;
use shared::{ObjectStorage, SdkClient};
use sqlx::PgPool;
use std::collections::HashMap;
use std::sync::Arc;
use time::OffsetDateTime;
use tokio::net::TcpListener;

const TEST_CRED_ID: &str = "01JTEST0ATLASSIAN0CRED0001";
pub const TEST_BASE_URL: &str = "https://test-company.atlassian.net";
pub const TEST_USER_EMAIL: &str = "test@example.com";
pub const TEST_API_TOKEN: &str = "test-api-token";

pub struct TestFixture {
    pub mock_api: Arc<MockAtlassianApi>,
    pub sdk_client: SdkClient,
    pub state: AppState,
    pub pool: PgPool,
    _test_env: TestEnvironment,
    _server_handle: tokio::task::JoinHandle<()>,
}

pub async fn setup_test_fixture(source_type: SourceType) -> Result<TestFixture> {
    std::env::set_var(
        "ENCRYPTION_KEY",
        "test_master_key_that_is_long_enough_32_chars",
    );
    std::env::set_var("ENCRYPTION_SALT", "test_salt_16_chars");

    let test_env = TestEnvironment::new().await?;

    // Seed Atlassian source and credentials
    seed_atlassian_source(test_env.db_pool.pool(), source_type).await?;

    let mut connector_urls = HashMap::new();
    connector_urls.insert(
        shared::models::SourceType::Confluence,
        "http://127.0.0.1:1/atlassian".to_string(),
    );
    connector_urls.insert(
        shared::models::SourceType::Jira,
        "http://127.0.0.1:1/atlassian".to_string(),
    );

    let config = ConnectorManagerConfig {
        database: test_env.database_config(),
        redis: test_env.redis_config(),
        port: 0,
        connector_urls,
        max_concurrent_syncs: 2,
        max_concurrent_syncs_per_type: 3,
        scheduler_interval_seconds: 600,
        stale_sync_timeout_minutes: 1,
    };

    let content_storage: Arc<dyn ObjectStorage> =
        Arc::new(PostgresStorage::new(test_env.db_pool.pool().clone()));

    let sync_manager = Arc::new(omni_connector_manager::sync_manager::SyncManager::new(
        &test_env.db_pool,
        config.clone(),
    ));

    let app_state = AppState {
        db_pool: test_env.db_pool.clone(),
        config,
        sync_manager,
        content_storage,
    };

    let app = create_app(app_state.clone());

    // Start the connector-manager on a random port
    let listener = TcpListener::bind("127.0.0.1:0").await?;
    let port = listener.local_addr()?.port();
    let server_handle = tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    tokio::time::sleep(tokio::time::Duration::from_millis(50)).await;

    let sdk_client = SdkClient::new(&format!("http://127.0.0.1:{}", port));

    let mock_api = Arc::new(MockAtlassianApi::new());

    let pool = test_env.db_pool.pool().clone();

    Ok(TestFixture {
        mock_api,
        sdk_client,
        state: app_state,
        pool,
        _test_env: test_env,
        _server_handle: server_handle,
    })
}

async fn seed_atlassian_source(pool: &PgPool, source_type: SourceType) -> Result<()> {
    let source_type_str = match source_type {
        SourceType::Confluence => "confluence",
        SourceType::Jira => "jira",
        _ => return Err(anyhow::anyhow!("Unsupported source type for test")),
    };

    // Update the existing test source to be an Atlassian source
    sqlx::query(
        r#"
        UPDATE sources SET source_type = $1, name = $2
        WHERE id = $3
        "#,
    )
    .bind(source_type_str)
    .bind(format!("Test {} Source", source_type_str))
    .bind("01JGF7V3E0Y2R1X8P5Q7W9T4N7")
    .execute(pool)
    .await?;

    let creds_repo = ServiceCredentialsRepo::new(pool.clone())?;
    let creds = ServiceCredentials {
        id: TEST_CRED_ID.to_string(),
        source_id: "01JGF7V3E0Y2R1X8P5Q7W9T4N7".to_string(),
        provider: ServiceProvider::Atlassian,
        auth_type: AuthType::ApiKey,
        principal_email: Some(TEST_USER_EMAIL.to_string()),
        credentials: serde_json::json!({"api_token": TEST_API_TOKEN}),
        config: serde_json::json!({"base_url": TEST_BASE_URL}),
        expires_at: None,
        last_validated_at: None,
        created_at: OffsetDateTime::now_utc(),
        updated_at: OffsetDateTime::now_utc(),
    };
    creds_repo.create(creds).await?;

    Ok(())
}

/// Count events in the connector_events_queue
pub async fn count_queued_events(pool: &PgPool) -> Result<i64> {
    let row: (i64,) = sqlx::query_as("SELECT COUNT(*) FROM connector_events_queue")
        .fetch_one(pool)
        .await?;
    Ok(row.0)
}

/// Get all queued events with their payloads
pub async fn get_queued_events(pool: &PgPool) -> Result<Vec<serde_json::Value>> {
    let rows = sqlx::query_as::<_, (serde_json::Value,)>(
        "SELECT payload FROM connector_events_queue ORDER BY created_at",
    )
    .fetch_all(pool)
    .await?;
    Ok(rows.into_iter().map(|(p,)| p).collect())
}

/// Get queued events filtered by event type
pub async fn get_queued_events_by_type(
    pool: &PgPool,
    event_type: &str,
) -> Result<Vec<serde_json::Value>> {
    let rows = sqlx::query_as::<_, (serde_json::Value,)>(
        "SELECT payload FROM connector_events_queue WHERE event_type = $1 ORDER BY created_at",
    )
    .bind(event_type)
    .fetch_all(pool)
    .await?;
    Ok(rows.into_iter().map(|(p,)| p).collect())
}

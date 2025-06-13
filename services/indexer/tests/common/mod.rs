use anyhow::Result;
use clio_indexer::{create_app, AppState};
use redis::Client as RedisClient;
use shared::db::pool::DatabasePool;
use shared::db::repositories::DocumentRepository;
use shared::models::Document;
use sqlx::PgPool;
use std::env;
use tokio::time::{sleep, timeout, Duration};
use uuid::Uuid;

/// Test fixture that automatically cleans up the test database on drop
pub struct TestFixture {
    pub state: AppState,
    pub app: axum::Router,
    db_name: String,
}

impl TestFixture {
    /// Get a reference to the app router
    pub fn app(&self) -> &axum::Router {
        &self.app
    }

    /// Manually cleanup the test database (automatically called on drop)
    pub async fn cleanup(&self) -> Result<()> {
        let base_url = env::var("DATABASE_URL")
            .unwrap_or_else(|_| "postgresql://clio:clio_password@localhost:5432/clio".to_string());
        cleanup_test_database_by_name(&base_url, &self.db_name).await
    }
}

impl Drop for TestFixture {
    fn drop(&mut self) {
        let db_name = self.db_name.clone();
        let base_url = env::var("DATABASE_URL")
            .unwrap_or_else(|_| "postgresql://clio:clio_password@localhost:5432/clio".to_string());

        // Best effort cleanup - if we're in a panic, skip cleanup
        if std::thread::panicking() {
            eprintln!(
                "Warning: Test panicked, database {} may not be cleaned up",
                db_name
            );
            return;
        }

        // Try to spawn cleanup task if we have a tokio runtime available
        if let Ok(handle) = tokio::runtime::Handle::try_current() {
            let cleanup_db_name = db_name.clone();
            let cleanup_base_url = base_url.clone();
            // Spawn and detach the cleanup task - it will run in the background
            let _ = handle.spawn(async move {
                tokio::time::sleep(std::time::Duration::from_millis(10)).await; // Give test time to finish
                if let Err(e) =
                    cleanup_test_database_by_name(&cleanup_base_url, &cleanup_db_name).await
                {
                    eprintln!(
                        "Warning: Failed to cleanup test database {}: {:?}",
                        cleanup_db_name, e
                    );
                }
            });
        } else {
            eprintln!(
                "Warning: No tokio runtime available, database {} may not be cleaned up",
                db_name
            );
        }
    }
}

/// Setup test app with automatic cleanup via TestFixture
pub async fn setup_test_fixture() -> Result<TestFixture> {
    tracing_subscriber::fmt::try_init().ok();
    let (db_pool, db_name) = setup_test_database_internal().await?;
    let redis_client = setup_test_redis().await?;

    let app_state = AppState {
        db_pool,
        redis_client,
    };

    let app = create_app(app_state.clone());

    Ok(TestFixture {
        state: app_state,
        app,
        db_name,
    })
}

/// Internal function that returns both pool and database name
async fn setup_test_database_internal() -> Result<(DatabasePool, String)> {
    dotenvy::dotenv().ok();

    let base_url = env::var("DATABASE_URL")
        .unwrap_or_else(|_| "postgresql://clio:clio_password@localhost:5432/clio".to_string());

    let test_db_name = format!("clio_test_{}", Uuid::new_v4().to_string().replace("-", ""));

    let (base_url_without_db, _) = base_url.rsplit_once('/').unwrap();
    let admin_url = format!("{}/postgres", base_url_without_db);

    let admin_pool = PgPool::connect(&admin_url).await?;
    sqlx::query(&format!("CREATE DATABASE {}", test_db_name))
        .execute(&admin_pool)
        .await?;

    let test_db_url = format!("{}/{}", base_url_without_db, test_db_name);
    env::set_var("DATABASE_URL", &test_db_url);

    let db_pool = DatabasePool::new(&test_db_url).await?;

    sqlx::migrate!("./migrations").run(db_pool.pool()).await?;

    seed_test_data(db_pool.pool()).await?;

    Ok((db_pool, test_db_name))
}

pub async fn setup_test_redis() -> Result<RedisClient> {
    let redis_url = env::var("REDIS_URL").unwrap_or_else(|_| "redis://localhost:6379".to_string());

    let client = RedisClient::open(redis_url)?;

    let mut conn = client.get_multiplexed_async_connection().await?;
    redis::cmd("FLUSHDB")
        .query_async::<String>(&mut conn)
        .await?;

    Ok(client)
}

async fn seed_test_data(pool: &PgPool) -> Result<()> {
    eprintln!("Seeding test data");
    let user_id = "01JGF7V3E0Y2R1X8P5Q7W9T4N6";
    let source_id = "01JGF7V3E0Y2R1X8P5Q7W9T4N7";

    sqlx::query(
        r#"
        INSERT INTO users (id, email, password_hash, created_at, updated_at)
        VALUES ($1, 'test@example.com', 'hash', NOW(), NOW())
        "#,
    )
    .bind(user_id)
    .execute(pool)
    .await?;

    sqlx::query(
        r#"
        INSERT INTO sources (id, name, source_type, config, created_by, created_at, updated_at)
        VALUES ($1, 'Test Source', 'test', '{}', $2, NOW(), NOW())
        "#,
    )
    .bind(source_id)
    .bind(user_id)
    .execute(pool)
    .await?;

    Ok(())
}

/// Cleanup test database by name - used by Drop impl
async fn cleanup_test_database_by_name(base_url: &str, db_name: &str) -> Result<()> {
    // Only cleanup test databases
    if !db_name.starts_with("clio_test_") {
        return Ok(());
    }

    let (base_url_without_db, _) = base_url.rsplit_once('/').unwrap();
    let admin_url = format!("{}/postgres", base_url_without_db);
    let admin_pool = PgPool::connect(&admin_url).await?;

    sqlx::query(&format!("DROP DATABASE IF EXISTS {} WITH (FORCE)", db_name))
        .execute(&admin_pool)
        .await?;

    Ok(())
}

pub mod fixtures {
    use clio_indexer::{CreateDocumentRequest, UpdateDocumentRequest};
    use serde_json::json;

    pub fn create_document_request() -> CreateDocumentRequest {
        CreateDocumentRequest {
            source_id: "01JGF7V3E0Y2R1X8P5Q7W9T4N7".to_string(),
            external_id: "ext_123".to_string(),
            title: "Test Document".to_string(),
            content: "This is test content for integration testing.".to_string(),
            metadata: json!({
                "author": "Test Author",
                "type": "document"
            }),
            permissions: json!({
                "users": ["user1", "user2"],
                "groups": ["group1"]
            }),
        }
    }

    pub fn update_document_request() -> UpdateDocumentRequest {
        UpdateDocumentRequest {
            title: Some("Updated Test Document".to_string()),
            content: Some("This is updated content.".to_string()),
            metadata: Some(json!({
                "author": "Updated Author",
                "type": "document",
                "version": 2
            })),
            permissions: Some(json!({
                "users": ["user1", "user2", "user3"],
                "groups": ["group1", "group2"]
            })),
        }
    }
}

/// Wait for a document to exist in the database with polling and timeout
pub async fn wait_for_document_exists(
    repo: &DocumentRepository,
    source_id: &str,
    doc_id: &str,
    timeout_duration: Duration,
) -> Result<Document, String> {
    let result = timeout(timeout_duration, async {
        loop {
            if let Ok(Some(doc)) = repo.find_by_external_id(source_id, doc_id).await {
                return doc;
            }
            sleep(Duration::from_millis(10)).await;
        }
    })
    .await;

    match result {
        Ok(doc) => Ok(doc),
        Err(_) => Err(format!(
            "Document {}:{} not found within timeout",
            source_id, doc_id
        )),
    }
}

/// Wait for a document to be deleted from the database with polling and timeout
pub async fn wait_for_document_deleted(
    repo: &DocumentRepository,
    source_id: &str,
    doc_id: &str,
    timeout_duration: Duration,
) -> Result<(), String> {
    let result = timeout(timeout_duration, async {
        loop {
            if let Ok(None) = repo.find_by_external_id(source_id, doc_id).await {
                return;
            }
            sleep(Duration::from_millis(10)).await;
        }
    })
    .await;

    match result {
        Ok(_) => Ok(()),
        Err(_) => Err(format!(
            "Document {}:{} was not deleted within timeout",
            source_id, doc_id
        )),
    }
}

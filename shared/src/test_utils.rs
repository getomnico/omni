use crate::db::pool::DatabasePool;
use crate::db::repositories::DocumentRepository;
use crate::models::Document;
use anyhow::Result;
use redis::Client as RedisClient;
use serde_json::json;
use sqlx::PgPool;
use std::env;
use tokio::time::{sleep, timeout, Duration};
use ulid::Ulid;
use uuid::Uuid;

/// Base test fixture for database and Redis setup
pub struct BaseTestFixture {
    pub db_pool: DatabasePool,
    pub redis_client: RedisClient,
    db_name: String,
}

impl BaseTestFixture {
    /// Create a new test fixture with isolated database and Redis
    pub async fn new() -> Result<Self> {
        tracing_subscriber::fmt::try_init().ok();
        let (db_pool, db_name) = setup_test_database_internal().await?;
        let redis_client = setup_test_redis().await?;

        Ok(Self {
            db_pool,
            redis_client,
            db_name,
        })
    }

    /// Get the database pool
    pub fn db_pool(&self) -> &DatabasePool {
        &self.db_pool
    }

    /// Get the Redis client
    pub fn redis_client(&self) -> &RedisClient {
        &self.redis_client
    }

    /// Get database config for tests
    pub fn database_config(&self) -> crate::config::DatabaseConfig {
        crate::config::DatabaseConfig {
            database_url: format!(
                "postgresql://clio:clio_password@localhost:5432/{}",
                &self.db_name
            ),
            max_connections: 5,
            acquire_timeout_seconds: 30,
        }
    }

    /// Get Redis config for tests  
    pub fn redis_config(&self) -> crate::config::RedisConfig {
        crate::config::RedisConfig {
            redis_url: env::var("REDIS_URL")
                .unwrap_or_else(|_| "redis://localhost:6379/1".to_string()),
        }
    }

    /// Manually cleanup the test database (automatically called on drop)
    pub async fn cleanup(&self) -> Result<()> {
        let base_url = env::var("DATABASE_URL")
            .unwrap_or_else(|_| "postgresql://clio:clio_password@localhost:5432/clio".to_string());
        cleanup_test_database_by_name(&base_url, &self.db_name).await
    }
}

impl Drop for BaseTestFixture {
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

    // Run migrations - look for migrations in the services that include this
    if let Ok(migrations_dir) = env::var("TEST_MIGRATIONS_DIR") {
        sqlx::migrate::Migrator::new(std::path::Path::new(&migrations_dir))
            .await?
            .run(db_pool.pool())
            .await?;
    }

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
        ON CONFLICT (id) DO NOTHING
        "#,
    )
    .bind(user_id)
    .execute(pool)
    .await?;

    sqlx::query(
        r#"
        INSERT INTO sources (id, name, source_type, config, created_by, created_at, updated_at)
        VALUES ($1, 'Test Source', 'test', '{}', $2, NOW(), NOW())
        ON CONFLICT (id) DO NOTHING
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

/// Create test documents with various content for search testing
pub async fn create_test_documents(pool: &PgPool) -> Result<Vec<String>> {
    let source_id = "01JGF7V3E0Y2R1X8P5Q7W9T4N7";
    let mut doc_ids = Vec::new();
    let content_storage = crate::ContentStorage::new(pool.clone());

    // Document 1: Technical documentation
    let doc_1_id = Ulid::new().to_string();
    let content_1 = "This is a comprehensive guide to Rust programming language. It covers memory safety, ownership, borrowing, and lifetimes. Rust is a systems programming language that runs blazingly fast, prevents segfaults, and guarantees thread safety.".to_string();
    let content_1_id = content_storage.store_text(content_1).await?;
    sqlx::query(
        r#"
        INSERT INTO documents (id, source_id, external_id, title, content_id, metadata, permissions, created_at, updated_at)
        VALUES ($1, $2, $3, $4, $5, $6, $7, NOW(), NOW())
        "#,
    )
    .bind(&doc_1_id)
    .bind(source_id)
    .bind("tech_doc_1")
    .bind("Rust Programming Guide")
    .bind(&content_1_id)
    .bind(json!({"type": "documentation", "category": "programming"}))
    .bind(json!({"users": ["user1"], "groups": ["engineers"]}))
    .execute(pool)
    .await?;
    doc_ids.push(doc_1_id);

    // Document 2: Meeting notes
    let doc_2_id = Ulid::new().to_string();
    let content_2 = "Attendees discussed the roadmap for Q4. Key priorities include improving search functionality, implementing semantic search, and optimizing database queries. The team will focus on PostgreSQL performance and Redis caching.".to_string();
    let content_2_id = content_storage.store_text(content_2).await?;
    sqlx::query(
        r#"
        INSERT INTO documents (id, source_id, external_id, title, content_id, metadata, permissions, created_at, updated_at)
        VALUES ($1, $2, $3, $4, $5, $6, $7, NOW(), NOW())
        "#,
    )
    .bind(&doc_2_id)
    .bind(source_id)
    .bind("meeting_notes_1")
    .bind("Q4 Planning Meeting")
    .bind(&content_2_id)
    .bind(json!({"type": "meeting", "date": "2024-01-15"}))
    .bind(json!({"users": ["user1", "user2"], "groups": ["team"]}))
    .execute(pool)
    .await?;
    doc_ids.push(doc_2_id);

    // Document 3: Project specifications
    let doc_3_id = Ulid::new().to_string();
    let content_3 = "The search engine combines full-text search with vector embeddings. It uses PostgreSQL with pgvector extension for similarity search. The architecture includes caching layer with Redis and supports multiple search modes: fulltext, semantic, and hybrid.".to_string();
    let content_3_id = content_storage.store_text(content_3).await?;
    sqlx::query(
        r#"
        INSERT INTO documents (id, source_id, external_id, title, content_id, metadata, permissions, created_at, updated_at)
        VALUES ($1, $2, $3, $4, $5, $6, $7, NOW(), NOW())
        "#,
    )
    .bind(&doc_3_id)
    .bind(source_id)
    .bind("project_spec_1")
    .bind("Search Engine Architecture")
    .bind(&content_3_id)
    .bind(json!({"type": "specification", "project": "clio"}))
    .bind(json!({"users": ["user1"], "groups": ["architects"]}))
    .execute(pool)
    .await?;
    doc_ids.push(doc_3_id);

    // Document 4: API documentation
    let doc_4_id = Ulid::new().to_string();
    let content_4 = "The API provides endpoints for document management and search. POST /search accepts queries with different modes. GET /suggestions returns autocomplete suggestions. All endpoints require authentication via JWT tokens.".to_string();
    let content_4_id = content_storage.store_text(content_4).await?;
    sqlx::query(
        r#"
        INSERT INTO documents (id, source_id, external_id, title, content_id, metadata, permissions, created_at, updated_at)
        VALUES ($1, $2, $3, $4, $5, $6, $7, NOW(), NOW())
        "#,
    )
    .bind(&doc_4_id)
    .bind(source_id)
    .bind("api_doc_1")
    .bind("REST API Endpoints")
    .bind(&content_4_id)
    .bind(json!({"type": "api_documentation", "version": "1.0"}))
    .bind(json!({"users": ["user1", "user2"], "groups": ["developers"]}))
    .execute(pool)
    .await?;
    doc_ids.push(doc_4_id);

    // Document 5: User guide
    let doc_5_id = Ulid::new().to_string();
    let content_5 = "Welcome to Clio! This guide will help you get started with searching across your organization's documents. You can search using keywords, phrases, or ask questions in natural language. The system will find relevant documents and highlight important passages.".to_string();
    let content_5_id = content_storage.store_text(content_5).await?;
    sqlx::query(
        r#"
        INSERT INTO documents (id, source_id, external_id, title, content_id, metadata, permissions, created_at, updated_at)
        VALUES ($1, $2, $3, $4, $5, $6, $7, NOW(), NOW())
        "#,
    )
    .bind(&doc_5_id)
    .bind(source_id)
    .bind("user_guide_1")
    .bind("Getting Started Guide")
    .bind(&content_5_id)
    .bind(json!({"type": "user_guide", "audience": "end_users"}))
    .bind(json!({"users": ["user1", "user2", "user3"], "groups": ["all_users"]}))
    .execute(pool)
    .await?;
    doc_ids.push(doc_5_id);

    Ok(doc_ids)
}

/// Create test documents with embeddings for semantic search testing
pub async fn create_test_documents_with_embeddings(pool: &PgPool) -> Result<Vec<String>> {
    let doc_ids = create_test_documents(pool).await?;

    // Create sample embeddings (in real scenario, these would come from AI service)
    // Using dummy embeddings for testing - in production these would be generated by the AI service
    let _dummy_embedding = vec![0.1f32; 1024]; // 1024-dimensional vector

    // Different embeddings for different documents to test similarity
    let embeddings = vec![
        // Document 1: Technical/Programming focused
        vec![0.8, 0.2, 0.1, 0.9, 0.3]
            .into_iter()
            .cycle()
            .take(1024)
            .collect::<Vec<f32>>(),
        // Document 2: Meeting/Planning focused
        vec![0.2, 0.8, 0.3, 0.1, 0.7]
            .into_iter()
            .cycle()
            .take(1024)
            .collect::<Vec<f32>>(),
        // Document 3: Architecture/Technical focused
        vec![0.9, 0.1, 0.2, 0.8, 0.4]
            .into_iter()
            .cycle()
            .take(1024)
            .collect::<Vec<f32>>(),
        // Document 4: API/Development focused
        vec![0.7, 0.3, 0.8, 0.2, 0.5]
            .into_iter()
            .cycle()
            .take(1024)
            .collect::<Vec<f32>>(),
        // Document 5: User guide focused
        vec![0.3, 0.7, 0.5, 0.4, 0.8]
            .into_iter()
            .cycle()
            .take(1024)
            .collect::<Vec<f32>>(),
    ];

    for (i, doc_id) in doc_ids.iter().enumerate() {
        let embedding = &embeddings[i];
        let embedding_id = Ulid::new().to_string();

        // For simplicity, we'll create one embedding per document (chunk_index = 0)
        // In real usage, documents would be split into chunks
        sqlx::query(
            r#"
            INSERT INTO embeddings (id, document_id, chunk_index, chunk_start_offset, chunk_end_offset, embedding, model_name, created_at)
            VALUES ($1, $2, $3, $4, $5, $6, $7, NOW())
            "#,
        )
        .bind(embedding_id)
        .bind(doc_id)
        .bind(0) // chunk_index
        .bind(0) // chunk_start_offset
        .bind(100) // chunk_end_offset
        .bind(embedding)
        .bind("intfloat/e5-large-v2") // model_name
        .execute(pool)
        .await?;
    }

    Ok(doc_ids)
}

pub const TEST_USER_ID: &str = "01JGF7V3E0Y2R1X8P5Q7W9T4N6";
pub const TEST_SOURCE_ID: &str = "01JGF7V3E0Y2R1X8P5Q7W9T4N7";

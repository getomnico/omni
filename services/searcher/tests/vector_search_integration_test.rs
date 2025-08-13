use clio_searcher::models::{SearchMode, SearchRequest};
use clio_searcher::search::SearchEngine;
use pgvector::Vector;
use serde::{Deserialize, Serialize};
use serde_json::json;
use shared::models::{Document, Embedding, SourceType};
use shared::test_environment::TestEnvironment;
use shared::{AIClient, ContentStorage, SearcherConfig};
use sqlx::types::time::OffsetDateTime;
use std::fs;
use ulid::Ulid;

#[derive(Debug, Deserialize, Serialize)]
struct TestEmbeddingData {
    documents: Vec<TestDocument>,
    queries: Vec<TestQuery>,
}

#[derive(Debug, Deserialize, Serialize)]
struct TestDocument {
    id: String,
    title: String,
    content: String,
    embedding: Vec<Vec<f32>>,
    chunks: Vec<Vec<i32>>,
    chunks_count: usize,
}

#[derive(Debug, Deserialize, Serialize)]
struct TestQuery {
    text: String,
    embedding: Vec<Vec<f32>>,
}

async fn load_test_embeddings() -> TestEmbeddingData {
    let data = fs::read_to_string("tests/test_embeddings.json")
        .or_else(|_| fs::read_to_string("test_embeddings.json"))
        .expect("Failed to read test_embeddings.json");
    serde_json::from_str(&data).expect("Failed to parse test embeddings")
}

async fn setup_test_environment() -> TestEnvironment {
    let env = TestEnvironment::new()
        .await
        .expect("Failed to setup test environment");

    // Set all required environment variables for searcher
    std::env::set_var("DATABASE_URL", env.db_pool.database_url());
    std::env::set_var("REDIS_URL", &env.redis_config().redis_url);
    std::env::set_var("PORT", "8002");
    std::env::set_var("AI_SERVICE_URL", &env.mock_ai_server.base_url);
    std::env::set_var("RUST_LOG", "info");

    env
}

async fn insert_test_documents(env: &TestEnvironment) {
    let test_data = load_test_embeddings().await;
    let content_storage = ContentStorage::new(env.db_pool.pool().clone());

    // Get first available user from database
    let user_id: String = sqlx::query_scalar("SELECT id FROM users LIMIT 1")
        .fetch_one(env.db_pool.pool())
        .await
        .expect("No users found in test database");

    // Insert source first
    let source_id = Ulid::new().to_string();
    sqlx::query(
        r#"
        INSERT INTO sources (id, name, source_type, config, is_active, created_at, updated_at, created_by)
        VALUES ($1, $2, $3, $4, $5, $6, $7, $8)
        "#,
    )
    .bind(&source_id)
    .bind("test_source")
    .bind(SourceType::LocalFiles)
    .bind(json!({}))
    .bind(true)
    .bind(OffsetDateTime::now_utc())
    .bind(OffsetDateTime::now_utc())
    .bind(&user_id)
    .execute(env.db_pool.pool())
    .await
    .unwrap();

    // Insert test documents with real embeddings
    for doc in test_data.documents {
        // Store content in content_blobs table
        let content_id = content_storage
            .store_text(&doc.content)
            .await
            .expect("Failed to store content");

        let document = Document {
            id: Ulid::new().to_string(),
            source_id: source_id.clone(),
            external_id: doc.id.clone(),
            title: doc.title.clone(),
            content_id: Some(content_id.clone()),
            content_type: Some("text/plain".to_string()),
            file_size: None,
            file_extension: None,
            url: None,
            parent_id: None,
            metadata: json!({}),
            permissions: json!({}),
            created_at: OffsetDateTime::now_utc(),
            updated_at: OffsetDateTime::now_utc(),
            last_indexed_at: OffsetDateTime::now_utc(),
        };

        // Insert document
        sqlx::query(
            r#"
            INSERT INTO documents (id, source_id, external_id, title, content_id, content_type, file_size, file_extension, url, parent_id, metadata, permissions, created_at, updated_at, last_indexed_at)
            VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13, $14, $15)
            "#,
        )
        .bind(&document.id)
        .bind(&document.source_id)
        .bind(&document.external_id)
        .bind(&document.title)
        .bind(&document.content_id)
        .bind(&document.content_type)
        .bind(&document.file_size)
        .bind(&document.file_extension)
        .bind(&document.url)
        .bind(&document.parent_id)
        .bind(&document.metadata)
        .bind(&document.permissions)
        .bind(&document.created_at)
        .bind(&document.updated_at)
        .bind(&document.last_indexed_at)
        .execute(env.db_pool.pool())
        .await
        .unwrap();

        // Insert embedding
        let embedding = Embedding {
            id: Ulid::new().to_string(),
            document_id: document.id,
            chunk_index: 0,
            chunk_start_offset: 0,
            chunk_end_offset: doc.content.len() as i32,
            embedding: Vector::from(doc.embedding.get(0).cloned().unwrap_or_default()),
            model_name: "jinaai/jina-embeddings-v3".to_string(),
            created_at: OffsetDateTime::now_utc(),
        };

        sqlx::query(
            r#"
            INSERT INTO embeddings (id, document_id, chunk_index, chunk_start_offset, chunk_end_offset, embedding, model_name, created_at)
            VALUES ($1, $2, $3, $4, $5, $6, $7, $8)
            "#,
        )
        .bind(&embedding.id)
        .bind(&embedding.document_id)
        .bind(&embedding.chunk_index)
        .bind(&embedding.chunk_start_offset)
        .bind(&embedding.chunk_end_offset)
        .bind(&embedding.embedding)
        .bind(&embedding.model_name)
        .bind(&embedding.created_at)
        .execute(env.db_pool.pool())
        .await
        .unwrap();
    }
}

#[tokio::test]
async fn test_vector_search() {
    let env = setup_test_environment().await;

    insert_test_documents(&env).await;

    // Load test queries
    let test_data = load_test_embeddings().await;

    // Test with "machine learning neural networks" query
    let ml_query = test_data
        .queries
        .iter()
        .find(|q| q.text == "machine learning neural networks")
        .expect("ML query not found");

    // Create search request with semantic mode
    let request = SearchRequest {
        query: "machine learning".to_string(),
        source_types: None,
        content_types: None,
        limit: Some(10),
        offset: None,
        mode: Some(SearchMode::Semantic),
        include_facets: Some(false),
    };

    // Test search
    let ai_client = AIClient::new("http://localhost:3003".to_string());
    let config = SearcherConfig::from_env();
    let search_engine = SearchEngine::new(
        env.db_pool.clone(),
        env.redis_client.clone(),
        ai_client,
        config,
    );

    // Create mock embedding response for the query
    let mut conn = env
        .redis_client
        .get_multiplexed_async_connection()
        .await
        .unwrap();
    let cache_key = format!("embeddings:jinaai/jina-embeddings-v3:{}", request.query);
    let _: () = redis::cmd("SET")
        .arg(&cache_key)
        .arg(
            serde_json::to_string(&ml_query.embedding.get(0).cloned().unwrap_or_default()).unwrap(),
        )
        .arg("EX")
        .arg(3600)
        .query_async(&mut conn)
        .await
        .unwrap();

    let response = search_engine.search(request).await.unwrap();

    assert!(!response.results.is_empty());
    // The most similar documents should contain ML/AI content
    let top_result = &response.results[0].document.title;
    assert!(
        top_result.contains("Machine Learning")
            || top_result.contains("Artificial Intelligence")
            || top_result.contains("Deep Learning"),
        "Expected ML/AI related document, got: {}",
        top_result
    );
}

#[tokio::test]
async fn test_hybrid_search() {
    let env = setup_test_environment().await;

    insert_test_documents(&env).await;

    // Load test queries
    let test_data = load_test_embeddings().await;

    // Test with "database optimization performance" query
    let db_query = test_data
        .queries
        .iter()
        .find(|q| q.text == "database optimization performance")
        .expect("Database query not found");

    // Create search request with hybrid mode
    let request = SearchRequest {
        query: "database performance".to_string(),
        source_types: None,
        content_types: None,
        limit: Some(10),
        offset: None,
        mode: Some(SearchMode::Hybrid),
        include_facets: Some(false),
    };

    // Test search
    let ai_client = AIClient::new("http://localhost:3003".to_string());
    let config = SearcherConfig::from_env();
    let search_engine = SearchEngine::new(
        env.db_pool.clone(),
        env.redis_client.clone(),
        ai_client,
        config,
    );

    // Create mock embedding response for the query
    let mut conn = env
        .redis_client
        .get_multiplexed_async_connection()
        .await
        .unwrap();
    let cache_key = format!("embeddings:jinaai/jina-embeddings-v3:{}", request.query);
    let _: () = redis::cmd("SET")
        .arg(&cache_key)
        .arg(
            serde_json::to_string(&db_query.embedding.get(0).cloned().unwrap_or_default()).unwrap(),
        )
        .arg("EX")
        .arg(3600)
        .query_async(&mut conn)
        .await
        .unwrap();

    let response = search_engine.search(request).await.unwrap();

    assert!(!response.results.is_empty());
    // Database document should be ranked first due to both text and vector similarity
    assert!(response.results[0].document.title.contains("Database"));
}

#[tokio::test]
async fn test_vector_search_similarity_ranking() {
    let env = setup_test_environment().await;

    insert_test_documents(&env).await;

    // Load test queries
    let test_data = load_test_embeddings().await;

    // Test with AI/deep learning query
    let ai_query = test_data
        .queries
        .iter()
        .find(|q| q.text == "artificial intelligence deep learning")
        .expect("AI query not found");

    // Create search request
    let request = SearchRequest {
        query: "artificial intelligence deep learning".to_string(),
        source_types: None,
        content_types: None,
        limit: Some(5),
        offset: None,
        mode: Some(SearchMode::Semantic),
        include_facets: Some(false),
    };

    let ai_client = AIClient::new("http://localhost:3003".to_string());
    let config = SearcherConfig::from_env();
    let search_engine = SearchEngine::new(
        env.db_pool.clone(),
        env.redis_client.clone(),
        ai_client,
        config,
    );

    // Create mock embedding response for the query
    let mut conn = env
        .redis_client
        .get_multiplexed_async_connection()
        .await
        .unwrap();
    let cache_key = format!("embeddings:jinaai/jina-embeddings-v3:{}", request.query);
    let _: () = redis::cmd("SET")
        .arg(&cache_key)
        .arg(
            serde_json::to_string(&ai_query.embedding.get(0).cloned().unwrap_or_default()).unwrap(),
        )
        .arg("EX")
        .arg(3600)
        .query_async(&mut conn)
        .await
        .unwrap();

    let response = search_engine.search(request).await.unwrap();

    assert!(!response.results.is_empty());

    // Check that AI/ML related documents are ranked higher
    let titles: Vec<String> = response
        .results
        .iter()
        .take(3)
        .map(|r| r.document.title.clone())
        .collect();

    let ai_ml_count = titles
        .iter()
        .filter(|t| t.contains("Learning") || t.contains("Intelligence") || t.contains("Neural"))
        .count();

    assert!(
        ai_ml_count >= 2,
        "Expected at least 2 AI/ML documents in top 3 results"
    );
}

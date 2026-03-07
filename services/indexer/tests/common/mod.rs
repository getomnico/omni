use anyhow::Result;
use omni_indexer::{create_app, AppState};
use shared::db::repositories::DocumentRepository;
use shared::models::{ConnectorEvent, Document, DocumentMetadata, DocumentPermissions};
use shared::queue::EventQueue;
use shared::storage::postgres::PostgresStorage;
use shared::test_environment::TestEnvironment;
use shared::ObjectStorage;
use sqlx::PgPool;
use std::sync::Arc;
use tokio::time::{sleep, timeout, Duration};

pub const TEST_SOURCE_ID: &str = "01JGF7V3E0Y2R1X8P5Q7W9T4N7";

pub struct TestFixture {
    pub state: AppState,
    #[allow(dead_code)]
    pub app: axum::Router,
    #[allow(dead_code)]
    test_env: TestEnvironment,
}

impl TestFixture {
    #[allow(dead_code)]
    pub fn app(&self) -> &axum::Router {
        &self.app
    }
}

pub async fn setup_test_fixture() -> Result<TestFixture> {
    std::env::set_var(
        "ENCRYPTION_KEY",
        "test_master_key_that_is_long_enough_32_chars",
    );
    std::env::set_var("ENCRYPTION_SALT", "test_salt_16_chars");

    let test_env = TestEnvironment::new().await?;

    let ai_client = shared::AIClient::new(test_env.mock_ai_server.base_url.clone());

    let embedding_queue =
        shared::embedding_queue::EmbeddingQueue::new(test_env.db_pool.pool().clone());

    let content_storage: Arc<dyn shared::ObjectStorage> =
        Arc::new(PostgresStorage::new(test_env.db_pool.pool().clone()));

    let service_credentials_repo = std::sync::Arc::new(
        shared::ServiceCredentialsRepo::new(test_env.db_pool.pool().clone()).unwrap(),
    );

    let app_state = AppState {
        db_pool: test_env.db_pool.clone(),
        redis_client: test_env.redis_client.clone(),
        ai_client,
        embedding_queue,
        content_storage,
        service_credentials_repo,
    };

    let app = create_app(app_state.clone());

    Ok(TestFixture {
        state: app_state,
        app,
        test_env,
    })
}

pub mod fixtures {
    use omni_indexer::{CreateDocumentRequest, UpdateDocumentRequest};
    use serde_json::json;

    #[allow(dead_code)]
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

    #[allow(dead_code)]
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

#[allow(dead_code)]
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

#[allow(dead_code)]
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

#[allow(dead_code)]
pub async fn wait_for_document_with_title(
    repo: &DocumentRepository,
    source_id: &str,
    doc_id: &str,
    expected_title: &str,
    timeout_duration: Duration,
) -> Result<Document, String> {
    let result = timeout(timeout_duration, async {
        loop {
            if let Ok(Some(doc)) = repo.find_by_external_id(source_id, doc_id).await {
                if doc.title == expected_title {
                    return doc;
                }
            }
            sleep(Duration::from_millis(10)).await;
        }
    })
    .await;

    match result {
        Ok(doc) => Ok(doc),
        Err(_) => Err(format!(
            "Document {}:{} with title '{}' not found within timeout",
            source_id, doc_id, expected_title
        )),
    }
}

#[allow(dead_code)]
pub async fn enqueue_dummy_events(
    event_queue: &EventQueue,
    source_id: &str,
    content_storage: &Arc<dyn ObjectStorage>,
    count: usize,
    sync_run_id: &str,
    doc_id_prefix: &str,
) -> Vec<String> {
    let mut event_ids = Vec::new();
    for i in 0..count {
        let content_id = content_storage
            .store_content(format!("content for doc {}", i).as_bytes(), None)
            .await
            .unwrap();

        let event = ConnectorEvent::DocumentCreated {
            sync_run_id: sync_run_id.to_string(),
            source_id: source_id.to_string(),
            document_id: format!("{}_{}", doc_id_prefix, i),
            content_id,
            metadata: DocumentMetadata {
                title: Some(format!("Document {}", i)),
                author: None,
                created_at: None,
                updated_at: None,
                content_type: None,
                mime_type: Some("text/plain".to_string()),
                size: Some("100".to_string()),
                url: None,
                path: None,
                extra: None,
            },
            permissions: DocumentPermissions {
                public: true,
                users: vec![],
                groups: vec![],
            },
            attributes: None,
        };

        let event_id = event_queue.enqueue(source_id, &event).await.unwrap();
        event_ids.push(event_id);
    }
    event_ids
}

#[allow(dead_code)]
pub async fn count_completed_events(pool: &PgPool) -> i64 {
    let row: (i64,) =
        sqlx::query_as("SELECT COUNT(*) FROM connector_events_queue WHERE status = 'completed'")
            .fetch_one(pool)
            .await
            .unwrap();
    row.0
}

#[allow(dead_code)]
pub async fn wait_for_completed(pool: &PgPool, expected: i64, timeout_duration: Duration) -> i64 {
    let start = std::time::Instant::now();
    loop {
        let completed = count_completed_events(pool).await;
        if completed >= expected {
            return completed;
        }
        if start.elapsed() > timeout_duration {
            return completed;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}

#[allow(dead_code)]
pub async fn wait_for_embedding_queue_entry(
    pool: &PgPool,
    document_id: &str,
    timeout_duration: Duration,
) -> Result<(), String> {
    let result = timeout(timeout_duration, async {
        loop {
            let row: Option<(i64,)> =
                sqlx::query_as("SELECT COUNT(*) FROM embedding_queue WHERE document_id = $1")
                    .bind(document_id)
                    .fetch_optional(pool)
                    .await
                    .unwrap();

            if let Some((count,)) = row {
                if count > 0 {
                    return;
                }
            }
            sleep(Duration::from_millis(50)).await;
        }
    })
    .await;

    match result {
        Ok(_) => Ok(()),
        Err(_) => Err(format!(
            "Embedding queue entry for document {} not found within timeout",
            document_id
        )),
    }
}

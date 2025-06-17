use anyhow::Result;
use clio_indexer::{create_app, AppState};
use shared::db::repositories::DocumentRepository;
use shared::models::Document;
use shared::test_environment::TestEnvironment;
use tokio::time::{sleep, timeout, Duration};

/// Test fixture that automatically cleans up the test database on drop
pub struct TestFixture {
    pub state: AppState,
    pub app: axum::Router,
    test_env: TestEnvironment,
}

impl TestFixture {
    /// Get a reference to the app router
    pub fn app(&self) -> &axum::Router {
        &self.app
    }
}

/// Setup test app with automatic cleanup via TestFixture
pub async fn setup_test_fixture() -> Result<TestFixture> {
    let test_env = TestEnvironment::new().await?;

    let ai_client = shared::AIClient::new(test_env.mock_ai_server.base_url.clone());

    let app_state = AppState {
        db_pool: test_env.db_pool.clone(),
        redis_client: test_env.redis_client.clone(),
        ai_client,
    };

    let app = create_app(app_state.clone());

    Ok(TestFixture {
        state: app_state,
        app,
        test_env,
    })
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

/// Wait for a document to exist with a specific title in the database with polling and timeout
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

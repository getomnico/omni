mod common;

use axum::http::StatusCode;
use axum_test::{TestServer, TestServerConfig};
use common::TEST_SOURCE_ID;
use omni_connector_manager::source_cleanup::SourceCleanup;
use serde_json::json;
use shared::db::repositories::SyncRunRepository;
use shared::models::{ConnectorEvent, DocumentMetadata, DocumentPermissions, SyncStatus};
use shared::queue::EventQueue;

fn test_server(fixture: &common::TestFixture) -> TestServer {
    let config = TestServerConfig::builder()
        .default_content_type("application/json")
        .expect_success_by_default()
        .build();
    TestServer::new_with_config(fixture.app.clone(), config).unwrap()
}

fn test_server_no_expect(fixture: &common::TestFixture) -> TestServer {
    let config = TestServerConfig::builder()
        .default_content_type("application/json")
        .build();
    TestServer::new_with_config(fixture.app.clone(), config).unwrap()
}

async fn trigger_sync(server: &TestServer) -> String {
    let resp = server
        .post("/sync")
        .json(&json!({"source_id": TEST_SOURCE_ID}))
        .await;
    resp.assert_status(StatusCode::OK);
    let body: serde_json::Value = resp.json();
    body["sync_run_id"].as_str().unwrap().to_string()
}

async fn seed_source(pool: &sqlx::PgPool, source_type: &str, is_active: bool) -> String {
    let id = shared::utils::generate_ulid();
    let user_id = "01JGF7V3E0Y2R1X8P5Q7W9T4N6";
    sqlx::query(
        r#"
        INSERT INTO sources (id, name, source_type, config, is_active, created_by, created_at, updated_at)
        VALUES ($1, 'Extra Source', $2, '{}', $3, $4, NOW(), NOW())
        "#,
    )
    .bind(&id)
    .bind(source_type)
    .bind(is_active)
    .bind(user_id)
    .execute(pool)
    .await
    .unwrap();
    id
}

async fn create_running_sync(pool: &sqlx::PgPool, source_id: &str) -> String {
    let repo = SyncRunRepository::new(pool);
    let sync_run = repo
        .create(source_id, shared::models::SyncType::Full, "manual")
        .await
        .unwrap();
    sync_run.id
}

// ============================================================================
// 1. test_sync_lifecycle — golden-path end-to-end
// ============================================================================
#[tokio::test]
async fn test_sync_lifecycle() {
    let fixture = common::setup_test_fixture().await.unwrap();
    let server = test_server(&fixture);
    let pool = fixture.state.db_pool.pool();
    let sync_run_repo = SyncRunRepository::new(pool);

    // Trigger sync
    let sync_run_id = trigger_sync(&server).await;

    let run = sync_run_repo
        .find_by_id(&sync_run_id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(run.status, SyncStatus::Running);

    let requests = fixture.mock_connector.get_sync_requests();
    assert_eq!(requests.len(), 1);
    assert_eq!(requests[0].source_id, TEST_SOURCE_ID);

    // SDK heartbeat
    server
        .post(&format!("/sdk/sync/{}/heartbeat", sync_run_id))
        .await
        .assert_status(StatusCode::OK);

    // SDK increment_scanned
    server
        .post(&format!("/sdk/sync/{}/scanned", sync_run_id))
        .json(&json!({"count": 5}))
        .await
        .assert_status(StatusCode::OK);
    server
        .post(&format!("/sdk/sync/{}/scanned", sync_run_id))
        .json(&json!({"count": 3}))
        .await
        .assert_status(StatusCode::OK);

    let run = sync_run_repo
        .find_by_id(&sync_run_id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(run.documents_scanned, 8);

    // SDK complete
    server
        .post(&format!("/sdk/sync/{}/complete", sync_run_id))
        .json(&json!({
            "documents_scanned": 42,
            "documents_updated": 10,
            "new_state": {"cursor": "abc"}
        }))
        .await
        .assert_status(StatusCode::OK);

    let run = sync_run_repo
        .find_by_id(&sync_run_id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(run.status, SyncStatus::Completed);
    assert_eq!(run.documents_scanned, 42);
    assert_eq!(run.documents_updated, 10);

    let source_row: (Option<serde_json::Value>,) =
        sqlx::query_as("SELECT connector_state FROM sources WHERE id = $1")
            .bind(TEST_SOURCE_ID)
            .fetch_one(pool)
            .await
            .unwrap();
    assert_eq!(source_row.0.unwrap()["cursor"].as_str(), Some("abc"));
}

// ============================================================================
// 2. test_sync_trigger_guards — rejection paths
// ============================================================================
#[tokio::test]
async fn test_sync_trigger_guards() {
    let fixture = common::setup_test_fixture().await.unwrap();
    let server = test_server_no_expect(&fixture);
    let pool = fixture.state.db_pool.pool();

    // Nonexistent source → 404
    let resp = server
        .post("/sync")
        .json(&json!({"source_id": "nonexistent_source_id_00000"}))
        .await;
    resp.assert_status(StatusCode::NOT_FOUND);

    // Inactive source → 400
    let inactive_id = seed_source(pool, "local_files", false).await;
    let resp = server
        .post("/sync")
        .json(&json!({"source_id": inactive_id}))
        .await;
    resp.assert_status(StatusCode::BAD_REQUEST);
    let body: serde_json::Value = resp.json();
    assert!(body["error"]
        .as_str()
        .unwrap()
        .to_lowercase()
        .contains("inactive"));

    // Already running → 409
    let resp = server
        .post("/sync")
        .json(&json!({"source_id": TEST_SOURCE_ID}))
        .await;
    resp.assert_status(StatusCode::OK);

    let resp = server
        .post("/sync")
        .json(&json!({"source_id": TEST_SOURCE_ID}))
        .await;
    resp.assert_status(StatusCode::CONFLICT);
    let body: serde_json::Value = resp.json();
    assert!(body["error"]
        .as_str()
        .unwrap()
        .to_lowercase()
        .contains("already running"));

    // Concurrency limit (max_concurrent_syncs=2)
    let source2 = seed_source(pool, "local_files", true).await;
    let _run2 = create_running_sync(pool, &source2).await;
    // Now 2 running (TEST_SOURCE_ID + source2) → third rejected
    let source3 = seed_source(pool, "local_files", true).await;
    let resp = server
        .post("/sync")
        .json(&json!({"source_id": source3}))
        .await;
    resp.assert_status(StatusCode::CONFLICT);
    let body: serde_json::Value = resp.json();
    assert!(body["error"]
        .as_str()
        .unwrap()
        .to_lowercase()
        .contains("concurrency"));

    // Mock connector received exactly 1 sync request
    let requests = fixture.mock_connector.get_sync_requests();
    assert_eq!(requests.len(), 1);
}

// ============================================================================
// 3. test_sync_connector_failure — connector /sync returns 500
// ============================================================================
#[tokio::test]
async fn test_sync_connector_failure() {
    let fixture = common::setup_test_fixture().await.unwrap();
    let pool = fixture.state.db_pool.pool();

    fixture
        .mock_connector
        .set_sync_response(StatusCode::INTERNAL_SERVER_ERROR, json!({"error": "boom"}));

    let server = test_server_no_expect(&fixture);

    let resp = server
        .post("/sync")
        .json(&json!({"source_id": TEST_SOURCE_ID}))
        .await;
    resp.assert_status(StatusCode::INTERNAL_SERVER_ERROR);

    let repo = SyncRunRepository::new(pool);
    let runs = repo.find_all_running().await.unwrap();
    assert!(runs.is_empty());
}

// ============================================================================
// 4. test_cancel_sync — cancel flow + double-cancel error
// ============================================================================
#[tokio::test]
async fn test_cancel_sync() {
    let fixture = common::setup_test_fixture().await.unwrap();
    let server = test_server(&fixture);
    let pool = fixture.state.db_pool.pool();
    let sync_run_repo = SyncRunRepository::new(pool);

    let sync_run_id = trigger_sync(&server).await;

    server
        .post(&format!("/sync/{}/cancel", sync_run_id))
        .await
        .assert_status(StatusCode::OK);

    let run = sync_run_repo
        .find_by_id(&sync_run_id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(run.status, SyncStatus::Cancelled);

    let cancel_requests = fixture.mock_connector.get_cancel_requests();
    assert_eq!(cancel_requests.len(), 1);
    assert_eq!(cancel_requests[0].sync_run_id, sync_run_id);

    // Double-cancel → 400
    let server2 = test_server_no_expect(&fixture);
    let resp = server2.post(&format!("/sync/{}/cancel", sync_run_id)).await;
    resp.assert_status(StatusCode::BAD_REQUEST);
    let body: serde_json::Value = resp.json();
    assert!(body["error"]
        .as_str()
        .unwrap()
        .to_lowercase()
        .contains("not running"));
}

// ============================================================================
// 5. test_sync_failure_via_sdk — connector reports failure
// ============================================================================
#[tokio::test]
async fn test_sync_failure_via_sdk() {
    let fixture = common::setup_test_fixture().await.unwrap();
    let server = test_server(&fixture);
    let pool = fixture.state.db_pool.pool();
    let sync_run_repo = SyncRunRepository::new(pool);

    let sync_run_id = trigger_sync(&server).await;

    server
        .post(&format!("/sdk/sync/{}/fail", sync_run_id))
        .json(&json!({"error": "Out of memory"}))
        .await
        .assert_status(StatusCode::OK);

    let run = sync_run_repo
        .find_by_id(&sync_run_id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(run.status, SyncStatus::Failed);
    assert_eq!(run.error_message.as_deref(), Some("Out of memory"));
}

// ============================================================================
// 6. test_sdk_event_and_content — data-flow SDK endpoints
// ============================================================================
#[tokio::test]
async fn test_sdk_event_and_content() {
    let fixture = common::setup_test_fixture().await.unwrap();
    let server = test_server(&fixture);
    let pool = fixture.state.db_pool.pool();

    let sync_run_id = trigger_sync(&server).await;

    // Emit event
    let event = ConnectorEvent::DocumentCreated {
        sync_run_id: sync_run_id.clone(),
        source_id: TEST_SOURCE_ID.to_string(),
        document_id: "doc_001".to_string(),
        content_id: "content_001".to_string(),
        metadata: DocumentMetadata {
            title: Some("Test Doc".to_string()),
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

    server
        .post("/sdk/events")
        .json(&json!({
            "sync_run_id": sync_run_id,
            "source_id": TEST_SOURCE_ID,
            "event": event
        }))
        .await
        .assert_status(StatusCode::OK);

    let event_queue = EventQueue::new(pool.clone());
    let stats = event_queue.get_queue_stats().await.unwrap();
    assert!(
        stats.pending >= 1,
        "Expected at least 1 pending event, got {}",
        stats.pending
    );

    // Store content
    let resp = server
        .post("/sdk/content")
        .json(&json!({
            "sync_run_id": sync_run_id,
            "content": "Hello World"
        }))
        .await;
    resp.assert_status(StatusCode::OK);
    let body: serde_json::Value = resp.json();
    let content_id = body["content_id"].as_str().unwrap();
    assert!(!content_id.is_empty());

    let stored = fixture
        .state
        .content_storage
        .get_text(content_id)
        .await
        .unwrap();
    assert_eq!(stored, "Hello World");
}

// ============================================================================
// 7. test_stale_sync_detection — verifies cancel is sent and next sync unblocked
// ============================================================================
#[tokio::test]
async fn test_stale_sync_detection() {
    let fixture = common::setup_test_fixture().await.unwrap();
    let server = test_server(&fixture);
    let pool = fixture.state.db_pool.pool();
    let sync_run_repo = SyncRunRepository::new(pool);

    // 1. Trigger sync via API — mock connector tracks source as active
    let sync_run_id = trigger_sync(&server).await;

    // 2. Backdate last_activity_at beyond the 1-minute timeout
    sqlx::query(
        "UPDATE sync_runs SET last_activity_at = NOW() - INTERVAL '10 minutes', started_at = NOW() - INTERVAL '10 minutes' WHERE id = $1",
    )
    .bind(&sync_run_id)
    .execute(pool)
    .await
    .unwrap();

    // 3. detect_stale_syncs should cancel on connector then mark failed
    let stale = fixture
        .state
        .sync_manager
        .detect_stale_syncs()
        .await
        .unwrap();
    assert!(
        stale.contains(&sync_run_id),
        "Expected stale sync_run_id in result"
    );

    // 4. Assert cancel request was received by mock connector
    let cancel_requests = fixture.mock_connector.get_cancel_requests();
    assert_eq!(
        cancel_requests.len(),
        1,
        "Expected exactly 1 cancel request"
    );
    assert_eq!(cancel_requests[0].sync_run_id, sync_run_id);

    // 5. Assert sync run is marked as failed
    let run = sync_run_repo
        .find_by_id(&sync_run_id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(run.status, SyncStatus::Failed);
    assert!(
        run.error_message
            .as_deref()
            .unwrap_or("")
            .contains("timed out"),
        "Expected 'timed out' in error, got: {:?}",
        run.error_message
    );

    // 6. Trigger another sync for the same source — must succeed, not 409
    let server2 = test_server_no_expect(&fixture);
    let resp = server2
        .post("/sync")
        .json(&json!({"source_id": TEST_SOURCE_ID}))
        .await;
    resp.assert_status(StatusCode::OK);
}

// ============================================================================
// 8. test_source_cleanup — deleted source document + row cleanup
// ============================================================================

async fn seed_deleted_source_with_documents(
    pool: &sqlx::PgPool,
    doc_count: usize,
) -> (String, Vec<String>) {
    let source_id = shared::utils::generate_ulid();
    let user_id = "01JGF7V3E0Y2R1X8P5Q7W9T4N6";

    sqlx::query(
        r#"
        INSERT INTO sources (id, name, source_type, config, is_active, is_deleted, created_by, created_at, updated_at)
        VALUES ($1, 'Deleted Source', 'local_files', '{}', false, true, $2, NOW(), NOW())
        "#,
    )
    .bind(&source_id)
    .bind(user_id)
    .execute(pool)
    .await
    .unwrap();

    let mut doc_ids = Vec::with_capacity(doc_count);
    for i in 0..doc_count {
        let doc_id = shared::utils::generate_ulid();
        sqlx::query(
            r#"
            INSERT INTO documents (id, source_id, external_id, title, metadata, permissions, created_at, updated_at, last_indexed_at)
            VALUES ($1, $2, $3, $4, '{}', '[]', NOW(), NOW(), NOW())
            "#,
        )
        .bind(&doc_id)
        .bind(&source_id)
        .bind(format!("ext_{}", i))
        .bind(format!("Doc {}", i))
        .execute(pool)
        .await
        .unwrap();
        doc_ids.push(doc_id);
    }

    (source_id, doc_ids)
}

#[tokio::test]
async fn test_source_cleanup() {
    let fixture = common::setup_test_fixture().await.unwrap();
    let pool = fixture.state.db_pool.pool();

    let (source_id, _doc_ids) = seed_deleted_source_with_documents(pool, 3).await;

    // Verify setup: 3 documents exist
    let (doc_count,): (i64,) =
        sqlx::query_as("SELECT COUNT(*) FROM documents WHERE source_id = $1")
            .bind(&source_id)
            .fetch_one(pool)
            .await
            .unwrap();
    assert_eq!(doc_count, 3);

    // First call: deletes documents, source row remains
    SourceCleanup::cleanup_deleted_sources(pool).await;

    let (doc_count,): (i64,) =
        sqlx::query_as("SELECT COUNT(*) FROM documents WHERE source_id = $1")
            .bind(&source_id)
            .fetch_one(pool)
            .await
            .unwrap();
    assert_eq!(
        doc_count, 0,
        "All documents should be deleted after first cleanup call"
    );

    let (source_count,): (i64,) = sqlx::query_as("SELECT COUNT(*) FROM sources WHERE id = $1")
        .bind(&source_id)
        .fetch_one(pool)
        .await
        .unwrap();
    assert_eq!(
        source_count, 1,
        "Source row should still exist after first cleanup call"
    );

    // Second call: no documents remain, so source row is deleted
    SourceCleanup::cleanup_deleted_sources(pool).await;

    let (source_count,): (i64,) = sqlx::query_as("SELECT COUNT(*) FROM sources WHERE id = $1")
        .bind(&source_id)
        .fetch_one(pool)
        .await
        .unwrap();
    assert_eq!(
        source_count, 0,
        "Source row should be deleted after second cleanup call"
    );
}

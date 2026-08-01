mod common;

use axum::http::StatusCode;
use axum_test::{TestServer, TestServerConfig};
use common::TEST_SOURCE_ID;
use omni_connector_manager::source_cleanup::SourceCleanup;
use redis::AsyncCommands;
use serde_json::json;
use shared::db::repositories::SyncRunRepository;
use shared::models::{
    ConnectorEvent, DocumentMetadata, DocumentPermissions, PersonSyncRecord, SyncStatus, SyncType,
};
use shared::queue::EventQueue;

struct DummyConnectorEmitter<'a> {
    server: &'a TestServer,
    source_id: String,
    sync_run_id: String,
}

impl<'a> DummyConnectorEmitter<'a> {
    async fn emit(&self, event: ConnectorEvent) {
        self.server
            .post("/sdk/events")
            .json(&json!({
                "sync_run_id": self.sync_run_id,
                "source_id": self.source_id,
                "event": event,
            }))
            .await
            .assert_status(StatusCode::OK);
    }
}

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

async fn set_source_checkpoint(pool: &sqlx::PgPool, checkpoint: serde_json::Value) {
    sqlx::query("UPDATE sources SET checkpoint = $1 WHERE id = $2")
        .bind(checkpoint)
        .bind(TEST_SOURCE_ID)
        .execute(pool)
        .await
        .unwrap();
}

async fn get_source_checkpoint(pool: &sqlx::PgPool) -> Option<serde_json::Value> {
    sqlx::query_scalar("SELECT checkpoint FROM sources WHERE id = $1")
        .bind(TEST_SOURCE_ID)
        .fetch_one(pool)
        .await
        .unwrap()
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

    // Save run checkpoint — it is promoted to the source on successful complete.
    server
        .put(&format!("/sdk/sync/{}/checkpoint", sync_run_id))
        .json(&json!({"cursor": "abc"}))
        .await
        .assert_status(StatusCode::OK);

    // SDK complete atomically flips status and publishes the checkpoint.
    server
        .post(&format!("/sdk/sync/{}/complete", sync_run_id))
        .await
        .assert_status(StatusCode::OK);

    let run = sync_run_repo
        .find_by_id(&sync_run_id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(run.status, SyncStatus::Completed);
    assert_eq!(run.documents_scanned, 8);
    assert_eq!(run.documents_updated, 0);

    let source_row: (Option<serde_json::Value>,) =
        sqlx::query_as("SELECT checkpoint FROM sources WHERE id = $1")
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
    assert!(
        body["error"]
            .as_str()
            .unwrap()
            .to_lowercase()
            .contains("inactive")
    );

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
    assert!(
        body["error"]
            .as_str()
            .unwrap()
            .to_lowercase()
            .contains("already running")
    );

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
    assert!(
        body["error"]
            .as_str()
            .unwrap()
            .to_lowercase()
            .contains("concurrency")
    );

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

#[tokio::test]
async fn test_realtime_unavailable_is_not_failed() {
    let fixture = common::setup_test_fixture().await.unwrap();
    let pool = fixture.state.db_pool.pool();

    fixture
        .mock_connector
        .set_sync_response(StatusCode::NOT_FOUND, json!({"error": "not available"}));

    let server = test_server_no_expect(&fixture);
    let resp = server
        .post("/sync")
        .json(&json!({"source_id": TEST_SOURCE_ID, "sync_mode": "realtime"}))
        .await;
    resp.assert_status(StatusCode::BAD_REQUEST);

    let repo = SyncRunRepository::new(pool);
    let runs = repo
        .list_runs_for_sync_types(&[TEST_SOURCE_ID.to_string()], &[SyncType::Realtime], 1)
        .await
        .unwrap();
    assert_eq!(runs.len(), 1);
    assert_eq!(runs[0].status, SyncStatus::Cancelled);
    assert_eq!(
        runs[0].error_message.as_deref(),
        Some("Realtime sync not available for this source")
    );
    assert!(repo.find_all_running().await.unwrap().is_empty());
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
    assert!(
        body["error"]
            .as_str()
            .unwrap()
            .to_lowercase()
            .contains("not running")
    );
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
// 8. test_monitor_resumes_lost_sync — connector restart mid-sync
// ============================================================================
#[tokio::test]
async fn test_monitor_resumes_lost_sync() {
    let fixture = common::setup_test_fixture().await.unwrap();
    let server = test_server(&fixture);
    let pool = fixture.state.db_pool.pool();
    let sync_run_repo = SyncRunRepository::new(pool);

    let sync_run_id = trigger_sync(&server).await;
    assert_eq!(fixture.mock_connector.get_sync_requests().len(), 1);

    // Simulate connector crash + restart: kills the HTTP server, rebinds on
    // the same port, drops in-memory active_syncs. Recorded history is kept.
    fixture.mock_connector.restart().await.unwrap();

    fixture
        .state
        .sync_manager
        .monitor_running_syncs()
        .await
        .unwrap();

    // Existing row should still be running (not failed), and the connector
    // should have received a second /sync for the same sync_run_id.
    let run = sync_run_repo
        .find_by_id(&sync_run_id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(run.status, SyncStatus::Running);

    let requests = fixture.mock_connector.get_sync_requests();
    assert_eq!(requests.len(), 2, "expected an auto-resume /sync call");
    assert_eq!(requests[1].sync_run_id, sync_run_id);
    assert_eq!(requests[1].source_id, TEST_SOURCE_ID);
}

// ============================================================================
// 9. test_monitor_gives_up_after_max_resume_attempts
// ============================================================================
#[tokio::test]
async fn test_monitor_gives_up_after_max_resume_attempts() {
    let fixture = common::setup_test_fixture().await.unwrap();
    let server = test_server(&fixture);
    let pool = fixture.state.db_pool.pool();
    let sync_run_repo = SyncRunRepository::new(pool);

    let sync_run_id = trigger_sync(&server).await;

    // Each restart drops active_syncs, so every monitor pass sees
    // running=false and attempts a resume. After 3 resumes, the 4th pass
    // should mark the row failed.
    for _ in 0..4 {
        fixture.mock_connector.restart().await.unwrap();
        fixture
            .state
            .sync_manager
            .monitor_running_syncs()
            .await
            .unwrap();
    }

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
            .contains("auto-resume gave up"),
        "unexpected error message: {:?}",
        run.error_message
    );

    // 3 successful resume attempts + 1 original trigger = 4 /sync calls.
    let requests = fixture.mock_connector.get_sync_requests();
    assert_eq!(requests.len(), 4);
}

// ============================================================================
// 10. test_monitor_noop_when_connector_reports_running
// ============================================================================
#[tokio::test]
async fn test_monitor_noop_when_connector_reports_running() {
    let fixture = common::setup_test_fixture().await.unwrap();
    let server = test_server(&fixture);
    let pool = fixture.state.db_pool.pool();
    let sync_run_repo = SyncRunRepository::new(pool);

    let sync_run_id = trigger_sync(&server).await;

    fixture
        .state
        .sync_manager
        .monitor_running_syncs()
        .await
        .unwrap();

    let run = sync_run_repo
        .find_by_id(&sync_run_id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(run.status, SyncStatus::Running);
    assert_eq!(
        fixture.mock_connector.get_sync_requests().len(),
        1,
        "monitor must not re-trigger when connector says sync is running"
    );
}

// ============================================================================
// 11. test_monitor_tolerates_404_status_endpoint
// ============================================================================
#[tokio::test]
async fn test_monitor_tolerates_404_status_endpoint() {
    let fixture = common::setup_test_fixture().await.unwrap();
    let server = test_server(&fixture);
    let pool = fixture.state.db_pool.pool();
    let sync_run_repo = SyncRunRepository::new(pool);

    let sync_run_id = trigger_sync(&server).await;

    // Simulates a connector that doesn't implement GET /sync/{id}
    // (current Rust connectors). Monitor must treat 404 as a no-op and
    // leave the row alone.
    fixture.mock_connector.set_status_endpoint_enabled(false);

    fixture
        .state
        .sync_manager
        .monitor_running_syncs()
        .await
        .unwrap();

    let run = sync_run_repo
        .find_by_id(&sync_run_id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(run.status, SyncStatus::Running);
    assert_eq!(fixture.mock_connector.get_sync_requests().len(), 1);
}

// ============================================================================
// 12. test_monitor_marks_failed_when_connector_unregistered
// ============================================================================
#[tokio::test]
async fn test_monitor_marks_failed_when_connector_unregistered() {
    let fixture = common::setup_test_fixture().await.unwrap();
    let server = test_server(&fixture);
    let pool = fixture.state.db_pool.pool();
    let sync_run_repo = SyncRunRepository::new(pool);

    let sync_run_id = trigger_sync(&server).await;

    // Simulate the Redis registration TTL expiring: the connector has been
    // down long enough that the manager no longer knows its URL.
    let mut redis_conn = fixture
        .state
        .redis_client
        .get_multiplexed_async_connection()
        .await
        .unwrap();
    let _: () = redis_conn
        .del("connector:manifest:filesystem")
        .await
        .unwrap();

    // MAX_RESUME_ATTEMPTS = 3, so 4 monitor passes should exceed the cap
    // and mark the row failed.
    for _ in 0..4 {
        fixture
            .state
            .sync_manager
            .monitor_running_syncs()
            .await
            .unwrap();
    }

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
            .contains("auto-resume gave up"),
        "unexpected error: {:?}",
        run.error_message
    );
}

// ============================================================================
// 13. test_monitor_treats_unreachable_connector_as_lost
// ============================================================================
#[tokio::test]
async fn test_monitor_treats_unreachable_connector_as_lost() {
    let fixture = common::setup_test_fixture().await.unwrap();
    let server = test_server(&fixture);
    let pool = fixture.state.db_pool.pool();
    let sync_run_repo = SyncRunRepository::new(pool);

    let sync_run_id = trigger_sync(&server).await;

    // Connector is down but its Redis registration is still present — the
    // situation during the first 90s of a connector outage.
    fixture.mock_connector.stop().await;

    for _ in 0..4 {
        fixture
            .state
            .sync_manager
            .monitor_running_syncs()
            .await
            .unwrap();
    }

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
            .contains("auto-resume gave up"),
        "unexpected error: {:?}",
        run.error_message
    );
}

// ============================================================================
// 14. test_source_cleanup — deleted source document + row cleanup
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
async fn test_source_cleanup_queues_people_deactivation_before_source_deletion() {
    let fixture = common::setup_test_fixture().await.unwrap();
    let pool = fixture.state.db_pool.pool();
    let (source_id, _) = seed_deleted_source_with_documents(pool, 0).await;
    let person_id = shared::utils::generate_ulid();
    sqlx::query(
        "INSERT INTO people (id,email,is_active,source_data) VALUES ($1,'cleanup@example.com',true,jsonb_build_object($2::text,jsonb_build_object('external_id','EMP')))",
    )
    .bind(&person_id)
    .bind(&source_id)
    .execute(pool)
    .await
    .unwrap();

    let ordinary_run = create_running_sync(pool, &source_id).await;
    EventQueue::new(pool.clone())
        .enqueue(
            &source_id,
            &ConnectorEvent::PersonSync {
                sync_run_id: ordinary_run.clone(),
                source_id: source_id.clone(),
                person: PersonSyncRecord {
                    external_id: "LATE".into(),
                    email: "late@example.com".into(),
                    display_name: None,
                    given_name: None,
                    middle_name: None,
                    surname: None,
                    job_title: None,
                    department: None,
                    division: None,
                    company_name: None,
                    office_location: None,
                    work_country: None,
                    employee_id: None,
                    employee_type: None,
                    cost_center: None,
                    grade: None,
                    band: None,
                    confirmation_status: None,
                    employment_start_date: None,
                    employment_end_date: None,
                    manager_external_id: None,
                    source_updated_at: None,
                },
            },
        )
        .await
        .unwrap();

    SourceCleanup::cleanup_deleted_sources(pool).await;
    let ordinary_pending: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM connector_events_queue WHERE source_id=$1 AND event_type='person_sync' AND status='pending'",
    )
    .bind(&source_id)
    .fetch_one(pool)
    .await
    .unwrap();
    assert_eq!(ordinary_pending, 1);
    let cleanup_events: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM connector_events_queue WHERE source_id=$1 AND event_type='person_deleted'",
    )
    .bind(&source_id)
    .fetch_one(pool)
    .await
    .unwrap();
    assert_eq!(
        cleanup_events, 0,
        "ordinary person mutation must quiesce first"
    );

    sqlx::query("UPDATE connector_events_queue SET status='completed' WHERE sync_run_id=$1")
        .bind(&ordinary_run)
        .execute(pool)
        .await
        .unwrap();
    sqlx::query(
        "INSERT INTO people (id,email,is_active,source_data) VALUES ($1,'late@example.com',true,jsonb_build_object($2::text,jsonb_build_object('external_id','LATE')))",
    )
    .bind(shared::utils::generate_ulid())
    .bind(&source_id)
    .execute(pool)
    .await
    .unwrap();
    SourceCleanup::cleanup_deleted_sources(pool).await;

    let source_count: i64 = sqlx::query_scalar("SELECT count(*) FROM sources WHERE id=$1")
        .bind(&source_id)
        .fetch_one(pool)
        .await
        .unwrap();
    assert_eq!(source_count, 1);
    let queued: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM connector_events_queue q JOIN sync_runs r ON r.id=q.sync_run_id WHERE q.source_id=$1 AND q.event_type='person_deleted' AND q.status='pending' AND r.trigger_type='source_cleanup' AND q.payload->>'email' IN ('cleanup@example.com','late@example.com')",
    )
    .bind(&source_id)
    .fetch_one(pool)
    .await
    .unwrap();
    assert_eq!(
        queued, 2,
        "cleanup must include data from the ordinary mutation"
    );

    SourceCleanup::cleanup_deleted_sources(pool).await;
    let cleanup_events: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM connector_events_queue WHERE source_id=$1 AND event_type='person_deleted'",
    )
    .bind(&source_id).fetch_one(pool).await.unwrap();
    assert_eq!(cleanup_events, 2, "pending cleanup must be idempotent");

    sqlx::query("UPDATE connector_events_queue SET status='completed' WHERE source_id=$1 AND event_type='person_deleted'")
        .bind(&source_id).execute(pool).await.unwrap();
    sqlx::query(
        "UPDATE people SET source_data=source_data-$1, is_active=false WHERE source_data ? $1",
    )
    .bind(&source_id)
    .execute(pool)
    .await
    .unwrap();
    SourceCleanup::cleanup_deleted_sources(pool).await;
    let source_count: i64 = sqlx::query_scalar("SELECT count(*) FROM sources WHERE id=$1")
        .bind(&source_id)
        .fetch_one(pool)
        .await
        .unwrap();
    assert_eq!(
        source_count, 0,
        "reconciled source should be physically deleted"
    );
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

#[tokio::test]
async fn test_source_cleanup_pending_non_person_event_blocks_source_deletion() {
    let fixture = common::setup_test_fixture().await.unwrap();
    let pool = fixture.state.db_pool.pool();
    let (source_id, _) = seed_deleted_source_with_documents(pool, 0).await;

    fn doc_event(sync_run_id: &str, source_id: &str, document_id: &str) -> ConnectorEvent {
        ConnectorEvent::DocumentCreated {
            sync_run_id: sync_run_id.to_string(),
            source_id: source_id.to_string(),
            document_id: document_id.to_string(),
            content_id: format!("{document_id}-content"),
            metadata: DocumentMetadata::default(),
            permissions: DocumentPermissions {
                public: false,
                users: vec![],
                groups: vec![],
            },
            attributes: None,
        }
    }

    // A document already written by a settled event.
    sqlx::query(
        r#"
        INSERT INTO documents (id, source_id, external_id, title, metadata, permissions, created_at, updated_at, last_indexed_at)
        VALUES ($1, $2, 'ext-existing', 'Existing', '{}', '[]', NOW(), NOW(), NOW())
        "#,
    )
    .bind(shared::utils::generate_ulid())
    .bind(&source_id)
    .execute(pool)
    .await
    .unwrap();

    // An already-admitted (pending) document event must quiesce before any
    // document deletion or source deletion — otherwise a processing event's
    // document write could land after the cleanup DELETE and be orphaned by
    // the physical source deletion.
    let run_id = create_running_sync(pool, &source_id).await;
    let pending_id = EventQueue::new(pool.clone())
        .enqueue(&source_id, &doc_event(&run_id, &source_id, "doc-1"))
        .await
        .unwrap();

    // A failed event simulates a processing -> failed transition that the
    // cleanup pass cannot observe atomically. It must be dead-lettered under
    // the lock, and the dead-letter must survive the early return (commit, not
    // rollback) so it can never be retried behind a deleted source.
    let failed_id = EventQueue::new(pool.clone())
        .enqueue(&source_id, &doc_event(&run_id, &source_id, "doc-2"))
        .await
        .unwrap();
    sqlx::query("UPDATE connector_events_queue SET status='failed' WHERE id=$1")
        .bind(&failed_id)
        .execute(pool)
        .await
        .unwrap();

    SourceCleanup::cleanup_deleted_sources(pool).await;

    // Unresolved events block document deletion AND source deletion: the
    // settled document must survive this pass untouched.
    let (doc_count,): (i64,) =
        sqlx::query_as("SELECT count(*) FROM documents WHERE source_id=$1")
            .bind(&source_id)
            .fetch_one(pool)
            .await
            .unwrap();
    assert_eq!(
        doc_count, 1,
        "documents must not be deleted while events are unresolved"
    );
    let (source_count,): (i64,) = sqlx::query_as("SELECT count(*) FROM sources WHERE id=$1")
        .bind(&source_id)
        .fetch_one(pool)
        .await
        .unwrap();
    assert_eq!(
        source_count, 1,
        "pending non-person event must prevent physical source deletion"
    );
    let (pending,): (i64,) = sqlx::query_as(
        "SELECT count(*) FROM connector_events_queue WHERE id=$1 AND status='pending'",
    )
    .bind(&pending_id)
    .fetch_one(pool)
    .await
    .unwrap();
    assert_eq!(pending, 1, "pending document event must remain pending");
    let (dead,): (i64,) = sqlx::query_as(
        "SELECT count(*) FROM connector_events_queue WHERE id=$1 AND status='dead_letter'",
    )
    .bind(&failed_id)
    .fetch_one(pool)
    .await
    .unwrap();
    assert_eq!(
        dead, 1,
        "failed non-person event must be dead-lettered under the lock and stay dead-lettered across the early return"
    );

    // Once the pending event settles, the next pass deletes the documents
    // (bounded batch), then a final pass removes the source row.
    sqlx::query("UPDATE connector_events_queue SET status='completed' WHERE id=$1")
        .bind(&pending_id)
        .execute(pool)
        .await
        .unwrap();
    SourceCleanup::cleanup_deleted_sources(pool).await;
    let (doc_count,): (i64,) =
        sqlx::query_as("SELECT count(*) FROM documents WHERE source_id=$1")
            .bind(&source_id)
            .fetch_one(pool)
            .await
            .unwrap();
    assert_eq!(
        doc_count, 0,
        "documents must be deleted only after all admitted events settle"
    );
    let (source_count,): (i64,) = sqlx::query_as("SELECT count(*) FROM sources WHERE id=$1")
        .bind(&source_id)
        .fetch_one(pool)
        .await
        .unwrap();
    assert_eq!(
        source_count, 1,
        "source must outlive the bounded document deletion pass"
    );

    SourceCleanup::cleanup_deleted_sources(pool).await;
    let (source_count,): (i64,) = sqlx::query_as("SELECT count(*) FROM sources WHERE id=$1")
        .bind(&source_id)
        .fetch_one(pool)
        .await
        .unwrap();
    assert_eq!(
        source_count, 0,
        "source must be deleted once all admitted events settle"
    );
}

// ============================================================================
// Checkpoint regression coverage
// ============================================================================
#[tokio::test]
async fn test_initial_sync_request_uses_latest_successful_source_checkpoint() {
    let fixture = common::setup_test_fixture().await.unwrap();
    let server = test_server(&fixture);
    let pool = fixture.state.db_pool.pool();

    set_source_checkpoint(pool, json!({"cursor": "last-success"})).await;

    let sync_run_id = trigger_sync(&server).await;
    let requests = fixture.mock_connector.get_sync_requests();
    assert_eq!(requests.len(), 1);
    assert_eq!(requests[0].sync_run_id, sync_run_id);
    assert!(!requests[0].is_resume);
    assert_eq!(
        requests[0].checkpoint.as_ref().unwrap()["cursor"].as_str(),
        Some("last-success")
    );
}

#[tokio::test]
async fn test_full_sync_resume_prefers_run_checkpoint_over_source_checkpoint() {
    let fixture = common::setup_test_fixture().await.unwrap();
    let server = test_server(&fixture);
    let pool = fixture.state.db_pool.pool();

    set_source_checkpoint(pool, json!({"cursor": "previous-success"})).await;
    let sync_run_id = trigger_sync(&server).await;

    server
        .put(&format!("/sdk/sync/{}/checkpoint", sync_run_id))
        .json(&json!({"cursor": "current-run"}))
        .await
        .assert_status(StatusCode::OK);

    fixture.mock_connector.restart().await.unwrap();
    fixture
        .state
        .sync_manager
        .monitor_running_syncs()
        .await
        .unwrap();

    let requests = fixture.mock_connector.get_sync_requests();
    assert_eq!(requests.len(), 2, "expected resume /sync call");
    assert!(requests[1].is_resume);
    assert_eq!(
        requests[1].checkpoint.as_ref().unwrap()["cursor"].as_str(),
        Some("current-run")
    );
}

#[tokio::test]
async fn test_resume_falls_back_to_source_checkpoint_before_first_run_checkpoint() {
    let fixture = common::setup_test_fixture().await.unwrap();
    let server = test_server(&fixture);
    let pool = fixture.state.db_pool.pool();

    set_source_checkpoint(pool, json!({"cursor": "last-success"})).await;
    let sync_run_id = trigger_sync(&server).await;

    fixture.mock_connector.restart().await.unwrap();
    fixture
        .state
        .sync_manager
        .monitor_running_syncs()
        .await
        .unwrap();

    let requests = fixture.mock_connector.get_sync_requests();
    assert_eq!(requests.len(), 2);
    assert_eq!(requests[1].sync_run_id, sync_run_id);
    assert!(requests[1].is_resume);
    assert_eq!(
        requests[1].checkpoint.as_ref().unwrap()["cursor"].as_str(),
        Some("last-success")
    );
}

#[tokio::test]
async fn test_failed_or_cancelled_run_checkpoint_is_not_promoted() {
    let fixture = common::setup_test_fixture().await.unwrap();
    let server = test_server(&fixture);
    let pool = fixture.state.db_pool.pool();

    set_source_checkpoint(pool, json!({"cursor": "previous-success"})).await;
    let sync_run_id = trigger_sync(&server).await;

    server
        .put(&format!("/sdk/sync/{}/checkpoint", sync_run_id))
        .json(&json!({"cursor": "failed-run"}))
        .await
        .assert_status(StatusCode::OK);
    server
        .post(&format!("/sdk/sync/{}/fail", sync_run_id))
        .json(&json!({"error": "boom"}))
        .await
        .assert_status(StatusCode::OK);

    let checkpoint = get_source_checkpoint(pool).await.unwrap();
    assert_eq!(checkpoint["cursor"].as_str(), Some("previous-success"));
}

#[tokio::test]
async fn test_completion_promotes_checkpoint_and_preserves_connector_metadata() {
    let fixture = common::setup_test_fixture().await.unwrap();
    let server = test_server(&fixture);
    let pool = fixture.state.db_pool.pool();

    sqlx::query("UPDATE sources SET connector_state = $1 WHERE id = $2")
        .bind(json!({"webhook_id": "wh-1"}))
        .bind(TEST_SOURCE_ID)
        .execute(pool)
        .await
        .unwrap();

    let sync_run_id = trigger_sync(&server).await;
    let before: time::OffsetDateTime =
        sqlx::query_scalar("SELECT updated_at FROM sources WHERE id = $1")
            .bind(TEST_SOURCE_ID)
            .fetch_one(pool)
            .await
            .unwrap();

    server
        .put(&format!("/sdk/sync/{}/checkpoint", sync_run_id))
        .json(&json!({"cursor": "completed"}))
        .await
        .assert_status(StatusCode::OK);
    server
        .post(&format!("/sdk/sync/{}/complete", sync_run_id))
        .await
        .assert_status(StatusCode::OK);

    let row: (
        Option<serde_json::Value>,
        Option<serde_json::Value>,
        time::OffsetDateTime,
    ) = sqlx::query_as("SELECT checkpoint, connector_state, updated_at FROM sources WHERE id = $1")
        .bind(TEST_SOURCE_ID)
        .fetch_one(pool)
        .await
        .unwrap();
    assert_eq!(row.0.unwrap()["cursor"].as_str(), Some("completed"));
    assert_eq!(row.1.unwrap()["webhook_id"].as_str(), Some("wh-1"));
    assert!(row.2 >= before);
}

#[tokio::test]
async fn test_incremental_after_failed_sync_starts_from_previous_successful_checkpoint() {
    let fixture = common::setup_test_fixture().await.unwrap();
    let server = test_server(&fixture);
    let pool = fixture.state.db_pool.pool();
    let sync_run_repo = SyncRunRepository::new(pool);

    let completed = sync_run_repo
        .create(TEST_SOURCE_ID, SyncType::Full, "manual")
        .await
        .unwrap();
    sync_run_repo
        .update_checkpoint(&completed.id, json!({"cursor": "successful"}))
        .await
        .unwrap();
    sync_run_repo
        .complete_and_publish_checkpoint(&completed.id)
        .await
        .unwrap();

    let failed = sync_run_repo
        .create(TEST_SOURCE_ID, SyncType::Incremental, "manual")
        .await
        .unwrap();
    sync_run_repo
        .update_checkpoint(&failed.id, json!({"cursor": "failed"}))
        .await
        .unwrap();
    sync_run_repo.mark_failed(&failed.id, "boom").await.unwrap();

    let resp = server
        .post("/sync")
        .json(&json!({"source_id": TEST_SOURCE_ID, "sync_mode": "incremental"}))
        .await;
    resp.assert_status(StatusCode::OK);

    let requests = fixture.mock_connector.get_sync_requests();
    assert_eq!(requests.len(), 1);
    assert_eq!(requests[0].sync_mode, "incremental");
    assert_eq!(
        requests[0].checkpoint.as_ref().unwrap()["cursor"].as_str(),
        Some("successful")
    );
}

fn person_event(run: &str, source: &str, email: &str) -> ConnectorEvent {
    ConnectorEvent::PersonDeleted {
        sync_run_id: run.into(),
        source_id: source.into(),
        email: email.into(),
    }
}

#[tokio::test]
async fn sdk_event_rejects_mismatched_event_context_and_run_ownership() {
    let fixture = common::setup_test_fixture().await.unwrap();
    let server = test_server_no_expect(&fixture);
    let pool = fixture.state.db_pool.pool();
    let run = create_running_sync(pool, TEST_SOURCE_ID).await;
    let other_source = seed_source(pool, "slack", true).await;
    let response = server
        .post("/sdk/events")
        .json(&json!({
            "sync_run_id": run, "source_id": TEST_SOURCE_ID,
            "event": person_event(&run, &other_source, "01J00000000000000000000001")
        }))
        .await;
    response.assert_status(StatusCode::BAD_REQUEST);

    let response = server
        .post("/sdk/events")
        .json(&json!({
            "sync_run_id": run, "source_id": other_source,
            "event": person_event(&run, &other_source, "01J00000000000000000000001")
        }))
        .await;
    response.assert_status(StatusCode::BAD_REQUEST);

    sqlx::query("UPDATE sync_runs SET status = 'completed' WHERE id = $1")
        .bind(&run)
        .execute(pool)
        .await
        .unwrap();
    let response = server
        .post("/sdk/events")
        .json(&json!({
            "sync_run_id": run, "source_id": TEST_SOURCE_ID,
            "event": person_event(&run, TEST_SOURCE_ID, "01J00000000000000000000002")
        }))
        .await;
    response.assert_status(StatusCode::BAD_REQUEST);
    response.assert_text_contains("running sync run");
}

#[tokio::test]
async fn sdk_event_rejects_deleted_source_before_enqueue() {
    let fixture = common::setup_test_fixture().await.unwrap();
    let server = test_server_no_expect(&fixture);
    let pool = fixture.state.db_pool.pool();
    let run = create_running_sync(pool, TEST_SOURCE_ID).await;
    sqlx::query("UPDATE sources SET is_deleted=true WHERE id=$1")
        .bind(TEST_SOURCE_ID)
        .execute(pool)
        .await
        .unwrap();
    let before: i64 = sqlx::query_scalar("SELECT count(*) FROM connector_events_queue")
        .fetch_one(pool)
        .await
        .unwrap();

    let response = server
        .post("/sdk/events")
        .json(&json!({
            "sync_run_id": run, "source_id": TEST_SOURCE_ID,
            "event": person_event(&run, TEST_SOURCE_ID, "deleted@example.com")
        }))
        .await;
    response.assert_status(StatusCode::BAD_REQUEST);
    response.assert_text_contains("deleted source");

    let after: i64 = sqlx::query_scalar("SELECT count(*) FROM connector_events_queue")
        .fetch_one(pool)
        .await
        .unwrap();
    assert_eq!(before, after);
}

#[tokio::test]
async fn sdk_batch_rejects_mixed_context_atomically() {
    let fixture = common::setup_test_fixture().await.unwrap();
    let server = test_server_no_expect(&fixture);
    let pool = fixture.state.db_pool.pool();
    let run = create_running_sync(pool, TEST_SOURCE_ID).await;
    let before: i64 = sqlx::query_scalar("SELECT count(*) FROM connector_events_queue")
        .fetch_one(pool)
        .await
        .unwrap();
    let response = server
        .post("/sdk/events/batch")
        .json(&json!({
            "sync_run_id": run, "source_id": TEST_SOURCE_ID,
            "events": [
                person_event(&run, TEST_SOURCE_ID, "01J00000000000000000000001"),
                person_event("wrong-run", TEST_SOURCE_ID, "01J00000000000000000000002")
            ]
        }))
        .await;
    response.assert_status(StatusCode::BAD_REQUEST);
    let after: i64 = sqlx::query_scalar("SELECT count(*) FROM connector_events_queue")
        .fetch_one(pool)
        .await
        .unwrap();
    assert_eq!(before, after);
}

async fn wait_for_person_queue(
    pool: &sqlx::PgPool,
    sync_run_ids: &[&str],
    expected: i64,
    timeout: std::time::Duration,
) {
    let start = std::time::Instant::now();
    loop {
        let completed: i64 = sqlx::query_scalar(
            "SELECT count(*) FROM connector_events_queue WHERE sync_run_id=ANY($1) AND status='completed'",
        )
        .bind(sync_run_ids)
        .fetch_one(pool)
        .await
        .unwrap();
        if completed == expected {
            return;
        }
        if start.elapsed() > timeout {
            let details: Vec<(String, String, Option<String>)> = sqlx::query_as(
                "SELECT event_type, status, substring(error_message from 1 for 200) FROM connector_events_queue WHERE sync_run_id=ANY($1) AND status<>'completed' ORDER BY id",
            )
            .bind(sync_run_ids)
            .fetch_all(pool)
            .await
            .unwrap();
            let incomplete: i64 = details.len() as i64;
            let detail_str: String = details
                .iter()
                .map(|(et, st, err)| format!("  {} status={} err={:?}", et, st, err))
                .collect::<Vec<_>>()
                .join("\n");
            panic!(
                "Person sync test timed out after {:.1}s: completed={}, incomplete={}\n{}completed events:\n{}",
                timeout.as_secs_f64(),
                completed,
                incomplete,
                detail_str,
                {
                    let completed_details: Vec<(String, String)> = sqlx::query_as(
                        "SELECT event_type, status FROM connector_events_queue WHERE sync_run_id=ANY($1) AND status='completed' ORDER BY id"
                    )
                    .bind(sync_run_ids)
                    .fetch_all(pool)
                    .await
                    .unwrap();
                    completed_details
                        .iter()
                        .map(|(et, st)| format!("  {} status={}", et, st))
                        .collect::<Vec<_>>()
                        .join("\n")
                }
            );
        }
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    }
}

async fn wait_for_people_rows(pool: &sqlx::PgPool, source_id: &str) {
    let start = std::time::Instant::now();
    loop {
        let ready: bool = sqlx::query_scalar(
            r#"
            SELECT
                (SELECT count(*) FROM people) = 3
                AND EXISTS (
                    SELECT 1 FROM people
                    WHERE email='shared@example.com' AND source_data ? $1
                )
                AND EXISTS (
                    SELECT 1 FROM people
                    WHERE email='doc-only@example.com'
                )
            "#,
        )
        .bind(source_id)
        .fetch_one(pool)
        .await
        .unwrap();
        if ready {
            return;
        }
        assert!(
            start.elapsed() <= std::time::Duration::from_secs(15),
            "timed out waiting for implicit and explicit people writes"
        );
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    }
}

#[tokio::test]
async fn person_sync_and_document_extraction_merge_correctly() {
    use shared::models::{DocumentMetadata, DocumentPermissions, PersonSyncRecord};

    let fixture = common::setup_test_fixture().await.unwrap();
    let server = test_server(&fixture);
    let pool = fixture.state.db_pool.pool();
    let user_id = "01JGF7V3E0Y2R1X8P5Q7W9T4N6";

    // Seed two sources
    let source_a = shared::utils::generate_ulid();
    let source_b = shared::utils::generate_ulid();
    sqlx::query(
        "INSERT INTO sources (id,name,source_type,config,is_active,created_by,created_at,updated_at) VALUES ($1,'PersonSync Source','darwinbox','{}',true,$2,NOW(),NOW())",
    )
    .bind(&source_a)
    .bind(user_id)
    .execute(pool)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO sources (id,name,source_type,config,is_active,created_by,created_at,updated_at) VALUES ($1,'Document Source','local_files','{}',true,$2,NOW(),NOW())",
    )
    .bind(&source_b)
    .bind(user_id)
    .execute(pool)
    .await
    .unwrap();

    // Create running sync runs for both
    let run_a = create_running_sync(pool, &source_a).await;
    let run_b = create_running_sync(pool, &source_b).await;

    // Start indexer QueueProcessor in background
    let indexer_state = fixture.indexer_state();
    let processor = omni_indexer::QueueProcessor::new(indexer_state)
        .with_poll_interval(std::time::Duration::from_millis(100))
        .with_batch_size(1)
        .with_full_batch_size(1)
        .with_full_max_age_secs(1);
    let processor_handle = tokio::spawn(async move {
        let _ = processor.start().await;
    });
    tokio::time::sleep(std::time::Duration::from_millis(300)).await;

    // Store content for the document event
    let content_id = fixture
        .state
        .content_storage
        .store_content(b"test document for person extraction", None)
        .await
        .unwrap();

    // ── Emit PersonSync events for source_a (authoritative) ──
    let emitter_a = DummyConnectorEmitter {
        server: &server,
        source_id: source_a.clone(),
        sync_run_id: run_a.clone(),
    };

    // PersonSync for shared@example.com (case variant to test normalization)
    emitter_a
        .emit(ConnectorEvent::PersonSync {
            sync_run_id: run_a.clone(),
            source_id: source_a.clone(),
            person: PersonSyncRecord {
                external_id: "EMP-001".to_string(),
                email: "Shared@Example.com".to_string(),
                display_name: Some("Alice Authority".to_string()),
                given_name: Some("Alice".to_string()),
                middle_name: None,
                surname: None,
                job_title: None,
                department: Some("Engineering".to_string()),
                division: None,
                company_name: None,
                office_location: None,
                work_country: None,
                employee_id: Some("EMP-001".to_string()),
                employee_type: None,
                cost_center: None,
                grade: None,
                band: None,
                confirmation_status: None,
                employment_start_date: None,
                employment_end_date: None,
                manager_external_id: None,
                source_updated_at: None,
            },
        })
        .await;

    // PersonSync for another-only@example.com (only authoritative, no doc mention)
    emitter_a
        .emit(ConnectorEvent::PersonSync {
            sync_run_id: run_a.clone(),
            source_id: source_a.clone(),
            person: PersonSyncRecord {
                external_id: "EMP-002".to_string(),
                email: "another-only@example.com".to_string(),
                display_name: Some("Bob Only".to_string()),
                given_name: None,
                middle_name: None,
                surname: None,
                job_title: None,
                department: None,
                division: None,
                company_name: None,
                office_location: None,
                work_country: None,
                employee_id: Some("EMP-002".to_string()),
                employee_type: None,
                cost_center: None,
                grade: None,
                band: None,
                confirmation_status: None,
                employment_start_date: None,
                employment_end_date: None,
                manager_external_id: None,
                source_updated_at: None,
            },
        })
        .await;

    // ── Emit DocumentCreated for source_b with people references ──
    let emitter_b = DummyConnectorEmitter {
        server: &server,
        source_id: source_b.clone(),
        sync_run_id: run_b.clone(),
    };
    emitter_b
        .emit(ConnectorEvent::DocumentCreated {
            sync_run_id: run_b.clone(),
            source_id: source_b.clone(),
            document_id: "doc-1".to_string(),
            content_id,
            metadata: DocumentMetadata {
                title: Some("Test Doc".to_string()),
                author: Some("doc-only@example.com".to_string()),
                ..Default::default()
            },
            permissions: DocumentPermissions {
                public: false,
                users: vec![
                    "Shared@Example.com".to_string(),
                    "doc-only@example.com".to_string(),
                ],
                groups: vec![],
            },
            attributes: None,
        })
        .await;

    // Wait for the exact test events and for implicit extraction, which runs
    // immediately after the document event is marked completed.
    wait_for_person_queue(
        pool,
        &[run_a.as_str(), run_b.as_str()],
        3,
        std::time::Duration::from_secs(15),
    )
    .await;
    wait_for_people_rows(pool, &source_a).await;

    // ── Assertions ──

    // 1. shared@example.com: one merged row, authoritative fields from PersonSync,
    //    source_data contains source_a entry
    let shared_row: (String, Option<String>, Option<String>, bool) = sqlx::query_as(
        "SELECT email, display_name, department, source_data ? $1 FROM people WHERE lower(email)='shared@example.com'",
    )
    .bind(&source_a)
    .fetch_one(pool)
    .await
    .unwrap();
    assert_eq!(
        shared_row.0, "shared@example.com",
        "email should be normalized to lowercase"
    );
    assert_eq!(
        shared_row.1.as_deref(),
        Some("Alice Authority"),
        "display_name must come from authoritative PersonSync"
    );
    assert_eq!(shared_row.2.as_deref(), Some("Engineering"));
    assert!(shared_row.3, "source_a must have an entry in source_data");

    // 2. doc-only@example.com: one row, display_name is null (weak extraction
    //    provides no name), source_data is empty
    let doc_only_row: (String, Option<String>, bool, bool) = sqlx::query_as(
        "SELECT email, display_name, is_active, source_data = '{}'::jsonb FROM people WHERE lower(email)='doc-only@example.com'",
    )
    .fetch_one(pool)
    .await
    .unwrap();
    assert_eq!(doc_only_row.0, "doc-only@example.com");
    assert!(
        doc_only_row.1.is_none(),
        "weak document extraction should not set a display_name"
    );
    assert!(doc_only_row.2, "doc-only person should be active");
    assert!(
        doc_only_row.3,
        "source_data should be empty for doc-only people"
    );

    // 3. another-only@example.com: only from PersonSync, has display_name and source_data
    let another_row: (String, Option<String>) = sqlx::query_as(
        "SELECT email, display_name FROM people WHERE lower(email)='another-only@example.com'",
    )
    .fetch_one(pool)
    .await
    .unwrap();
    assert_eq!(another_row.0, "another-only@example.com");
    assert_eq!(
        another_row.1.as_deref(),
        Some("Bob Only"),
        "PersonSync display_name must be stored"
    );

    // 4. Exactly 3 people rows, no duplicates
    let total: i64 = sqlx::query_scalar("SELECT count(*) FROM people")
        .fetch_one(pool)
        .await
        .unwrap();
    assert_eq!(total, 3, "must have exactly 3 distinct canonical people");

    // 5. No person duplicates by email
    let dup_count: i64 =
        sqlx::query_scalar("SELECT count(*) - count(DISTINCT lower(email)) FROM people")
            .fetch_one(pool)
            .await
            .unwrap();
    assert_eq!(dup_count, 0, "no duplicate email rows");

    processor_handle.abort();
    // Give the processor a moment to shut down cleanly
    tokio::time::sleep(std::time::Duration::from_millis(100)).await;
}

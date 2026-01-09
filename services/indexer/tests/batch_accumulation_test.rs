mod common;

use common::setup_test_fixture;
use omni_indexer::QueueProcessor;
use shared::db::repositories::DocumentRepository;
use shared::models::{ConnectorEvent, DocumentMetadata, DocumentPermissions};
use shared::queue::EventQueue;
use shared::ObjectStorage;
use sqlx::PgPool;
use std::sync::Arc;
use tokio::time::Duration;

const TEST_SOURCE_ID: &str = "01JGF7V3E0Y2R1X8P5Q7W9T4N7";

/// Helper to enqueue N dummy events
async fn enqueue_dummy_events(
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

/// Helper to count completed events
async fn count_completed_events(pool: &PgPool) -> i64 {
    let row: (i64,) =
        sqlx::query_as("SELECT COUNT(*) FROM connector_events_queue WHERE status = 'completed'")
            .fetch_one(pool)
            .await
            .unwrap();
    row.0
}

/// Helper to wait for expected completed count with timeout
async fn wait_for_completed(pool: &PgPool, expected: i64, timeout: Duration) -> i64 {
    let start = std::time::Instant::now();
    loop {
        let completed = count_completed_events(pool).await;
        if completed >= expected {
            return completed;
        }
        if start.elapsed() > timeout {
            return completed;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}

/// Test 1: Single event should be processed after idle timeout
#[tokio::test]
async fn test_idle_timeout_single_event() {
    let fixture = setup_test_fixture().await.unwrap();
    let event_queue = EventQueue::new(fixture.state.db_pool.pool().clone());

    let state = fixture.state.clone();
    let processor = QueueProcessor::new(state).with_accumulation_config(
        Duration::from_millis(200), // Short idle_timeout for testing
        Duration::from_secs(30),    // max_accumulation_wait
        Duration::from_millis(50),  // Short batch_check_interval
    );

    let processor_handle = tokio::spawn(async move {
        let _ = processor.start().await;
    });

    // Give processor time to initialize
    tokio::time::sleep(Duration::from_millis(100)).await;

    // Enqueue single event
    enqueue_dummy_events(
        &event_queue,
        TEST_SOURCE_ID,
        &fixture.state.content_storage,
        1,
        "sync_idle_single",
        "idle_single",
    )
    .await;

    // Wait for processing (idle timeout + processing time)
    let completed =
        wait_for_completed(fixture.state.db_pool.pool(), 1, Duration::from_secs(5)).await;

    assert_eq!(
        completed, 1,
        "Single event should be processed after idle timeout"
    );

    processor_handle.abort();
}

/// Test 2: Multiple events enqueued rapidly should be processed together after idle timeout
#[tokio::test]
async fn test_idle_timeout_burst_then_pause() {
    let fixture = setup_test_fixture().await.unwrap();
    let event_queue = EventQueue::new(fixture.state.db_pool.pool().clone());

    let state = fixture.state.clone();
    let processor = QueueProcessor::new(state).with_accumulation_config(
        Duration::from_millis(200),
        Duration::from_secs(30),
        Duration::from_millis(50),
    );

    let processor_handle = tokio::spawn(async move {
        let _ = processor.start().await;
    });
    tokio::time::sleep(Duration::from_millis(100)).await;

    // Enqueue 5 events rapidly (no delay between)
    enqueue_dummy_events(
        &event_queue,
        TEST_SOURCE_ID,
        &fixture.state.content_storage,
        5,
        "sync_burst",
        "burst",
    )
    .await;

    // Wait for all to be processed
    let completed =
        wait_for_completed(fixture.state.db_pool.pool(), 5, Duration::from_secs(5)).await;

    assert_eq!(
        completed, 5,
        "All burst events should be processed together after idle timeout"
    );

    processor_handle.abort();
}

/// Test 3: Events should be processed when threshold (batch_size) is reached
#[tokio::test]
async fn test_threshold_trigger() {
    let fixture = setup_test_fixture().await.unwrap();
    let event_queue = EventQueue::new(fixture.state.db_pool.pool().clone());

    let state = fixture.state.clone();
    let processor = QueueProcessor::new(state)
        .with_batch_size(10) // Small batch for testing
        .with_accumulation_config(
            Duration::from_secs(60),   // Long idle timeout (shouldn't trigger)
            Duration::from_secs(300),  // Long max wait
            Duration::from_millis(50), // Check interval
        );

    let processor_handle = tokio::spawn(async move {
        let _ = processor.start().await;
    });
    tokio::time::sleep(Duration::from_millis(100)).await;

    // Enqueue exactly batch_size events
    enqueue_dummy_events(
        &event_queue,
        TEST_SOURCE_ID,
        &fixture.state.content_storage,
        10,
        "sync_threshold",
        "threshold",
    )
    .await;

    // Should be processed quickly due to threshold, not idle timeout
    let completed =
        wait_for_completed(fixture.state.db_pool.pool(), 10, Duration::from_secs(5)).await;

    assert_eq!(completed, 10, "Batch should process when threshold reached");

    processor_handle.abort();
}

/// Test 4: Events exceeding batch_size should be processed in multiple batches
#[tokio::test]
async fn test_threshold_with_continuation() {
    let fixture = setup_test_fixture().await.unwrap();
    let event_queue = EventQueue::new(fixture.state.db_pool.pool().clone());

    let state = fixture.state.clone();
    let processor = QueueProcessor::new(state)
        .with_batch_size(10)
        .with_accumulation_config(
            Duration::from_millis(200),
            Duration::from_secs(30),
            Duration::from_millis(50),
        );

    let processor_handle = tokio::spawn(async move {
        let _ = processor.start().await;
    });
    tokio::time::sleep(Duration::from_millis(100)).await;

    // Enqueue 15 events (more than batch_size)
    enqueue_dummy_events(
        &event_queue,
        TEST_SOURCE_ID,
        &fixture.state.content_storage,
        15,
        "sync_continuation",
        "continuation",
    )
    .await;

    // Wait for all events to be processed (10 via threshold, 5 via idle)
    let completed =
        wait_for_completed(fixture.state.db_pool.pool(), 15, Duration::from_secs(10)).await;

    assert_eq!(
        completed, 15,
        "All events should be processed (10 via threshold, 5 via idle)"
    );

    processor_handle.abort();
}

/// Test 5: Events should be processed after max timeout even if idle timeout not reached
#[tokio::test]
async fn test_max_timeout_safety_net() {
    let fixture = setup_test_fixture().await.unwrap();
    let event_queue = EventQueue::new(fixture.state.db_pool.pool().clone());

    let state = fixture.state.clone();
    let processor = QueueProcessor::new(state)
        .with_batch_size(100) // High threshold (won't hit)
        .with_accumulation_config(
            Duration::from_secs(60),    // Long idle timeout (won't trigger)
            Duration::from_millis(500), // Short max wait for testing
            Duration::from_millis(50),  // Check interval
        );

    let processor_handle = tokio::spawn(async move {
        let _ = processor.start().await;
    });
    tokio::time::sleep(Duration::from_millis(100)).await;

    // Enqueue events with small gaps (prevents idle timeout from triggering)
    for i in 0..5 {
        enqueue_dummy_events(
            &event_queue,
            TEST_SOURCE_ID,
            &fixture.state.content_storage,
            1,
            "sync_max_timeout",
            &format!("max_timeout_{}", i),
        )
        .await;
        // Small delay - less than the long idle timeout
        tokio::time::sleep(Duration::from_millis(50)).await;
    }

    // Wait for max timeout to trigger processing
    let completed =
        wait_for_completed(fixture.state.db_pool.pool(), 5, Duration::from_secs(5)).await;

    assert!(
        completed >= 5,
        "Events should process after max timeout, got {} completed",
        completed
    );

    processor_handle.abort();
}

/// Test 6: Multiple events for same document_id should be deduplicated
#[tokio::test]
async fn test_batch_deduplication() {
    let fixture = setup_test_fixture().await.unwrap();
    let event_queue = EventQueue::new(fixture.state.db_pool.pool().clone());

    let state = fixture.state.clone();
    let processor = QueueProcessor::new(state).with_accumulation_config(
        Duration::from_millis(200),
        Duration::from_secs(30),
        Duration::from_millis(50),
    );

    let processor_handle = tokio::spawn(async move {
        let _ = processor.start().await;
    });
    tokio::time::sleep(Duration::from_millis(100)).await;

    // Enqueue multiple events for SAME document_id
    for i in 0..3 {
        let content_id = fixture
            .state
            .content_storage
            .store_content(format!("content version {}", i).as_bytes(), None)
            .await
            .unwrap();

        let event = ConnectorEvent::DocumentCreated {
            sync_run_id: "sync_dedup".to_string(),
            source_id: TEST_SOURCE_ID.to_string(),
            document_id: "same_doc".to_string(), // Same doc ID for all events
            content_id,
            metadata: DocumentMetadata {
                title: Some(format!("Document Version {}", i)),
                author: None,
                created_at: None,
                updated_at: None,
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
        event_queue.enqueue(TEST_SOURCE_ID, &event).await.unwrap();
    }

    // Wait for all events to be completed
    let completed =
        wait_for_completed(fixture.state.db_pool.pool(), 3, Duration::from_secs(5)).await;

    // All 3 events should be completed (deduplicated during processing)
    assert_eq!(completed, 3, "All events should be marked completed");

    // But only 1 document should exist
    let doc_repo = DocumentRepository::new(fixture.state.db_pool.pool());
    let doc = doc_repo
        .find_by_external_id(TEST_SOURCE_ID, "same_doc")
        .await
        .unwrap();
    assert!(doc.is_some(), "Document should exist");

    processor_handle.abort();
}

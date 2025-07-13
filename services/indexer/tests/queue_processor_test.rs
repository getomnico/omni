mod common;

use clio_indexer::QueueProcessor;
use shared::db::repositories::DocumentRepository;
use shared::models::{ConnectorEvent, DocumentMetadata, DocumentPermissions};
use shared::queue::EventQueue;
use sqlx::types::time::OffsetDateTime;
use std::collections::HashMap;
use tokio::time::{sleep, timeout, Duration};

#[tokio::test]
async fn test_queue_processor_document_created() {
    let fixture = common::setup_test_fixture().await.unwrap();

    let processor = QueueProcessor::new(fixture.state.clone());
    let event_queue = EventQueue::new(fixture.state.db_pool.pool().clone());

    let processor_handle = tokio::spawn(async move { processor.start().await });

    // Give processor time to start
    sleep(Duration::from_millis(100)).await;

    let source_id = "01JGF7V3E0Y2R1X8P5Q7W9T4N7"; // Use the source ID from seed data
    let doc_id = "doc_123";

    // Create content in content storage
    let content_id = fixture
        .state
        .content_storage
        .store_content("This is content from a connector event".as_bytes())
        .await
        .unwrap();

    let event = ConnectorEvent::DocumentCreated {
        sync_run_id: "test_sync_run_created".to_string(),
        source_id: source_id.to_string(),
        document_id: doc_id.to_string(),
        content_id: content_id,
        metadata: DocumentMetadata {
            title: Some("Event Document".to_string()),
            author: Some("Event Author".to_string()),
            created_at: Some(OffsetDateTime::now_utc()),
            updated_at: Some(OffsetDateTime::now_utc()),
            mime_type: Some("text/plain".to_string()),
            size: Some("1024".to_string()),
            url: Some("https://example.com/doc".to_string()),
            path: Some("/docs/event_document".to_string()),
            extra: Some(HashMap::new()),
        },
        permissions: DocumentPermissions {
            public: false,
            users: vec!["user1".to_string(), "user2".to_string()],
            groups: vec!["group1".to_string()],
        },
    };

    // Queue the event using PostgreSQL queue
    event_queue.enqueue(&source_id, &event).await.unwrap();

    let repo = DocumentRepository::new(fixture.state.db_pool.pool());
    let document =
        common::wait_for_document_exists(&repo, source_id, doc_id, Duration::from_secs(5))
            .await
            .expect("Document should be created");

    assert_eq!(document.title, "Event Document");

    // Verify content is stored correctly
    let stored_content_bytes = fixture
        .state
        .content_storage
        .get_content(&document.content_id.unwrap())
        .await
        .unwrap();
    let stored_content = String::from_utf8(stored_content_bytes).unwrap();
    assert_eq!(stored_content, "This is content from a connector event");
    assert!(document.last_indexed_at > document.created_at);

    let metadata = document.metadata.as_object().unwrap();
    assert_eq!(metadata["author"].as_str().unwrap(), "Event Author");
    assert_eq!(metadata["mime_type"].as_str().unwrap(), "text/plain");

    let permissions = document.permissions.as_object().unwrap();
    assert_eq!(permissions["public"].as_bool().unwrap(), false);
    assert_eq!(permissions["users"].as_array().unwrap().len(), 2);

    processor_handle.abort();
}

#[tokio::test]
async fn test_queue_processor_document_updated() {
    let fixture = common::setup_test_fixture().await.unwrap();

    let processor = QueueProcessor::new(fixture.state.clone());
    let event_queue = EventQueue::new(fixture.state.db_pool.pool().clone());

    let processor_handle = tokio::spawn(async move { processor.start().await });

    sleep(Duration::from_millis(200)).await;

    let source_id = "01JGF7V3E0Y2R1X8P5Q7W9T4N7"; // Use the source ID from seed data
    let doc_id = "doc_456";

    // Create content in content storage for initial document
    let create_content_id = fixture
        .state
        .content_storage
        .store_content("Initial content".as_bytes())
        .await
        .unwrap();

    let create_event = ConnectorEvent::DocumentCreated {
        sync_run_id: "test_sync_run_updated".to_string(),
        source_id: source_id.to_string(),
        document_id: doc_id.to_string(),
        content_id: create_content_id,
        metadata: DocumentMetadata {
            title: Some("Initial Title".to_string()),
            author: Some("Initial Author".to_string()),
            created_at: Some(OffsetDateTime::now_utc()),
            updated_at: Some(OffsetDateTime::now_utc()),
            mime_type: Some("text/plain".to_string()),
            size: Some("500".to_string()),
            url: None,
            path: Some("/docs/initial_document".to_string()),
            extra: Some(HashMap::new()),
        },
        permissions: DocumentPermissions {
            public: false,
            users: vec!["user1".to_string()],
            groups: vec![],
        },
    };

    // Queue the create event using PostgreSQL queue
    event_queue
        .enqueue(&source_id, &create_event)
        .await
        .unwrap();

    // Wait for document to be created before updating
    let repo = DocumentRepository::new(fixture.state.db_pool.pool());
    let _initial_doc =
        common::wait_for_document_exists(&repo, source_id, doc_id, Duration::from_secs(5))
            .await
            .expect("Initial document should be created");

    // Create content in content storage for updated document
    let update_content_id = fixture
        .state
        .content_storage
        .store_content("Updated content with more information".as_bytes())
        .await
        .unwrap();

    let update_event = ConnectorEvent::DocumentUpdated {
        sync_run_id: "test_sync_run_updated".to_string(),
        source_id: source_id.to_string(),
        document_id: doc_id.to_string(),
        content_id: update_content_id,
        metadata: DocumentMetadata {
            title: Some("Updated Title".to_string()),
            author: Some("Updated Author".to_string()),
            created_at: None,
            updated_at: Some(OffsetDateTime::now_utc()),
            mime_type: Some("text/markdown".to_string()),
            size: Some("1500".to_string()),
            url: Some("https://example.com/updated".to_string()),
            path: Some("/docs/updated_document".to_string()),
            extra: Some(HashMap::new()),
        },
        permissions: Some(DocumentPermissions {
            public: true,
            users: vec![
                "user1".to_string(),
                "user2".to_string(),
                "user3".to_string(),
            ],
            groups: vec!["admin".to_string()],
        }),
    };

    // Queue the update event using PostgreSQL queue
    event_queue
        .enqueue(&source_id, &update_event)
        .await
        .unwrap();

    // Wait for document to be updated (check for updated title)
    let updated_document = timeout(Duration::from_secs(5), async {
        loop {
            if let Ok(Some(doc)) = repo.find_by_external_id(source_id, doc_id).await {
                if doc.title == "Updated Title" {
                    return doc;
                }
            }
            sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("Document should be updated");

    let document = updated_document;

    assert_eq!(document.title, "Updated Title");

    // Verify updated content is stored correctly
    let updated_content_bytes = fixture
        .state
        .content_storage
        .get_content(&document.content_id.unwrap())
        .await
        .unwrap();
    let updated_content = String::from_utf8(updated_content_bytes).unwrap();
    assert_eq!(updated_content, "Updated content with more information");

    let metadata = document.metadata.as_object().unwrap();
    assert_eq!(metadata["author"].as_str().unwrap(), "Updated Author");
    assert_eq!(metadata["mime_type"].as_str().unwrap(), "text/markdown");
    assert_eq!(metadata["size"].as_i64().unwrap(), 1500);

    let permissions = document.permissions.as_object().unwrap();
    assert_eq!(permissions["public"].as_bool().unwrap(), true);
    assert_eq!(permissions["users"].as_array().unwrap().len(), 3);
    assert_eq!(permissions["groups"].as_array().unwrap().len(), 1);

    processor_handle.abort();
}

#[tokio::test]
async fn test_queue_processor_document_deleted() {
    let fixture = common::setup_test_fixture().await.unwrap();

    let processor = QueueProcessor::new(fixture.state.clone());
    let event_queue = EventQueue::new(fixture.state.db_pool.pool().clone());

    let processor_handle = tokio::spawn(async move { processor.start().await });

    sleep(Duration::from_millis(200)).await;

    let source_id = "01JGF7V3E0Y2R1X8P5Q7W9T4N7"; // Use the source ID from seed data
    let doc_id = "doc_789";

    // Create content in content storage for delete test
    let delete_content_id = fixture
        .state
        .content_storage
        .store_content("Content to be deleted".as_bytes())
        .await
        .unwrap();

    let create_event = ConnectorEvent::DocumentCreated {
        sync_run_id: "test_sync_run_deleted".to_string(),
        source_id: source_id.to_string(),
        document_id: doc_id.to_string(),
        content_id: delete_content_id,
        metadata: DocumentMetadata {
            title: Some("Delete Me".to_string()),
            author: Some("Test Author".to_string()),
            created_at: Some(OffsetDateTime::now_utc()),
            updated_at: Some(OffsetDateTime::now_utc()),
            mime_type: Some("text/plain".to_string()),
            size: Some("100".to_string()),
            url: None,
            path: Some("/docs/delete_test_document".to_string()),
            extra: Some(HashMap::new()),
        },
        permissions: DocumentPermissions {
            public: false,
            users: vec![],
            groups: vec![],
        },
    };

    // Queue the create event using PostgreSQL queue
    event_queue
        .enqueue(&source_id, &create_event)
        .await
        .unwrap();

    let repo = DocumentRepository::new(fixture.state.db_pool.pool());
    let _document =
        common::wait_for_document_exists(&repo, source_id, doc_id, Duration::from_secs(5))
            .await
            .expect("Document should be created");

    let delete_event = ConnectorEvent::DocumentDeleted {
        sync_run_id: "test_sync_run_deleted".to_string(),
        source_id: source_id.to_string(),
        document_id: doc_id.to_string(),
    };

    // Queue the delete event using PostgreSQL queue
    event_queue
        .enqueue(&source_id, &delete_event)
        .await
        .unwrap();

    common::wait_for_document_deleted(&repo, source_id, doc_id, Duration::from_secs(5))
        .await
        .expect("Document should be deleted");

    processor_handle.abort();
}

#[tokio::test]
async fn test_queue_processor_multiple_events() {
    let fixture = common::setup_test_fixture().await.unwrap();

    let processor = QueueProcessor::new(fixture.state.clone());
    let event_queue = EventQueue::new(fixture.state.db_pool.pool().clone());

    let processor_handle = tokio::spawn(async move { processor.start().await });

    sleep(Duration::from_millis(200)).await;

    let source_id = "01JGF7V3E0Y2R1X8P5Q7W9T4N7"; // Use the source ID from seed data

    for i in 0..5 {
        // Create content in content storage for each document
        let multi_content_id = fixture
            .state
            .content_storage
            .store_content(format!("Content for document {}", i).as_bytes())
            .await
            .unwrap();

        let event = ConnectorEvent::DocumentCreated {
            sync_run_id: format!("test_sync_run_multi_{}", i),
            source_id: source_id.to_string(),
            document_id: format!("multi_doc_{}", i),
            content_id: multi_content_id,
            metadata: DocumentMetadata {
                title: Some(format!("Document {}", i)),
                author: Some("Batch Author".to_string()),
                created_at: Some(OffsetDateTime::now_utc()),
                updated_at: Some(OffsetDateTime::now_utc()),
                mime_type: Some("text/plain".to_string()),
                size: Some((100 * (i + 1)).to_string()),
                url: None,
                path: Some(format!("/docs/multi_document_{}", i)),
                extra: Some(HashMap::new()),
            },
            permissions: DocumentPermissions {
                public: i % 2 == 0,
                users: vec![format!("user{}", i)],
                groups: vec![],
            },
        };

        // Queue each event using PostgreSQL queue
        event_queue.enqueue(&source_id, &event).await.unwrap();
    }

    let repo = DocumentRepository::new(fixture.state.db_pool.pool());

    // Wait for all documents to be created
    for i in 0..5 {
        let document = common::wait_for_document_exists(
            &repo,
            source_id,
            &format!("multi_doc_{}", i),
            Duration::from_secs(5),
        )
        .await
        .expect(&format!("Document {} should exist", i));

        assert_eq!(document.title, format!("Document {}", i));

        // For the first document, verify content is stored correctly
        if i == 0 && document.content_id.is_some() {
            let stored_content_bytes = fixture
                .state
                .content_storage
                .get_content(&document.content_id.unwrap())
                .await
                .unwrap();
            let stored_content = String::from_utf8(stored_content_bytes).unwrap();
            assert_eq!(stored_content, format!("Content for document {}", i));
        }

        let permissions = document.permissions.as_object().unwrap();
        assert_eq!(permissions["public"].as_bool().unwrap(), i % 2 == 0);
    }

    processor_handle.abort();
}

#[tokio::test]
async fn test_queue_processor_batch_processing() {
    let fixture = common::setup_test_fixture().await.unwrap();

    let processor = QueueProcessor::new(fixture.state.clone());
    let event_queue = EventQueue::new(fixture.state.db_pool.pool().clone());

    let processor_handle = tokio::spawn(async move { processor.start().await });

    sleep(Duration::from_millis(200)).await;

    let source_id = "01JGF7V3E0Y2R1X8P5Q7W9T4N7"; // Use the source ID from seed data

    // Queue multiple events rapidly to test batch processing
    for i in 0..15 {
        // Create content in content storage for each batch document
        let batch_content_id = fixture
            .state
            .content_storage
            .store_content(format!("Batch content {}", i).as_bytes())
            .await
            .unwrap();

        let event = ConnectorEvent::DocumentCreated {
            sync_run_id: format!("test_sync_run_batch_{}", i),
            source_id: source_id.to_string(),
            document_id: format!("batch_doc_{}", i),
            content_id: batch_content_id,
            metadata: DocumentMetadata {
                title: Some(format!("Batch Document {}", i)),
                author: Some("Batch Author".to_string()),
                created_at: Some(OffsetDateTime::now_utc()),
                updated_at: Some(OffsetDateTime::now_utc()),
                mime_type: Some("text/plain".to_string()),
                size: Some("100".to_string()),
                url: None,
                path: Some(format!("/docs/batch_document_{}", i)),
                extra: Some(HashMap::new()),
            },
            permissions: DocumentPermissions {
                public: true,
                users: vec![],
                groups: vec![],
            },
        };

        event_queue.enqueue(&source_id, &event).await.unwrap();
    }

    let repo = DocumentRepository::new(fixture.state.db_pool.pool());

    // Wait for all documents to be processed
    for i in 0..15 {
        let document = common::wait_for_document_exists(
            &repo,
            source_id,
            &format!("batch_doc_{}", i),
            Duration::from_secs(10),
        )
        .await
        .expect(&format!("Batch document {} should exist", i));

        assert_eq!(document.title, format!("Batch Document {}", i));
    }

    processor_handle.abort();
}

#[tokio::test]
async fn test_queue_recovery_on_startup() {
    let fixture = common::setup_test_fixture().await.unwrap();
    let event_queue = EventQueue::new(fixture.state.db_pool.pool().clone());
    let source_id = "01JGF7V3E0Y2R1X8P5Q7W9T4N7";

    // Create content in content storage for recovery test
    let recovery_content_id = fixture
        .state
        .content_storage
        .store_content("Recovery test content".as_bytes())
        .await
        .unwrap();

    // Manually insert some events directly into the processing state to simulate a restart scenario
    let event = ConnectorEvent::DocumentCreated {
        sync_run_id: "test_sync_run_recovery".to_string(),
        source_id: source_id.to_string(),
        document_id: "recovery_doc_1".to_string(),
        content_id: recovery_content_id,
        metadata: DocumentMetadata {
            title: Some("Recovery Document".to_string()),
            author: Some("Recovery Author".to_string()),
            created_at: Some(OffsetDateTime::now_utc()),
            updated_at: Some(OffsetDateTime::now_utc()),
            mime_type: Some("text/plain".to_string()),
            size: Some("1024".to_string()),
            url: None,
            path: Some("/docs/recovery_document".to_string()),
            extra: Some(HashMap::new()),
        },
        permissions: DocumentPermissions {
            public: false,
            users: vec!["user1".to_string()],
            groups: vec![],
        },
    };

    // Queue the event normally first
    let event_id = event_queue.enqueue(&source_id, &event).await.unwrap();

    // Manually set the event to processing state with an old timestamp to simulate stale processing
    sqlx::query(
        r#"
        UPDATE connector_events_queue 
        SET status = 'processing', 
            processing_started_at = NOW() - INTERVAL '10 minutes'
        WHERE id = $1
        "#,
    )
    .bind(&event_id)
    .execute(fixture.state.db_pool.pool())
    .await
    .unwrap();

    // Verify the event is in processing state
    let stats = event_queue.get_queue_stats().await.unwrap();
    assert_eq!(stats.processing, 1);
    assert_eq!(stats.pending, 0);

    // Test recovery method directly
    let recovered = event_queue
        .recover_stale_processing_items(300)
        .await
        .unwrap();
    assert_eq!(recovered, 1);

    // Verify the event is back to pending state
    let stats_after = event_queue.get_queue_stats().await.unwrap();
    assert_eq!(stats_after.processing, 0);
    assert_eq!(stats_after.pending, 1);

    // Now start the processor to verify it can process the recovered event
    let processor = QueueProcessor::new(fixture.state.clone());
    let processor_handle = tokio::spawn(async move { processor.start().await });

    sleep(Duration::from_millis(100)).await;

    // Wait for the document to be processed
    let repo = DocumentRepository::new(fixture.state.db_pool.pool());
    let document = common::wait_for_document_exists(
        &repo,
        source_id,
        "recovery_doc_1",
        Duration::from_secs(5),
    )
    .await
    .expect("Recovered document should be processed");

    assert_eq!(document.title, "Recovery Document");

    // Verify content is stored correctly
    let stored_content_bytes = fixture
        .state
        .content_storage
        .get_content(&document.content_id.unwrap())
        .await
        .unwrap();
    let stored_content = String::from_utf8(stored_content_bytes).unwrap();
    assert_eq!(stored_content, "Recovery test content");

    processor_handle.abort();
}

#[tokio::test]
async fn test_embedding_queue_recovery() {
    let fixture = common::setup_test_fixture().await.unwrap();
    let embedding_queue = shared::EmbeddingQueue::new(fixture.state.db_pool.pool().clone());

    // Add a document to the embedding queue
    let document_id = "test_doc_for_embedding";
    let _content = "This is test content for embedding recovery";

    let queue_id = embedding_queue
        .enqueue(document_id.to_string())
        .await
        .unwrap();

    // Manually set the item to processing state with an old timestamp
    sqlx::query(
        r#"
        UPDATE embedding_queue 
        SET status = 'processing', 
            processing_started_at = CURRENT_TIMESTAMP - INTERVAL '10 minutes'
        WHERE id = $1
        "#,
    )
    .bind(&queue_id)
    .execute(fixture.state.db_pool.pool())
    .await
    .unwrap();

    // Verify the item is in processing state
    let stats = embedding_queue.get_queue_stats().await.unwrap();
    assert_eq!(stats.processing, 1);
    assert_eq!(stats.pending, 0);

    // Test recovery method
    let recovered = embedding_queue
        .recover_stale_processing_items(300)
        .await
        .unwrap();
    assert_eq!(recovered, 1);

    // Verify the item is back to pending state
    let stats_after = embedding_queue.get_queue_stats().await.unwrap();
    assert_eq!(stats_after.processing, 0);
    assert_eq!(stats_after.pending, 1);
}

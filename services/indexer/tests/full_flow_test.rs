mod common;

use omni_indexer::QueueProcessor;
use shared::db::repositories::DocumentRepository;
use shared::models::{ConnectorEvent, DocumentMetadata, DocumentPermissions};
use shared::queue::EventQueue;
use std::collections::HashMap;
use tokio::time::{sleep, Duration};

#[tokio::test]
async fn test_full_indexing_flow() {
    let fixture = common::setup_test_fixture().await.unwrap();
    let server = axum_test::TestServer::new(fixture.app().clone()).unwrap();

    // Start queue processor
    let processor = QueueProcessor::new(fixture.state.clone());
    let event_queue = EventQueue::new(fixture.state.db_pool.pool().clone());
    let processor_handle = tokio::spawn(async move { processor.start().await });

    // Give processor time to start
    sleep(Duration::from_millis(100)).await;

    let source_id = "01JGF7V3E0Y2R1X8P5Q7W9T4N7"; // Use the source ID from seed data

    // 1. Create document via event
    let create_content_id = fixture
        .state
        .content_storage
        .store_content("This is a complete flow test document".as_bytes())
        .await
        .unwrap();

    let create_event = ConnectorEvent::DocumentCreated {
        sync_run_id: "test_sync_run_1".to_string(),
        source_id: source_id.to_string(),
        document_id: "flow_doc_1".to_string(),
        content_id: create_content_id,
        metadata: DocumentMetadata {
            title: Some("Flow Test Document".to_string()),
            author: Some("Integration Test".to_string()),
            created_at: Some(sqlx::types::time::OffsetDateTime::now_utc()),
            updated_at: Some(sqlx::types::time::OffsetDateTime::now_utc()),
            mime_type: Some("text/plain".to_string()),
            size: Some("1024".to_string()),
            url: Some("https://example.com/flow-test".to_string()),
            path: Some("/docs/flow_test_document".to_string()),
            extra: Some(HashMap::from([
                ("category".to_string(), serde_json::json!("test")),
                ("priority".to_string(), serde_json::json!("high")),
            ])),
        },
        permissions: DocumentPermissions {
            public: false,
            users: vec!["test_user".to_string()],
            groups: vec!["test_group".to_string()],
        },
    };

    // Queue the create event using PostgreSQL queue
    event_queue
        .enqueue(&source_id, &create_event)
        .await
        .unwrap();

    // Wait for document to be processed
    let repo = DocumentRepository::new(fixture.state.db_pool.pool());
    let document =
        common::wait_for_document_exists(&repo, source_id, "flow_doc_1", Duration::from_secs(5))
            .await
            .expect("Document should be created via event processing");

    // 2. Verify document was created correctly
    assert_eq!(document.title, "Flow Test Document");

    // Verify content is stored correctly
    let stored_content_bytes = fixture
        .state
        .content_storage
        .get_content(&document.content_id.unwrap())
        .await
        .unwrap();
    let stored_content = String::from_utf8(stored_content_bytes).unwrap();
    assert_eq!(stored_content, "This is a complete flow test document");
    assert_eq!(document.source_id, source_id);
    assert_eq!(document.external_id, "flow_doc_1");

    let metadata = document.metadata.as_object().unwrap();
    assert_eq!(metadata["author"].as_str().unwrap(), "Integration Test");
    assert_eq!(metadata["category"].as_str().unwrap(), "test");
    assert_eq!(metadata["priority"].as_str().unwrap(), "high");

    let permissions = document.permissions.as_object().unwrap();
    assert_eq!(permissions["public"].as_bool().unwrap(), false);
    assert_eq!(permissions["users"].as_array().unwrap().len(), 1);
    assert_eq!(permissions["groups"].as_array().unwrap().len(), 1);

    // 3. Test document retrieval via API
    let response = server.get(&format!("/documents/{}", document.id)).await;

    assert_eq!(response.status_code(), 200);

    let returned_doc: shared::models::Document = response.json();
    assert_eq!(returned_doc.id, document.id);
    assert_eq!(returned_doc.title, "Flow Test Document");

    // 4. Update document via event
    let update_content_id = fixture
        .state
        .content_storage
        .store_content("This is updated content for the flow test".as_bytes())
        .await
        .unwrap();

    let update_event = ConnectorEvent::DocumentUpdated {
        sync_run_id: "test_sync_run_1".to_string(),
        source_id: source_id.to_string(),
        document_id: "flow_doc_1".to_string(),
        content_id: update_content_id,
        metadata: DocumentMetadata {
            title: Some("Updated Flow Test Document".to_string()),
            author: Some("Integration Test Updated".to_string()),
            created_at: None,
            updated_at: Some(sqlx::types::time::OffsetDateTime::now_utc()),
            mime_type: Some("text/markdown".to_string()),
            size: Some("2048".to_string()),
            url: Some("https://example.com/flow-test-updated".to_string()),
            path: Some("/docs/updated_flow_test_document".to_string()),
            extra: Some(HashMap::from([
                ("category".to_string(), serde_json::json!("test")),
                ("priority".to_string(), serde_json::json!("medium")),
                ("status".to_string(), serde_json::json!("updated")),
            ])),
        },
        permissions: Some(DocumentPermissions {
            public: true,
            users: vec!["test_user".to_string(), "admin_user".to_string()],
            groups: vec!["test_group".to_string(), "admin_group".to_string()],
        }),
    };

    // Queue the update event using PostgreSQL queue
    event_queue
        .enqueue(&source_id, &update_event)
        .await
        .unwrap();

    // Wait for document to be updated
    let updated_document = common::wait_for_document_with_title(
        &repo,
        source_id,
        "flow_doc_1",
        "Updated Flow Test Document",
        Duration::from_secs(5),
    )
    .await
    .expect("Document should be updated via event processing");

    // 5. Verify document was updated correctly
    assert_eq!(updated_document.title, "Updated Flow Test Document");

    // Verify updated content is stored correctly
    let updated_content_bytes = fixture
        .state
        .content_storage
        .get_content(&updated_document.content_id.unwrap())
        .await
        .unwrap();
    let updated_content = String::from_utf8(updated_content_bytes).unwrap();
    assert_eq!(updated_content, "This is updated content for the flow test");
    assert_eq!(updated_document.id, document.id); // Same document ID

    let updated_metadata = updated_document.metadata.as_object().unwrap();
    assert_eq!(
        updated_metadata["author"].as_str().unwrap(),
        "Integration Test Updated"
    );
    assert_eq!(updated_metadata["priority"].as_str().unwrap(), "medium");
    assert_eq!(updated_metadata["status"].as_str().unwrap(), "updated");

    let updated_permissions = updated_document.permissions.as_object().unwrap();
    assert_eq!(updated_permissions["public"].as_bool().unwrap(), true);
    assert_eq!(updated_permissions["users"].as_array().unwrap().len(), 2);
    assert_eq!(updated_permissions["groups"].as_array().unwrap().len(), 2);

    // 6. Test updated document retrieval via API
    let updated_response = server.get(&format!("/documents/{}", document.id)).await;

    assert_eq!(updated_response.status_code(), 200);

    let updated_returned_doc: shared::models::Document = updated_response.json();
    assert_eq!(updated_returned_doc.title, "Updated Flow Test Document");

    // TODO: API content field check disabled during content storage migration
    // assert_eq!(
    //     updated_returned_doc.content,
    //     Some("This is updated content for the flow test".to_string())
    // );

    // 7. Delete document via event
    let delete_event = ConnectorEvent::DocumentDeleted {
        sync_run_id: "test_sync_run_1".to_string(),
        source_id: source_id.to_string(),
        document_id: "flow_doc_1".to_string(),
    };

    // Queue the delete event using PostgreSQL queue
    event_queue
        .enqueue(&source_id, &delete_event)
        .await
        .unwrap();

    // Wait for document to be deleted
    common::wait_for_document_deleted(&repo, source_id, "flow_doc_1", Duration::from_secs(5))
        .await
        .expect("Document should be deleted via event processing");

    // 8. Verify document is no longer accessible via API
    let deleted_response = server.get(&format!("/documents/{}", document.id)).await;

    assert_eq!(deleted_response.status_code(), 404);

    processor_handle.abort();
}

#[tokio::test]
async fn test_concurrent_event_processing() {
    let fixture = common::setup_test_fixture().await.unwrap();

    // Start queue processor
    let processor = QueueProcessor::new(fixture.state.clone());
    let event_queue = EventQueue::new(fixture.state.db_pool.pool().clone());
    let processor_handle = tokio::spawn(async move { processor.start().await });

    // Give processor time to start
    sleep(Duration::from_millis(100)).await;

    let source_id = "01JGF7V3E0Y2R1X8P5Q7W9T4N7"; // Use the source ID from seed data

    // Queue multiple events concurrently
    let mut handles = vec![];
    for i in 0..20 {
        let queue = event_queue.clone();
        let src_id = source_id.to_string();
        let content_storage = fixture.state.content_storage.clone();
        let handle = tokio::spawn(async move {
            // Create content in content storage
            let content_id = content_storage
                .store_content(format!("Concurrent content {}", i).as_bytes())
                .await
                .unwrap();

            let event = ConnectorEvent::DocumentCreated {
                sync_run_id: format!("concurrent_sync_run_{}", i),
                source_id: src_id.clone(),
                document_id: format!("concurrent_doc_{}", i),
                content_id: content_id,
                metadata: DocumentMetadata {
                    title: Some(format!("Concurrent Document {}", i)),
                    author: Some("Concurrent Test".to_string()),
                    created_at: Some(sqlx::types::time::OffsetDateTime::now_utc()),
                    updated_at: Some(sqlx::types::time::OffsetDateTime::now_utc()),
                    mime_type: Some("text/plain".to_string()),
                    size: Some("100".to_string()),
                    url: None,
                    path: Some(format!("/docs/concurrent_document_{}", i)),
                    extra: Some(HashMap::new()),
                },
                permissions: DocumentPermissions {
                    public: true,
                    users: vec![],
                    groups: vec![],
                },
            };

            queue.enqueue(&src_id, &event).await.unwrap();
        });
        handles.push(handle);
    }

    // Wait for all events to be queued
    for handle in handles {
        handle.await.unwrap();
    }

    let repo = DocumentRepository::new(fixture.state.db_pool.pool());

    // Wait for all documents to be processed
    for i in 0..20 {
        let document = common::wait_for_document_exists(
            &repo,
            source_id,
            &format!("concurrent_doc_{}", i),
            Duration::from_secs(10),
        )
        .await
        .expect(&format!("Concurrent document {} should exist", i));

        assert_eq!(document.title, format!("Concurrent Document {}", i));

        // Verify content is stored correctly
        let stored_content_bytes = fixture
            .state
            .content_storage
            .get_content(&document.content_id.unwrap())
            .await
            .unwrap();
        let stored_content = String::from_utf8(stored_content_bytes).unwrap();
        assert_eq!(stored_content, format!("Concurrent content {}", i));
    }

    processor_handle.abort();
}

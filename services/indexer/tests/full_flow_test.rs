mod common;

use chrono::Utc;
use clio_indexer::events::{ConnectorEvent, DocumentMetadata, DocumentPermissions};
use clio_indexer::processor::EventProcessor;
use redis::AsyncCommands;
use shared::db::repositories::DocumentRepository;
use std::collections::HashMap;
use tokio::time::{sleep, Duration};

#[tokio::test]
async fn test_full_indexing_flow() {
    let fixture = common::setup_test_fixture().await.unwrap();
    let server = axum_test::TestServer::new(fixture.app().clone()).unwrap();

    // Start event processor
    let processor = EventProcessor::new(fixture.state.clone());
    let processor_handle = tokio::spawn(async move { processor.start().await });

    // Give processor time to start
    sleep(Duration::from_millis(100)).await;

    // Simulate connector publishing events
    let mut redis_conn = fixture
        .state
        .redis_client
        .get_multiplexed_async_connection()
        .await
        .unwrap();

    // 1. Create document via event
    let create_event = ConnectorEvent::DocumentCreated {
        source_id: "01JGF7V3E0Y2R1X8P5Q7W9T4N7".to_string(), // Use the source ID from seed data
        document_id: "flow_doc_1".to_string(),
        content: "This is a complete flow test document".to_string(),
        metadata: DocumentMetadata {
            title: Some("Flow Test Document".to_string()),
            author: Some("Integration Test".to_string()),
            created_at: Some(Utc::now()),
            updated_at: Some(Utc::now()),
            mime_type: Some("text/plain".to_string()),
            size: Some(1024),
            url: Some("https://example.com/flow-test".to_string()),
            parent_id: None,
            extra: HashMap::from([
                ("category".to_string(), serde_json::json!("test")),
                ("priority".to_string(), serde_json::json!("high")),
            ]),
        },
        permissions: DocumentPermissions {
            public: false,
            users: vec!["user1".to_string(), "user2".to_string()],
            groups: vec!["developers".to_string()],
        },
    };

    let event_json = serde_json::to_string(&create_event).unwrap();
    let _: () = redis_conn
        .publish("connector_events", event_json)
        .await
        .unwrap();

    // 2. Wait for document to be created and query via REST API
    let repo = DocumentRepository::new(fixture.state.db_pool.pool());
    let doc = common::wait_for_document_exists(
        &repo,
        "01JGF7V3E0Y2R1X8P5Q7W9T4N7",
        "flow_doc_1",
        Duration::from_secs(5),
    )
    .await
    .expect("Document should be created via event");

    let response = server.get(&format!("/documents/{}", doc.id)).await;
    assert_eq!(response.status_code(), axum::http::StatusCode::OK);

    let fetched_doc: shared::models::Document = response.json();
    assert_eq!(fetched_doc.title, "Flow Test Document");
    assert_eq!(
        fetched_doc.content,
        Some("This is a complete flow test document".to_string())
    );

    // 3. Update via REST API
    let update_request = serde_json::json!({
        "title": "Updated Flow Test Document",
        "content": "This content has been updated via REST API"
    });

    let update_response = server
        .put(&format!("/documents/{}", doc.id))
        .json(&update_request)
        .await;

    assert_eq!(update_response.status_code(), axum::http::StatusCode::OK);

    // 4. Update via event
    let update_event = ConnectorEvent::DocumentUpdated {
        source_id: "01JGF7V3E0Y2R1X8P5Q7W9T4N7".to_string(), // Use the source ID from seed data
        document_id: "flow_doc_1".to_string(),
        content: "Final content updated via event".to_string(),
        metadata: DocumentMetadata {
            title: Some("Final Flow Test Document".to_string()),
            author: Some("Event Processor".to_string()),
            created_at: None,
            updated_at: Some(Utc::now()),
            mime_type: Some("text/markdown".to_string()),
            size: Some(2048),
            url: Some("https://example.com/flow-test-updated".to_string()),
            parent_id: None,
            extra: HashMap::from([
                ("category".to_string(), serde_json::json!("test")),
                ("priority".to_string(), serde_json::json!("critical")),
                ("version".to_string(), serde_json::json!(2)),
            ]),
        },
        permissions: None,
    };

    let update_json = serde_json::to_string(&update_event).unwrap();
    let _: () = redis_conn
        .publish("connector_events", update_json)
        .await
        .unwrap();

    // 5. Wait for update and verify final state
    let final_doc = tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            let response = server.get(&format!("/documents/{}", doc.id)).await;
            let doc: shared::models::Document = response.json();
            if doc.title == "Final Flow Test Document" {
                return doc;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("Document should be updated");

    assert_eq!(final_doc.title, "Final Flow Test Document");
    assert_eq!(
        final_doc.content,
        Some("Final content updated via event".to_string())
    );

    let metadata = final_doc.metadata.as_object().unwrap();
    assert_eq!(metadata["author"].as_str().unwrap(), "Event Processor");
    assert_eq!(metadata["mime_type"].as_str().unwrap(), "text/markdown");
    let extra = metadata["extra"].as_object().unwrap();
    assert_eq!(extra["version"].as_i64().unwrap(), 2);

    // 6. Clean up via event
    let delete_event = ConnectorEvent::DocumentDeleted {
        source_id: "01JGF7V3E0Y2R1X8P5Q7W9T4N7".to_string(), // Use the source ID from seed data
        document_id: "flow_doc_1".to_string(),
    };

    let delete_json = serde_json::to_string(&delete_event).unwrap();
    let _: () = redis_conn
        .publish("connector_events", delete_json)
        .await
        .unwrap();

    // Wait for deletion and verify
    common::wait_for_document_deleted(
        &repo,
        "01JGF7V3E0Y2R1X8P5Q7W9T4N7",
        "flow_doc_1",
        Duration::from_secs(5),
    )
    .await
    .expect("Document should be deleted");

    let deleted_response = server.get(&format!("/documents/{}", doc.id)).await;
    assert_eq!(
        deleted_response.status_code(),
        axum::http::StatusCode::NOT_FOUND
    );

    processor_handle.abort();
}

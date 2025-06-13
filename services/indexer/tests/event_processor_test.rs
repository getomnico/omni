mod common;

use chrono::Utc;
use clio_indexer::events::{ConnectorEvent, DocumentMetadata, DocumentPermissions};
use clio_indexer::processor::EventProcessor;
use redis::AsyncCommands;
use shared::db::repositories::DocumentRepository;
use std::collections::HashMap;
use tokio::time::{sleep, timeout, Duration};

#[tokio::test]
async fn test_event_processor_document_created() {
    let fixture = common::setup_test_fixture().await.unwrap();

    let processor = EventProcessor::new(fixture.state.clone());

    let processor_handle = tokio::spawn(async move { processor.start().await });

    // Give processor time to start
    sleep(Duration::from_millis(100)).await;

    let source_id = "01JGF7V3E0Y2R1X8P5Q7W9T4N7"; // Use the source ID from seed data
    let doc_id = "doc_123";

    let event = ConnectorEvent::DocumentCreated {
        source_id: source_id.to_string(),
        document_id: doc_id.to_string(),
        content: "This is content from a connector event".to_string(),
        metadata: DocumentMetadata {
            title: Some("Event Document".to_string()),
            author: Some("Event Author".to_string()),
            created_at: Some(Utc::now()),
            updated_at: Some(Utc::now()),
            mime_type: Some("text/plain".to_string()),
            size: Some(1024),
            url: Some("https://example.com/doc".to_string()),
            parent_id: None,
            extra: HashMap::new(),
        },
        permissions: DocumentPermissions {
            public: false,
            users: vec!["user1".to_string(), "user2".to_string()],
            groups: vec!["group1".to_string()],
        },
    };

    let mut redis_conn = fixture
        .state
        .redis_client
        .get_multiplexed_async_connection()
        .await
        .unwrap();
    let event_json = serde_json::to_string(&event).unwrap();
    let _: () = redis_conn
        .publish("connector_events", event_json)
        .await
        .unwrap();

    let repo = DocumentRepository::new(fixture.state.db_pool.pool());
    let document =
        common::wait_for_document_exists(&repo, source_id, doc_id, Duration::from_secs(5))
            .await
            .expect("Document should be created");

    assert_eq!(document.title, "Event Document");
    assert_eq!(
        document.content,
        Some("This is content from a connector event".to_string())
    );
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
async fn test_event_processor_document_updated() {
    let fixture = common::setup_test_fixture().await.unwrap();

    let processor = EventProcessor::new(fixture.state.clone());

    let processor_handle = tokio::spawn(async move { processor.start().await });

    sleep(Duration::from_millis(200)).await;

    let source_id = "01JGF7V3E0Y2R1X8P5Q7W9T4N7"; // Use the source ID from seed data
    let doc_id = "doc_456";

    let create_event = ConnectorEvent::DocumentCreated {
        source_id: source_id.to_string(),
        document_id: doc_id.to_string(),
        content: "Initial content".to_string(),
        metadata: DocumentMetadata {
            title: Some("Initial Title".to_string()),
            author: Some("Initial Author".to_string()),
            created_at: Some(Utc::now()),
            updated_at: Some(Utc::now()),
            mime_type: Some("text/plain".to_string()),
            size: Some(500),
            url: None,
            parent_id: None,
            extra: HashMap::new(),
        },
        permissions: DocumentPermissions {
            public: false,
            users: vec!["user1".to_string()],
            groups: vec![],
        },
    };

    let mut redis_conn = fixture
        .state
        .redis_client
        .get_multiplexed_async_connection()
        .await
        .unwrap();
    let create_json = serde_json::to_string(&create_event).unwrap();
    let _: () = redis_conn
        .publish("connector_events", create_json)
        .await
        .unwrap();

    // Wait for document to be created before updating
    let repo = DocumentRepository::new(fixture.state.db_pool.pool());
    let _initial_doc =
        common::wait_for_document_exists(&repo, source_id, doc_id, Duration::from_secs(5))
            .await
            .expect("Initial document should be created");

    let update_event = ConnectorEvent::DocumentUpdated {
        source_id: source_id.to_string(),
        document_id: doc_id.to_string(),
        content: "Updated content with more information".to_string(),
        metadata: DocumentMetadata {
            title: Some("Updated Title".to_string()),
            author: Some("Updated Author".to_string()),
            created_at: None,
            updated_at: Some(Utc::now()),
            mime_type: Some("text/markdown".to_string()),
            size: Some(1500),
            url: Some("https://example.com/updated".to_string()),
            parent_id: None,
            extra: HashMap::new(),
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

    let update_json = serde_json::to_string(&update_event).unwrap();
    let _: () = redis_conn
        .publish("connector_events", update_json)
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
    assert_eq!(
        document.content,
        Some("Updated content with more information".to_string())
    );

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
async fn test_event_processor_document_deleted() {
    let fixture = common::setup_test_fixture().await.unwrap();

    let processor = EventProcessor::new(fixture.state.clone());

    let processor_handle = tokio::spawn(async move { processor.start().await });

    sleep(Duration::from_millis(200)).await;

    let source_id = "01JGF7V3E0Y2R1X8P5Q7W9T4N7"; // Use the source ID from seed data
    let doc_id = "doc_789";

    let create_event = ConnectorEvent::DocumentCreated {
        source_id: source_id.to_string(),
        document_id: doc_id.to_string(),
        content: "Content to be deleted".to_string(),
        metadata: DocumentMetadata {
            title: Some("Delete Me".to_string()),
            author: Some("Test Author".to_string()),
            created_at: Some(Utc::now()),
            updated_at: Some(Utc::now()),
            mime_type: Some("text/plain".to_string()),
            size: Some(100),
            url: None,
            parent_id: None,
            extra: HashMap::new(),
        },
        permissions: DocumentPermissions {
            public: false,
            users: vec![],
            groups: vec![],
        },
    };

    let mut redis_conn = fixture
        .state
        .redis_client
        .get_multiplexed_async_connection()
        .await
        .unwrap();
    let create_json = serde_json::to_string(&create_event).unwrap();
    let _: () = redis_conn
        .publish("connector_events", create_json)
        .await
        .unwrap();

    let repo = DocumentRepository::new(fixture.state.db_pool.pool());
    let _document =
        common::wait_for_document_exists(&repo, source_id, doc_id, Duration::from_secs(5))
            .await
            .expect("Document should be created");

    let delete_event = ConnectorEvent::DocumentDeleted {
        source_id: source_id.to_string(),
        document_id: doc_id.to_string(),
    };

    let delete_json = serde_json::to_string(&delete_event).unwrap();
    let _: () = redis_conn
        .publish("connector_events", delete_json)
        .await
        .unwrap();

    common::wait_for_document_deleted(&repo, source_id, doc_id, Duration::from_secs(5))
        .await
        .expect("Document should be deleted");

    processor_handle.abort();
}

#[tokio::test]
async fn test_event_processor_multiple_events() {
    let fixture = common::setup_test_fixture().await.unwrap();

    let processor = EventProcessor::new(fixture.state.clone());

    let processor_handle = tokio::spawn(async move { processor.start().await });

    sleep(Duration::from_millis(200)).await;

    let mut redis_conn = fixture
        .state
        .redis_client
        .get_multiplexed_async_connection()
        .await
        .unwrap();

    let source_id = "01JGF7V3E0Y2R1X8P5Q7W9T4N7"; // Use the source ID from seed data

    for i in 0..5 {
        let event = ConnectorEvent::DocumentCreated {
            source_id: source_id.to_string(),
            document_id: format!("multi_doc_{}", i),
            content: format!("Content for document {}", i),
            metadata: DocumentMetadata {
                title: Some(format!("Document {}", i)),
                author: Some("Batch Author".to_string()),
                created_at: Some(Utc::now()),
                updated_at: Some(Utc::now()),
                mime_type: Some("text/plain".to_string()),
                size: Some(100 * (i + 1) as i64),
                url: None,
                parent_id: None,
                extra: HashMap::new(),
            },
            permissions: DocumentPermissions {
                public: i % 2 == 0,
                users: vec![format!("user{}", i)],
                groups: vec![],
            },
        };

        let event_json = serde_json::to_string(&event).unwrap();
        let _: () = redis_conn
            .publish("connector_events", event_json)
            .await
            .unwrap();
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
        assert_eq!(
            document.content,
            Some(format!("Content for document {}", i))
        );

        let permissions = document.permissions.as_object().unwrap();
        assert_eq!(permissions["public"].as_bool().unwrap(), i % 2 == 0);
    }

    processor_handle.abort();
}

#[tokio::test]
async fn test_event_processor_invalid_event_handling() {
    let fixture = common::setup_test_fixture().await.unwrap();

    let processor = EventProcessor::new(fixture.state.clone());

    let processor_handle = tokio::spawn(async move { processor.start().await });

    sleep(Duration::from_millis(200)).await;

    let mut redis_conn = fixture
        .state
        .redis_client
        .get_multiplexed_async_connection()
        .await
        .unwrap();

    let _: () = redis_conn
        .publish("connector_events", "invalid json")
        .await
        .unwrap();

    let source_id = "01JGF7V3E0Y2R1X8P5Q7W9T4N7"; // Use the source ID from seed data
    let doc_id = "valid_doc";

    let valid_event = ConnectorEvent::DocumentCreated {
        source_id: source_id.to_string(),
        document_id: doc_id.to_string(),
        content: "Valid content".to_string(),
        metadata: DocumentMetadata {
            title: Some("Valid Document".to_string()),
            author: None,
            created_at: Some(Utc::now()),
            updated_at: Some(Utc::now()),
            mime_type: None,
            size: None,
            url: None,
            parent_id: None,
            extra: HashMap::new(),
        },
        permissions: DocumentPermissions {
            public: true,
            users: vec![],
            groups: vec![],
        },
    };

    let valid_json = serde_json::to_string(&valid_event).unwrap();
    let _: () = redis_conn
        .publish("connector_events", valid_json)
        .await
        .unwrap();

    let repo = DocumentRepository::new(fixture.state.db_pool.pool());
    let document =
        common::wait_for_document_exists(&repo, source_id, doc_id, Duration::from_secs(5))
            .await
            .expect("Valid document should be created despite previous error");

    assert_eq!(document.title, "Valid Document");

    processor_handle.abort();
}

mod common;

use axum::http::StatusCode;
use axum_test::TestServer;
use common::TEST_SOURCE_ID;
use common::fixtures::{create_document_request, update_document_request};
use omni_indexer::{BulkDocumentOperation, BulkDocumentRequest, QueueProcessor};
use serde_json::{Value, json};
use shared::db::repositories::{DocumentRepository, GroupRepository, PersonRepository};
use shared::models::{ConnectorEvent, Document, DocumentMetadata, DocumentPermissions};
use shared::queue::EventQueue;
use sqlx::types::time::OffsetDateTime;
use std::collections::HashMap;
use tokio::time::Duration;

#[tokio::test]
async fn test_event_driven_document_lifecycle() {
    let fixture = common::setup_test_fixture().await.unwrap();
    let event_queue = EventQueue::new(fixture.state.db_pool.pool().clone());
    let repo = DocumentRepository::new(fixture.state.db_pool.pool());

    let processor =
        QueueProcessor::new(fixture.state.clone()).with_poll_interval(Duration::from_millis(200));
    let processor_handle = tokio::spawn(async move {
        let _ = processor.start().await;
    });
    tokio::time::sleep(Duration::from_millis(100)).await;

    let doc_id = "lifecycle_doc_1";

    // --- Create ---
    let content = "This is the lifecycle test document content";
    let content_id = fixture
        .state
        .content_storage
        .store_content(content.as_bytes(), None)
        .await
        .unwrap();

    let create_event = ConnectorEvent::DocumentCreated {
        sync_run_id: "sync_lifecycle".to_string(),
        source_id: TEST_SOURCE_ID.to_string(),
        document_id: doc_id.to_string(),
        content_id,
        metadata: DocumentMetadata {
            title: Some("Lifecycle Document".to_string()),
            author: Some("Test Author".to_string()),
            created_at: Some(OffsetDateTime::now_utc()),
            updated_at: Some(OffsetDateTime::now_utc()),
            content_type: None,
            mime_type: Some("application/pdf".to_string()),
            size: Some("1024".to_string()),
            url: Some("https://example.com/docs/report.pdf".to_string()),
            path: Some("/docs/lifecycle_document".to_string()),
            extra: Some(HashMap::from([("category".to_string(), json!("test"))])),
        },
        permissions: DocumentPermissions {
            public: false,
            users: vec!["user1".to_string(), "user2".to_string()],
            groups: vec!["group1".to_string()],
        },
        attributes: None,
    };

    event_queue
        .enqueue(TEST_SOURCE_ID, &create_event)
        .await
        .unwrap();

    let document =
        common::wait_for_document_exists(&repo, TEST_SOURCE_ID, doc_id, Duration::from_secs(5))
            .await
            .expect("Document should be created");

    assert_eq!(document.title, "Lifecycle Document");
    assert_eq!(document.source_id, TEST_SOURCE_ID);
    assert_eq!(document.external_id, doc_id);

    let stored_bytes = fixture
        .state
        .content_storage
        .get_content(&document.content_id.unwrap())
        .await
        .unwrap();
    assert_eq!(String::from_utf8(stored_bytes).unwrap(), content);

    let metadata = document.metadata.as_object().unwrap();
    assert_eq!(metadata["author"].as_str().unwrap(), "Test Author");
    assert_eq!(metadata["mime_type"].as_str().unwrap(), "application/pdf");
    assert_eq!(metadata["size"].as_i64().unwrap(), 1024);

    let permissions = document.permissions.as_object().unwrap();
    assert_eq!(permissions["public"].as_bool().unwrap(), false);
    assert_eq!(permissions["users"].as_array().unwrap().len(), 2);
    assert_eq!(permissions["groups"].as_array().unwrap().len(), 1);

    assert!(document.last_indexed_at > document.created_at);
    assert_eq!(document.file_extension, Some("pdf".to_string()));

    // Verify embedding queue entry was created
    common::wait_for_embedding_queue_entry(
        fixture.state.db_pool.pool(),
        &document.id,
        Duration::from_secs(5),
    )
    .await
    .expect("Embedding queue entry should exist");

    let completed =
        common::wait_for_completed(fixture.state.db_pool.pool(), 1, Duration::from_secs(5)).await;
    assert!(completed >= 1);

    // --- Update ---
    let updated_content = "This is updated lifecycle content";
    let update_content_id = fixture
        .state
        .content_storage
        .store_content(updated_content.as_bytes(), None)
        .await
        .unwrap();

    let update_event = ConnectorEvent::DocumentUpdated {
        sync_run_id: "sync_lifecycle".to_string(),
        source_id: TEST_SOURCE_ID.to_string(),
        document_id: doc_id.to_string(),
        content_id: update_content_id,
        metadata: DocumentMetadata {
            title: Some("Updated Lifecycle Document".to_string()),
            author: Some("Updated Author".to_string()),
            created_at: None,
            updated_at: Some(OffsetDateTime::now_utc()),
            content_type: None,
            mime_type: Some("text/markdown".to_string()),
            size: Some("2048".to_string()),
            url: Some("https://example.com/docs/report-v2.md".to_string()),
            path: Some("/docs/updated_lifecycle_document".to_string()),
            extra: Some(HashMap::new()),
        },
        permissions: Some(DocumentPermissions {
            public: true,
            users: vec![
                "user1".to_string(),
                "user2".to_string(),
                "user3".to_string(),
            ],
            groups: vec!["group1".to_string(), "group2".to_string()],
        }),
        attributes: None,
    };

    event_queue
        .enqueue(TEST_SOURCE_ID, &update_event)
        .await
        .unwrap();

    let updated_doc = common::wait_for_document_with_title(
        &repo,
        TEST_SOURCE_ID,
        doc_id,
        "Updated Lifecycle Document",
        Duration::from_secs(5),
    )
    .await
    .expect("Document should be updated");

    assert_eq!(updated_doc.title, "Updated Lifecycle Document");
    assert_eq!(updated_doc.id, document.id);

    let updated_bytes = fixture
        .state
        .content_storage
        .get_content(&updated_doc.content_id.unwrap())
        .await
        .unwrap();
    assert_eq!(String::from_utf8(updated_bytes).unwrap(), updated_content);

    let updated_permissions = updated_doc.permissions.as_object().unwrap();
    assert_eq!(updated_permissions["public"].as_bool().unwrap(), true);
    assert_eq!(updated_permissions["users"].as_array().unwrap().len(), 3);
    assert_eq!(updated_permissions["groups"].as_array().unwrap().len(), 2);
    assert!(updated_doc.updated_at > document.updated_at);

    // Regression: ON CONFLICT DO UPDATE must propagate denormalized columns
    // (url, file_size, file_extension, content_type), not just metadata jsonb.
    // Before this fix, the upsert only refreshed `metadata`, leaving the
    // column copies stale — searcher reads `doc.url` (the column) at
    // services/searcher/src/search.rs:87, so search hits would still link
    // to the old URL after re-emit.
    assert_eq!(
        updated_doc.url,
        Some("https://example.com/docs/report-v2.md".to_string()),
        "documents.url column must reflect the latest metadata.url after upsert"
    );
    assert_eq!(
        updated_doc.file_extension,
        Some("md".to_string()),
        "documents.file_extension column must update when URL extension changes"
    );

    sqlx::query(
        r#"
        INSERT INTO embeddings (id, document_id, chunk_index, chunk_start_offset, chunk_end_offset, embedding, model_name, dimensions)
        VALUES ('01TESTLIFECYCLEDEL000001AA', $1, 0, 0, 10, '[0.1,0.2,0.3]'::vector, 'test-model', 3)
        "#,
    )
    .bind(&updated_doc.id)
    .execute(fixture.state.db_pool.pool())
    .await
    .unwrap();

    // --- Delete ---
    let delete_event = ConnectorEvent::DocumentDeleted {
        sync_run_id: "sync_lifecycle".to_string(),
        source_id: TEST_SOURCE_ID.to_string(),
        document_id: doc_id.to_string(),
    };

    event_queue
        .enqueue(TEST_SOURCE_ID, &delete_event)
        .await
        .unwrap();

    common::wait_for_document_deleted(&repo, TEST_SOURCE_ID, doc_id, Duration::from_secs(5))
        .await
        .expect("Document should be deleted");

    let embedding_count_after_delete: (i64,) =
        sqlx::query_as("SELECT COUNT(*) FROM embeddings WHERE document_id = $1")
            .bind(&updated_doc.id)
            .fetch_one(fixture.state.db_pool.pool())
            .await
            .unwrap();
    assert_eq!(embedding_count_after_delete.0, 0);

    processor_handle.abort();
}

#[tokio::test]
async fn test_unchanged_content_upsert_does_not_requeue_existing_embeddings() {
    let fixture = common::setup_test_fixture().await.unwrap();
    let event_queue = EventQueue::new(fixture.state.db_pool.pool().clone());
    let repo = DocumentRepository::new(fixture.state.db_pool.pool());

    let processor =
        QueueProcessor::new(fixture.state.clone()).with_poll_interval(Duration::from_millis(200));
    let processor_handle = tokio::spawn(async move {
        let _ = processor.start().await;
    });
    tokio::time::sleep(Duration::from_millis(100)).await;

    let doc_id = "embedding_idempotency_doc";
    let content_id = fixture
        .state
        .content_storage
        .store_content(b"stable content", None)
        .await
        .unwrap();

    let create_event = ConnectorEvent::DocumentCreated {
        sync_run_id: "sync_embedding_idempotency".to_string(),
        source_id: TEST_SOURCE_ID.to_string(),
        document_id: doc_id.to_string(),
        content_id: content_id.clone(),
        metadata: DocumentMetadata {
            title: Some("Stable Content".to_string()),
            author: None,
            created_at: None,
            updated_at: Some(OffsetDateTime::now_utc()),
            content_type: None,
            mime_type: Some("text/plain".to_string()),
            size: Some("14".to_string()),
            url: Some("https://example.com/stable.txt".to_string()),
            path: Some("/stable.txt".to_string()),
            extra: None,
        },
        permissions: DocumentPermissions {
            public: false,
            users: vec!["user@example.com".to_string()],
            groups: vec![],
        },
        attributes: None,
    };

    event_queue
        .enqueue(TEST_SOURCE_ID, &create_event)
        .await
        .unwrap();

    let document =
        common::wait_for_document_exists(&repo, TEST_SOURCE_ID, doc_id, Duration::from_secs(5))
            .await
            .expect("Document should be created");
    common::wait_for_completed(fixture.state.db_pool.pool(), 1, Duration::from_secs(5)).await;

    sqlx::query("UPDATE embedding_queue SET status = 'completed' WHERE document_id = $1")
        .bind(&document.id)
        .execute(fixture.state.db_pool.pool())
        .await
        .unwrap();
    sqlx::query(
        r#"
        INSERT INTO embeddings (id, document_id, chunk_index, chunk_start_offset, chunk_end_offset, embedding, model_name, dimensions)
        VALUES ('emb_idempotency_existing', $1, 0, 0, 14, '[0.1,0.2,0.3]'::vector, 'test-model', 3)
        "#,
    )
    .bind(&document.id)
    .execute(fixture.state.db_pool.pool())
    .await
    .unwrap();

    let metadata_only_update = ConnectorEvent::DocumentUpdated {
        sync_run_id: "sync_embedding_idempotency".to_string(),
        source_id: TEST_SOURCE_ID.to_string(),
        document_id: doc_id.to_string(),
        content_id: content_id.clone(),
        metadata: DocumentMetadata {
            title: Some("Stable Content Renamed".to_string()),
            author: None,
            created_at: None,
            updated_at: Some(OffsetDateTime::now_utc()),
            content_type: None,
            mime_type: Some("text/plain".to_string()),
            size: Some("14".to_string()),
            url: Some("https://example.com/stable-renamed.txt".to_string()),
            path: Some("/stable-renamed.txt".to_string()),
            extra: None,
        },
        permissions: Some(DocumentPermissions {
            public: false,
            users: vec![
                "user@example.com".to_string(),
                "other@example.com".to_string(),
            ],
            groups: vec![],
        }),
        attributes: None,
    };

    event_queue
        .enqueue(TEST_SOURCE_ID, &metadata_only_update)
        .await
        .unwrap();
    common::wait_for_document_with_title(
        &repo,
        TEST_SOURCE_ID,
        doc_id,
        "Stable Content Renamed",
        Duration::from_secs(5),
    )
    .await
    .expect("Document should be updated");
    common::wait_for_completed(fixture.state.db_pool.pool(), 2, Duration::from_secs(5)).await;

    let queue_rows_after_metadata_update: (i64,) =
        sqlx::query_as("SELECT COUNT(*) FROM embedding_queue WHERE document_id = $1")
            .bind(&document.id)
            .fetch_one(fixture.state.db_pool.pool())
            .await
            .unwrap();
    assert_eq!(queue_rows_after_metadata_update.0, 1);

    let new_content_id = fixture
        .state
        .content_storage
        .store_content(b"changed content", None)
        .await
        .unwrap();
    let content_update = ConnectorEvent::DocumentUpdated {
        sync_run_id: "sync_embedding_idempotency".to_string(),
        source_id: TEST_SOURCE_ID.to_string(),
        document_id: doc_id.to_string(),
        content_id: new_content_id,
        metadata: DocumentMetadata {
            title: Some("Changed Content".to_string()),
            author: None,
            created_at: None,
            updated_at: Some(OffsetDateTime::now_utc()),
            content_type: None,
            mime_type: Some("text/plain".to_string()),
            size: Some("15".to_string()),
            url: Some("https://example.com/changed.txt".to_string()),
            path: Some("/changed.txt".to_string()),
            extra: None,
        },
        permissions: None,
        attributes: None,
    };

    event_queue
        .enqueue(TEST_SOURCE_ID, &content_update)
        .await
        .unwrap();
    common::wait_for_document_with_title(
        &repo,
        TEST_SOURCE_ID,
        doc_id,
        "Changed Content",
        Duration::from_secs(5),
    )
    .await
    .expect("Document should be updated with changed content");
    common::wait_for_completed(fixture.state.db_pool.pool(), 3, Duration::from_secs(5)).await;

    let queue_rows_after_content_update: (i64,) =
        sqlx::query_as("SELECT COUNT(*) FROM embedding_queue WHERE document_id = $1")
            .bind(&document.id)
            .fetch_one(fixture.state.db_pool.pool())
            .await
            .unwrap();
    assert_eq!(queue_rows_after_content_update.0, 2);

    let embeddings_after_content_update: (i64,) =
        sqlx::query_as("SELECT COUNT(*) FROM embeddings WHERE document_id = $1")
            .bind(&document.id)
            .fetch_one(fixture.state.db_pool.pool())
            .await
            .unwrap();
    assert_eq!(embeddings_after_content_update.0, 1);

    processor_handle.abort();
}

#[tokio::test]
async fn test_latest_document_event_wins_within_indexer_batch() {
    let fixture = common::setup_test_fixture().await.unwrap();
    let event_queue = EventQueue::new(fixture.state.db_pool.pool().clone());
    let repo = DocumentRepository::new(fixture.state.db_pool.pool());

    let doc_id = "last_event_wins_doc";
    let first_content_id = fixture
        .state
        .content_storage
        .store_content(b"first content", None)
        .await
        .unwrap();
    let second_content_id = fixture
        .state
        .content_storage
        .store_content(b"second content", None)
        .await
        .unwrap();

    let first_event = ConnectorEvent::DocumentCreated {
        sync_run_id: "sync_last_event_wins".to_string(),
        source_id: TEST_SOURCE_ID.to_string(),
        document_id: doc_id.to_string(),
        content_id: first_content_id,
        metadata: DocumentMetadata {
            title: Some("First Event".to_string()),
            author: None,
            created_at: None,
            updated_at: Some(OffsetDateTime::now_utc()),
            content_type: None,
            mime_type: Some("text/plain".to_string()),
            size: Some("13".to_string()),
            url: Some("https://example.com/first.txt".to_string()),
            path: Some("/first.txt".to_string()),
            extra: None,
        },
        permissions: DocumentPermissions {
            public: false,
            users: vec!["first@example.com".to_string()],
            groups: vec![],
        },
        attributes: None,
    };
    let second_event = ConnectorEvent::DocumentCreated {
        sync_run_id: "sync_last_event_wins".to_string(),
        source_id: TEST_SOURCE_ID.to_string(),
        document_id: doc_id.to_string(),
        content_id: second_content_id,
        metadata: DocumentMetadata {
            title: Some("Second Event".to_string()),
            author: None,
            created_at: None,
            updated_at: Some(OffsetDateTime::now_utc()),
            content_type: None,
            mime_type: Some("text/plain".to_string()),
            size: Some("14".to_string()),
            url: Some("https://example.com/second.txt".to_string()),
            path: Some("/second.txt".to_string()),
            extra: None,
        },
        permissions: DocumentPermissions {
            public: true,
            users: vec!["second@example.com".to_string()],
            groups: vec!["second-group@example.com".to_string()],
        },
        attributes: None,
    };

    let event_ids = event_queue
        .enqueue_batch(TEST_SOURCE_ID, &[first_event, second_event])
        .await
        .unwrap();
    assert_eq!(event_ids.len(), 2);

    let processor = QueueProcessor::new(fixture.state.clone())
        .with_poll_interval(Duration::from_millis(200))
        .with_batch_size(100);
    let processor_handle = tokio::spawn(async move {
        let _ = processor.start().await;
    });

    let document = common::wait_for_document_with_title(
        &repo,
        TEST_SOURCE_ID,
        doc_id,
        "Second Event",
        Duration::from_secs(5),
    )
    .await
    .expect("latest event should determine final document state");

    let stored_bytes = fixture
        .state
        .content_storage
        .get_content(&document.content_id.unwrap())
        .await
        .unwrap();
    assert_eq!(String::from_utf8(stored_bytes).unwrap(), "second content");

    assert_eq!(
        document.url,
        Some("https://example.com/second.txt".to_string())
    );
    let permissions = document.permissions.as_object().unwrap();
    assert_eq!(permissions["public"].as_bool().unwrap(), true);
    let users: Vec<_> = permissions["users"]
        .as_array()
        .unwrap()
        .iter()
        .map(|u| u.as_str().unwrap())
        .collect();
    assert_eq!(users, vec!["second@example.com"]);
    let groups: Vec<_> = permissions["groups"]
        .as_array()
        .unwrap()
        .iter()
        .map(|g| g.as_str().unwrap())
        .collect();
    assert_eq!(groups, vec!["second-group@example.com"]);

    let completed =
        common::wait_for_completed(fixture.state.db_pool.pool(), 2, Duration::from_secs(5)).await;
    assert!(completed >= 2);

    processor_handle.abort();
}

#[tokio::test]
async fn test_url_inferred_file_extension_is_capped() {
    let fixture = common::setup_test_fixture().await.unwrap();
    let event_queue = EventQueue::new(fixture.state.db_pool.pool().clone());
    let repo = DocumentRepository::new(fixture.state.db_pool.pool());

    let processor =
        QueueProcessor::new(fixture.state.clone()).with_poll_interval(Duration::from_millis(200));
    let processor_handle = tokio::spawn(async move {
        let _ = processor.start().await;
    });
    tokio::time::sleep(Duration::from_millis(100)).await;

    let long_suffix = "a".repeat(64);
    let gmail_like_url = format!(
        "https://mail.google.com/mail/u/0/#inbox/rfc822msgid%3A1577287121.{}",
        long_suffix
    );

    for (document_id, url) in [
        ("gmail_long_extension_doc", gmail_like_url.clone()),
        (
            "normal_pdf_doc",
            "https://example.com/docs/report.pdf".to_string(),
        ),
    ] {
        let content_id = fixture
            .state
            .content_storage
            .store_content(format!("content for {document_id}").as_bytes(), None)
            .await
            .unwrap();

        let event = ConnectorEvent::DocumentCreated {
            sync_run_id: "sync_file_extension_cap".to_string(),
            source_id: TEST_SOURCE_ID.to_string(),
            document_id: document_id.to_string(),
            content_id,
            metadata: DocumentMetadata {
                title: Some(document_id.to_string()),
                author: None,
                created_at: None,
                updated_at: None,
                content_type: None,
                mime_type: Some("message/rfc822".to_string()),
                size: Some("100".to_string()),
                url: Some(url),
                path: None,
                extra: None,
            },
            permissions: DocumentPermissions {
                public: false,
                users: vec!["user1".to_string()],
                groups: vec![],
            },
            attributes: None,
        };

        event_queue.enqueue(TEST_SOURCE_ID, &event).await.unwrap();
    }

    let completed =
        common::wait_for_completed(fixture.state.db_pool.pool(), 2, Duration::from_secs(5)).await;
    assert_eq!(completed, 2);

    let gmail_doc = common::wait_for_document_exists(
        &repo,
        TEST_SOURCE_ID,
        "gmail_long_extension_doc",
        Duration::from_secs(5),
    )
    .await
    .expect("Gmail-like document should be indexed");
    let gmail_extension = gmail_doc.file_extension.unwrap();
    assert_eq!(gmail_extension.chars().count(), 50);
    assert_eq!(gmail_extension, "a".repeat(50));
    assert_eq!(gmail_doc.url, Some(gmail_like_url));

    let pdf_doc = common::wait_for_document_exists(
        &repo,
        TEST_SOURCE_ID,
        "normal_pdf_doc",
        Duration::from_secs(5),
    )
    .await
    .expect("Normal PDF document should be indexed");
    assert_eq!(pdf_doc.file_extension, Some("pdf".to_string()));

    processor_handle.abort();
}

#[tokio::test]
async fn test_batch_processing_and_deduplication() {
    let fixture = common::setup_test_fixture().await.unwrap();
    let event_queue = EventQueue::new(fixture.state.db_pool.pool().clone());
    let pool = fixture.state.db_pool.pool();
    let repo = DocumentRepository::new(pool);

    let processor = QueueProcessor::new(fixture.state.clone())
        .with_batch_size(10)
        .with_poll_interval(Duration::from_millis(200));
    let processor_handle = tokio::spawn(async move {
        let _ = processor.start().await;
    });
    tokio::time::sleep(Duration::from_millis(100)).await;

    // Phase A: Deduplication — 3 events for the same document
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
            document_id: "dedup_doc".to_string(),
            content_id,
            metadata: DocumentMetadata {
                title: Some(format!("Document Version {}", i)),
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
        event_queue.enqueue(TEST_SOURCE_ID, &event).await.unwrap();
    }

    let completed = common::wait_for_completed(pool, 3, Duration::from_secs(5)).await;
    assert_eq!(completed, 3, "All 3 dedup events should be completed");

    let doc = repo
        .find_by_external_id(TEST_SOURCE_ID, "dedup_doc")
        .await
        .unwrap();
    assert!(
        doc.is_some(),
        "Exactly one document should exist after deduplication"
    );

    // Phase B: Threshold trigger — 10 events hitting batch_size
    common::enqueue_dummy_events(
        &event_queue,
        TEST_SOURCE_ID,
        &fixture.state.content_storage,
        10,
        "sync_threshold",
        "threshold",
    )
    .await;

    let completed = common::wait_for_completed(pool, 13, Duration::from_secs(5)).await;
    assert_eq!(
        completed, 13,
        "All 13 events (3 dedup + 10 threshold) should be completed"
    );

    for i in 0..10 {
        let doc = repo
            .find_by_external_id(TEST_SOURCE_ID, &format!("threshold_{}", i))
            .await
            .unwrap();
        assert!(doc.is_some(), "Threshold document {} should exist", i);
    }

    // Phase C: Continuation — 15 events exceeding batch_size
    common::enqueue_dummy_events(
        &event_queue,
        TEST_SOURCE_ID,
        &fixture.state.content_storage,
        15,
        "sync_continuation",
        "continuation",
    )
    .await;

    let completed = common::wait_for_completed(pool, 28, Duration::from_secs(10)).await;
    assert_eq!(completed, 28, "All 28 events should be completed");

    for i in 0..15 {
        let doc = repo
            .find_by_external_id(TEST_SOURCE_ID, &format!("continuation_{}", i))
            .await
            .unwrap();
        assert!(doc.is_some(), "Continuation document {} should exist", i);
    }

    processor_handle.abort();
}

#[tokio::test]
async fn test_recovery_and_dead_letter() {
    let fixture = common::setup_test_fixture().await.unwrap();
    let event_queue = EventQueue::new(fixture.state.db_pool.pool().clone());
    let pool = fixture.state.db_pool.pool();
    let repo = DocumentRepository::new(pool);

    // Phase A: Stale processing recovery
    let content_id = fixture
        .state
        .content_storage
        .store_content("Recovery test content".as_bytes(), None)
        .await
        .unwrap();

    let event = ConnectorEvent::DocumentCreated {
        sync_run_id: "sync_recovery".to_string(),
        source_id: TEST_SOURCE_ID.to_string(),
        document_id: "recovery_doc".to_string(),
        content_id,
        metadata: DocumentMetadata {
            title: Some("Recovery Document".to_string()),
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
            public: false,
            users: vec!["user1".to_string()],
            groups: vec![],
        },
        attributes: None,
    };

    let event_id = event_queue.enqueue(TEST_SOURCE_ID, &event).await.unwrap();

    sqlx::query(
        "UPDATE connector_events_queue SET status = 'processing', processing_started_at = NOW() - INTERVAL '10 minutes' WHERE id = $1"
    )
    .bind(&event_id)
    .execute(pool)
    .await
    .unwrap();

    let stats = event_queue.get_queue_stats().await.unwrap();
    assert_eq!(stats.processing, 1);
    assert_eq!(stats.pending, 0);

    let recovered = event_queue
        .recover_stale_processing_items(300)
        .await
        .unwrap();
    assert_eq!(recovered, 1);

    let stats_after = event_queue.get_queue_stats().await.unwrap();
    assert_eq!(stats_after.processing, 0);
    assert_eq!(stats_after.pending, 1);

    let processor = QueueProcessor::new(fixture.state.clone());
    let processor_handle = tokio::spawn(async move {
        let _ = processor.start().await;
    });
    tokio::time::sleep(Duration::from_millis(100)).await;

    let document = common::wait_for_document_exists(
        &repo,
        TEST_SOURCE_ID,
        "recovery_doc",
        Duration::from_secs(5),
    )
    .await
    .expect("Recovered document should be processed");
    assert_eq!(document.title, "Recovery Document");

    processor_handle.abort();

    // Phase B: Dead letter queue
    let dl_content_id = fixture
        .state
        .content_storage
        .store_content("Dead letter content".as_bytes(), None)
        .await
        .unwrap();

    let dl_event = ConnectorEvent::DocumentCreated {
        sync_run_id: "sync_deadletter".to_string(),
        source_id: TEST_SOURCE_ID.to_string(),
        document_id: "deadletter_doc".to_string(),
        content_id: dl_content_id,
        metadata: DocumentMetadata {
            title: Some("Dead Letter Document".to_string()),
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

    let dl_event_id = event_queue
        .enqueue(TEST_SOURCE_ID, &dl_event)
        .await
        .unwrap();

    // mark_failed increments retry_count each call; at retry_count >= max_retries (3), status becomes dead_letter
    event_queue
        .mark_failed(&dl_event_id, "error attempt 1")
        .await
        .unwrap();
    event_queue
        .mark_failed(&dl_event_id, "error attempt 2")
        .await
        .unwrap();
    event_queue
        .mark_failed(&dl_event_id, "error attempt 3")
        .await
        .unwrap();

    let dl_stats = event_queue.get_queue_stats().await.unwrap();
    assert!(
        dl_stats.dead_letter >= 1,
        "Should have at least 1 dead letter item, got {}",
        dl_stats.dead_letter
    );

    let retried = event_queue.retry_failed_events().await.unwrap();
    assert_eq!(retried, 0, "Dead letter items should not be retried");

    // Phase C: Embedding queue recovery
    let embed_content_id = fixture
        .state
        .content_storage
        .store_content("Embedding recovery content".as_bytes(), None)
        .await
        .unwrap();

    let embed_doc = Document {
        id: "embed_recovery_doc".to_string(),
        source_id: TEST_SOURCE_ID.to_string(),
        external_id: "embed_recovery_external".to_string(),
        title: "Embedding Recovery Document".to_string(),
        content_id: Some(embed_content_id),
        content_type: Some("text/plain".to_string()),
        file_size: Some(25),
        file_extension: None,
        url: None,
        metadata: json!({}),
        permissions: json!({"public": true, "users": [], "groups": []}),
        attributes: json!({}),
        created_at: OffsetDateTime::now_utc(),
        updated_at: OffsetDateTime::now_utc(),
        last_indexed_at: OffsetDateTime::now_utc(),
    };

    repo.create(embed_doc).await.unwrap();

    let embedding_queue = shared::EmbeddingQueue::new(pool.clone());

    let stats_before = embedding_queue.get_queue_stats().await.unwrap();

    let queue_id = embedding_queue
        .enqueue("embed_recovery_doc".to_string())
        .await
        .unwrap()
        .unwrap();

    sqlx::query(
        "UPDATE embedding_queue SET status = 'processing', processing_started_at = CURRENT_TIMESTAMP - INTERVAL '10 minutes' WHERE id = $1"
    )
    .bind(&queue_id)
    .execute(pool)
    .await
    .unwrap();

    let embed_recovered = embedding_queue
        .recover_stale_processing_items(300)
        .await
        .unwrap();
    assert!(
        embed_recovered >= 1,
        "Should recover at least 1 stale embedding item"
    );

    // Verify our specific item was recovered back to pending
    let row: (String,) = sqlx::query_as("SELECT status FROM embedding_queue WHERE id = $1")
        .bind(&queue_id)
        .fetch_one(pool)
        .await
        .unwrap();
    assert_eq!(row.0, "pending");

    let embed_stats_after = embedding_queue.get_queue_stats().await.unwrap();
    assert!(
        embed_stats_after.pending >= stats_before.pending + 1,
        "Pending count should have increased"
    );
}

#[tokio::test]
async fn test_api_document_operations() {
    let fixture = common::setup_test_fixture().await.unwrap();
    let server = TestServer::new(fixture.app().clone()).unwrap();

    // 1. Health check
    let response = server.get("/health").await;
    assert_eq!(response.status_code(), StatusCode::OK);
    let body: Value = response.json();
    assert_eq!(body["status"], "healthy");
    assert_eq!(body["service"], "indexer");
    assert_eq!(body["database"], "connected");
    assert_eq!(body["redis"], "connected");

    // 2. Create document
    let request = create_document_request();
    let response = server.post("/documents").json(&request).await;
    assert_eq!(response.status_code(), StatusCode::OK);

    let created_doc: Document = response.json();
    assert_eq!(created_doc.source_id, request.source_id);
    assert_eq!(created_doc.external_id, request.external_id);
    assert_eq!(created_doc.title, request.title);
    assert_eq!(created_doc.metadata, request.metadata);
    assert_eq!(created_doc.permissions, request.permissions);

    let count: (i64,) = sqlx::query_as("SELECT COUNT(*) FROM documents")
        .fetch_one(fixture.state.db_pool.pool())
        .await
        .unwrap();
    assert_eq!(count.0, 1);

    // 3. Get document
    let response = server.get(&format!("/documents/{}", created_doc.id)).await;
    assert_eq!(response.status_code(), StatusCode::OK);
    let fetched_doc: Document = response.json();
    assert_eq!(fetched_doc.id, created_doc.id);
    assert_eq!(fetched_doc.title, created_doc.title);

    // 4. Full update
    let update_req = update_document_request();
    let response = server
        .put(&format!("/documents/{}", created_doc.id))
        .json(&update_req)
        .await;
    assert_eq!(response.status_code(), StatusCode::OK);

    let updated_doc: Document = response.json();
    let expected_title = update_req.title.unwrap();
    let expected_metadata = update_req.metadata.unwrap();
    let expected_permissions = update_req.permissions.unwrap();

    assert_eq!(updated_doc.id, created_doc.id);
    assert_eq!(updated_doc.title, expected_title);
    assert_eq!(updated_doc.metadata, expected_metadata);
    assert_eq!(updated_doc.permissions, expected_permissions);
    assert!(updated_doc.updated_at > created_doc.updated_at);

    let db_doc: Document = sqlx::query_as::<_, Document>("SELECT * FROM documents WHERE id = $1")
        .bind(&created_doc.id)
        .fetch_one(fixture.state.db_pool.pool())
        .await
        .unwrap();
    assert_eq!(db_doc.title, expected_title);
    assert_eq!(db_doc.metadata, expected_metadata);

    // 5. Partial update (title only)
    let partial_update = json!({ "title": "Only Title Updated" });
    let response = server
        .put(&format!("/documents/{}", created_doc.id))
        .json(&partial_update)
        .await;
    assert_eq!(response.status_code(), StatusCode::OK);

    let partial_doc: Document = response.json();
    assert_eq!(partial_doc.title, "Only Title Updated");
    assert_eq!(partial_doc.metadata, expected_metadata);
    assert_eq!(partial_doc.permissions, expected_permissions);

    // 6. Bulk operations: create 2nd doc, then bulk create 3rd + update 1st + delete nonexistent
    let mut create_doc2 = create_document_request();
    create_doc2.external_id = "ext_456".to_string();
    create_doc2.title = "Second Document".to_string();
    let doc2_response = server.post("/documents").json(&create_doc2).await;
    let doc2: Document = doc2_response.json();

    let mut create_doc3 = create_document_request();
    create_doc3.external_id = "ext_789".to_string();
    create_doc3.title = "Third Document".to_string();

    let bulk_request = BulkDocumentRequest {
        operations: vec![
            BulkDocumentOperation {
                operation: "create".to_string(),
                document_id: None,
                document: Some(create_doc3),
                updates: None,
            },
            BulkDocumentOperation {
                operation: "update".to_string(),
                document_id: Some(created_doc.id.clone()),
                document: None,
                updates: Some(update_document_request()),
            },
            BulkDocumentOperation {
                operation: "delete".to_string(),
                document_id: Some("nonexistent-id".to_string()),
                document: None,
                updates: None,
            },
        ],
    };

    let bulk_response = server.post("/documents/bulk").json(&bulk_request).await;
    assert_eq!(bulk_response.status_code(), StatusCode::OK);

    let bulk_result: Value = bulk_response.json();
    assert_eq!(bulk_result["success_count"], 2);
    assert_eq!(bulk_result["error_count"], 1);
    assert_eq!(bulk_result["errors"].as_array().unwrap().len(), 1);

    let updated_via_bulk = server.get(&format!("/documents/{}", created_doc.id)).await;
    let updated_bulk_doc: Document = updated_via_bulk.json();
    assert_eq!(updated_bulk_doc.title, "Updated Test Document");

    // 7. Delete document
    let delete_response = server.delete(&format!("/documents/{}", doc2.id)).await;
    assert_eq!(delete_response.status_code(), StatusCode::OK);

    let delete_body: Value = delete_response.json();
    assert_eq!(delete_body["message"], "Document deleted successfully");

    let get_deleted = server.get(&format!("/documents/{}", doc2.id)).await;
    assert_eq!(get_deleted.status_code(), StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn test_people_extraction_from_events() {
    let fixture = common::setup_test_fixture().await.unwrap();
    let event_queue = EventQueue::new(fixture.state.db_pool.pool().clone());
    let repo = DocumentRepository::new(fixture.state.db_pool.pool());
    let person_repo = PersonRepository::new(fixture.state.db_pool.pool());

    let processor =
        QueueProcessor::new(fixture.state.clone()).with_poll_interval(Duration::from_millis(200));
    let processor_handle = tokio::spawn(async move {
        let _ = processor.start().await;
    });
    tokio::time::sleep(Duration::from_millis(100)).await;

    // Create a document with emails in permissions.users and metadata.author
    let content_id = fixture
        .state
        .content_storage
        .store_content("People extraction test".as_bytes(), None)
        .await
        .unwrap();

    let create_event = ConnectorEvent::DocumentCreated {
        sync_run_id: "sync_people".to_string(),
        source_id: TEST_SOURCE_ID.to_string(),
        document_id: "people_doc_1".to_string(),
        content_id,
        metadata: DocumentMetadata {
            title: Some("Team Meeting Notes".to_string()),
            author: Some("alice@example.com".to_string()),
            created_at: Some(OffsetDateTime::now_utc()),
            updated_at: Some(OffsetDateTime::now_utc()),
            content_type: None,
            mime_type: Some("text/plain".to_string()),
            size: Some("500".to_string()),
            url: None,
            path: None,
            extra: None,
        },
        permissions: DocumentPermissions {
            public: false,
            users: vec![
                "alice@example.com".to_string(),
                "bob@example.com".to_string(),
                "carol@example.com".to_string(),
            ],
            groups: vec![],
        },
        attributes: None,
    };

    event_queue
        .enqueue(TEST_SOURCE_ID, &create_event)
        .await
        .unwrap();

    // Wait for the document to be indexed
    common::wait_for_document_exists(
        &repo,
        TEST_SOURCE_ID,
        "people_doc_1",
        Duration::from_secs(5),
    )
    .await
    .expect("Document should be created");

    // Give a moment for people extraction (runs after document upsert)
    tokio::time::sleep(Duration::from_millis(500)).await;

    // Verify people were extracted and upserted
    let alice = person_repo
        .fetch_person_by_email("alice@example.com")
        .await
        .expect("DB query should succeed");
    assert!(
        alice.is_some(),
        "alice@example.com should be in people table"
    );

    let bob = person_repo
        .fetch_person_by_email("bob@example.com")
        .await
        .expect("DB query should succeed");
    assert!(bob.is_some(), "bob@example.com should be in people table");

    let carol = person_repo
        .fetch_person_by_email("carol@example.com")
        .await
        .expect("DB query should succeed");
    assert!(
        carol.is_some(),
        "carol@example.com should be in people table"
    );

    // Verify non-email strings were NOT extracted (e.g. the old test used "user1" without @)
    let nobody = person_repo
        .fetch_person_by_email("nonexistent@example.com")
        .await
        .expect("DB query should succeed");
    assert!(
        nobody.is_none(),
        "nonexistent email should not be in people table"
    );

    // Send a second document with overlapping people — verify deduplication
    let content_id2 = fixture
        .state
        .content_storage
        .store_content("Second document".as_bytes(), None)
        .await
        .unwrap();

    let create_event2 = ConnectorEvent::DocumentCreated {
        sync_run_id: "sync_people".to_string(),
        source_id: TEST_SOURCE_ID.to_string(),
        document_id: "people_doc_2".to_string(),
        content_id: content_id2,
        metadata: DocumentMetadata {
            title: Some("Project Update".to_string()),
            author: Some("bob@example.com".to_string()),
            ..Default::default()
        },
        permissions: DocumentPermissions {
            public: false,
            users: vec![
                "bob@example.com".to_string(),
                "dave@example.com".to_string(),
            ],
            groups: vec![],
        },
        attributes: None,
    };

    event_queue
        .enqueue(TEST_SOURCE_ID, &create_event2)
        .await
        .unwrap();

    common::wait_for_document_exists(
        &repo,
        TEST_SOURCE_ID,
        "people_doc_2",
        Duration::from_secs(5),
    )
    .await
    .expect("Second document should be created");

    tokio::time::sleep(Duration::from_millis(500)).await;

    // bob should still be one row (deduplication)
    let bob_count: (i64,) =
        sqlx::query_as("SELECT COUNT(*) FROM people WHERE lower(email) = 'bob@example.com'")
            .fetch_one(fixture.state.db_pool.pool())
            .await
            .unwrap();
    assert_eq!(
        bob_count.0, 1,
        "bob should appear exactly once in people table"
    );

    // dave should now exist
    let dave = person_repo
        .fetch_person_by_email("dave@example.com")
        .await
        .expect("DB query should succeed");
    assert!(dave.is_some(), "dave@example.com should be in people table");

    // Total people count should be 4 (alice, bob, carol, dave)
    let total: (i64,) = sqlx::query_as("SELECT COUNT(*) FROM people")
        .fetch_one(fixture.state.db_pool.pool())
        .await
        .unwrap();
    assert_eq!(total.0, 4, "Should have exactly 4 people");

    processor_handle.abort();
}

#[tokio::test]
async fn test_group_membership_sync_event() {
    let fixture = common::setup_test_fixture().await.unwrap();
    let event_queue = EventQueue::new(fixture.state.db_pool.pool().clone());
    let pool = fixture.state.db_pool.pool();

    let processor =
        QueueProcessor::new(fixture.state.clone()).with_poll_interval(Duration::from_millis(200));
    let processor_handle = tokio::spawn(async move {
        let _ = processor.start().await;
    });
    tokio::time::sleep(Duration::from_millis(100)).await;

    // Enqueue a group membership sync event
    let event = ConnectorEvent::GroupMembershipSync {
        sync_run_id: "sync_groups_1".to_string(),
        source_id: TEST_SOURCE_ID.to_string(),
        group_email: "engineering@test.com".to_string(),
        group_name: Some("Engineering Team".to_string()),
        member_emails: vec![
            "alice@test.com".to_string(),
            "bob@test.com".to_string(),
            "charlie@test.com".to_string(),
        ],
    };
    event_queue.enqueue(TEST_SOURCE_ID, &event).await.unwrap();

    // Wait for event to be processed
    common::wait_for_completed(pool, 1, Duration::from_secs(10)).await;

    // Verify group was created
    let group_repo = GroupRepository::new(pool);
    let alice_groups = group_repo
        .find_groups_for_user("alice@test.com")
        .await
        .unwrap();
    assert_eq!(alice_groups, vec!["engineering@test.com"]);

    let bob_groups = group_repo
        .find_groups_for_user("bob@test.com")
        .await
        .unwrap();
    assert_eq!(bob_groups, vec!["engineering@test.com"]);

    let charlie_groups = group_repo
        .find_groups_for_user("charlie@test.com")
        .await
        .unwrap();
    assert_eq!(charlie_groups, vec!["engineering@test.com"]);

    // Non-member should have no groups
    let outsider_groups = group_repo
        .find_groups_for_user("outsider@test.com")
        .await
        .unwrap();
    assert!(outsider_groups.is_empty());

    processor_handle.abort();
}

mod common;

use axum::http::StatusCode;
use axum_test::TestServer;
use common::fixtures::{create_document_request, update_document_request};
use omni_indexer::{BulkDocumentOperation, BulkDocumentRequest};
use serde_json::{json, Value};
use shared::models::Document;
use sqlx::Row;

#[tokio::test]
async fn test_health_check() {
    let fixture = common::setup_test_fixture().await.unwrap();
    let server = TestServer::new(fixture.app().clone()).unwrap();

    let response = server.get("/health").await;

    assert_eq!(response.status_code(), StatusCode::OK);

    let body: Value = response.json();
    assert_eq!(body["status"], "healthy");
    assert_eq!(body["service"], "indexer");
    assert_eq!(body["database"], "connected");
    assert_eq!(body["redis"], "connected");
}

#[tokio::test]
async fn test_create_document() {
    let fixture = common::setup_test_fixture().await.unwrap();
    let server = TestServer::new(fixture.app().clone()).unwrap();

    // Verify database is empty before test
    let initial_count_result = sqlx::query("SELECT COUNT(*) as count FROM documents")
        .fetch_one(fixture.state.db_pool.pool())
        .await
        .unwrap();
    let initial_count: i64 = initial_count_result.get("count");
    assert_eq!(initial_count, 0, "Database should be empty before test");

    let request = create_document_request();
    let response = server.post("/documents").json(&request).await;

    if response.status_code() != StatusCode::OK {
        let error_text = response.text();
        panic!(
            "Expected 200 OK, got {}: {}",
            response.status_code(),
            error_text
        );
    }

    let document: Document = response.json();
    assert_eq!(document.source_id, request.source_id);
    assert_eq!(document.external_id, request.external_id);
    assert_eq!(document.title, request.title);
    // TODO: Content field checks disabled during content storage migration
    // assert_eq!(document.content, Some(request.content));
    assert_eq!(document.metadata, request.metadata);
    assert_eq!(document.permissions, request.permissions);

    // Verify exactly one document exists in database
    let final_count_result = sqlx::query("SELECT COUNT(*) as count FROM documents")
        .fetch_one(fixture.state.db_pool.pool())
        .await
        .unwrap();
    let final_count: i64 = final_count_result.get("count");
    assert_eq!(
        final_count, 1,
        "Exactly one document should exist after creation"
    );

    // Cleanup: Delete the created document
    let delete_response = server.delete(&format!("/documents/{}", document.id)).await;
    assert_eq!(delete_response.status_code(), StatusCode::OK);

    // Verify cleanup worked
    let cleanup_count_result = sqlx::query("SELECT COUNT(*) as count FROM documents")
        .fetch_one(fixture.state.db_pool.pool())
        .await
        .unwrap();
    let cleanup_count: i64 = cleanup_count_result.get("count");
    assert_eq!(cleanup_count, 0, "Database should be empty after cleanup");
}

#[tokio::test]
async fn test_get_nonexistent_document() {
    let fixture = common::setup_test_fixture().await.unwrap();
    let server = TestServer::new(fixture.app().clone()).unwrap();

    let response = server.get("/documents/nonexistent-id").await;

    assert_eq!(response.status_code(), StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn test_update_document() {
    let fixture = common::setup_test_fixture().await.unwrap();
    let server = TestServer::new(fixture.app().clone()).unwrap();

    let create_response = server
        .post("/documents")
        .json(&create_document_request())
        .await;

    let created_doc: Document = create_response.json();

    let update_request = update_document_request();
    let update_response = server
        .put(&format!("/documents/{}", created_doc.id))
        .json(&update_request)
        .await;

    assert_eq!(update_response.status_code(), StatusCode::OK);

    let updated_doc: Document = update_response.json();

    // Store expected values for comparison
    let expected_title = update_request.title.unwrap();
    let expected_metadata = update_request.metadata.unwrap();
    let expected_permissions = update_request.permissions.unwrap();

    assert_eq!(updated_doc.id, created_doc.id);
    assert_eq!(updated_doc.title, expected_title);
    // TODO: Content field checks disabled during content storage migration
    // assert_eq!(updated_doc.content, update_request.content);
    assert_eq!(updated_doc.metadata, expected_metadata);
    assert_eq!(updated_doc.permissions, expected_permissions);
    assert!(updated_doc.updated_at > created_doc.updated_at);

    // Re-fetch the document from the database to verify persistence
    let db_doc_result = sqlx::query_as::<_, Document>("SELECT * FROM documents WHERE id = $1")
        .bind(&created_doc.id)
        .fetch_one(fixture.state.db_pool.pool())
        .await
        .unwrap();

    // Verify the database document matches our expectations
    assert_eq!(db_doc_result.id, created_doc.id);
    assert_eq!(db_doc_result.title, expected_title);
    assert_eq!(db_doc_result.metadata, expected_metadata);
    assert_eq!(db_doc_result.permissions, expected_permissions);
    assert!(db_doc_result.updated_at > created_doc.updated_at);
}

#[tokio::test]
async fn test_partial_update_document() {
    let fixture = common::setup_test_fixture().await.unwrap();
    let server = TestServer::new(fixture.app().clone()).unwrap();

    let create_response = server
        .post("/documents")
        .json(&create_document_request())
        .await;

    let created_doc: Document = create_response.json();

    let partial_update = json!({
        "title": "Only Title Updated"
    });

    let update_response = server
        .put(&format!("/documents/{}", created_doc.id))
        .json(&partial_update)
        .await;

    assert_eq!(update_response.status_code(), StatusCode::OK);

    let updated_doc: Document = update_response.json();
    assert_eq!(updated_doc.title, "Only Title Updated");
    // TODO: Content field checks disabled during content storage migration
    // assert_eq!(updated_doc.content, created_doc.content);
    assert_eq!(updated_doc.metadata, created_doc.metadata);
    assert_eq!(updated_doc.permissions, created_doc.permissions);
}

#[tokio::test]
async fn test_delete_document() {
    let fixture = common::setup_test_fixture().await.unwrap();
    let server = TestServer::new(fixture.app().clone()).unwrap();

    let create_response = server
        .post("/documents")
        .json(&create_document_request())
        .await;

    let created_doc: Document = create_response.json();

    let delete_response = server
        .delete(&format!("/documents/{}", created_doc.id))
        .await;

    assert_eq!(delete_response.status_code(), StatusCode::OK);

    let delete_body: Value = delete_response.json();
    assert_eq!(delete_body["message"], "Document deleted successfully");
    assert_eq!(delete_body["id"], created_doc.id);

    let get_response = server.get(&format!("/documents/{}", created_doc.id)).await;

    assert_eq!(get_response.status_code(), StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn test_delete_nonexistent_document() {
    let fixture = common::setup_test_fixture().await.unwrap();
    let server = TestServer::new(fixture.app().clone()).unwrap();

    let response = server.delete("/documents/nonexistent-id").await;

    assert_eq!(response.status_code(), StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn test_bulk_operations() {
    let fixture = common::setup_test_fixture().await.unwrap();
    let server = TestServer::new(fixture.app().clone()).unwrap();

    let create_doc1 = create_document_request();
    let mut create_doc2 = create_document_request();
    create_doc2.external_id = "ext_456".to_string();
    create_doc2.title = "Second Document".to_string();

    let created_doc1_response = server.post("/documents").json(&create_doc1).await;
    let created_doc1: Document = created_doc1_response.json();

    let bulk_request = BulkDocumentRequest {
        operations: vec![
            BulkDocumentOperation {
                operation: "create".to_string(),
                document_id: None,
                document: Some(create_doc2),
                updates: None,
            },
            BulkDocumentOperation {
                operation: "update".to_string(),
                document_id: Some(created_doc1.id.clone()),
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
    assert!(bulk_result["errors"].as_array().unwrap().len() == 1);

    let updated_doc_response = server.get(&format!("/documents/{}", created_doc1.id)).await;
    let updated_doc: Document = updated_doc_response.json();
    assert_eq!(updated_doc.title, "Updated Test Document");
}

#[tokio::test]
async fn test_concurrent_document_operations() {
    let fixture = common::setup_test_fixture().await.unwrap();
    let server = TestServer::new(fixture.app().clone()).unwrap();

    let mut requests = vec![];
    for i in 0..5 {
        let mut request = create_document_request();
        request.external_id = format!("ext_{}", i);
        request.title = format!("Document {}", i);
        requests.push(request);
    }

    let mut handles = vec![];
    for request in requests {
        let response_future = server.post("/documents").json(&request).await;

        assert_eq!(response_future.status_code(), StatusCode::OK);
        let doc: Document = response_future.json();
        handles.push(doc);
    }

    assert_eq!(handles.len(), 5);

    for (i, doc) in handles.iter().enumerate() {
        assert_eq!(doc.external_id, format!("ext_{}", i));
    }
}

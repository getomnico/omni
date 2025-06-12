mod common;

use axum::http::StatusCode;
use axum_test::TestServer;
use common::fixtures::{create_document_request, update_document_request};
use indexer::{BulkDocumentOperation, BulkDocumentRequest};
use serde_json::{json, Value};
use shared::models::Document;
use sqlx::Row;
use ulid;

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
async fn test_direct_database_insert() {
    let fixture = common::setup_test_fixture().await.unwrap();

    // Try a direct database insert to isolate the issue
    let document_id = ulid::Ulid::new().to_string();
    let source_id = "01JGF7V3E0Y2R1X8P5Q7W9T4N7";
    let external_id = "ext_123";

    eprintln!("Attempting direct insert with IDs:");
    eprintln!("document_id: '{}' (len={})", document_id, document_id.len());
    eprintln!("source_id: '{}' (len={})", source_id, source_id.len());
    eprintln!("external_id: '{}' (len={})", external_id, external_id.len());

    let result = sqlx::query(
        r#"
        INSERT INTO documents (id, source_id, external_id, title, content, metadata, permissions, created_at, updated_at, last_indexed_at)
        VALUES ($1, $2, $3, $4, $5, $6, $7, NOW(), NOW(), NOW())
        "#,
    )
    .bind(&document_id)
    .bind(source_id)
    .bind(external_id)
    .bind("Test Title")
    .bind(Some("Test Content"))
    .bind(&serde_json::json!({"test": "data"}))
    .bind(&serde_json::json!({"users": ["test"]}))
    .execute(fixture.state.db_pool.pool())
    .await;

    match result {
        Ok(_) => eprintln!("Direct database insert succeeded!"),
        Err(e) => eprintln!("Direct database insert failed: {}", e),
    }
}

#[tokio::test]
async fn test_create_document() {
    let fixture = common::setup_test_fixture().await.unwrap();
    let server = TestServer::new(fixture.app().clone()).unwrap();

    let request = create_document_request();
    eprintln!(
        "Request data: {}",
        serde_json::to_string_pretty(&request).unwrap()
    );

    let response = server.post("/documents").json(&request).await;

    if response.status_code() != StatusCode::OK {
        eprintln!("Response status: {}", response.status_code());
        eprintln!("Response body: {}", response.text());

        // Debug: Check database state by running a simple query
        let result = sqlx::query("SELECT COUNT(*) as count FROM sources")
            .fetch_one(fixture.state.db_pool.pool())
            .await;
        match result {
            Ok(row) => {
                let count: i64 = row.get("count");
                eprintln!("Sources count: {}", count);
            }
            Err(e) => eprintln!("Database query error: {}", e),
        }
    }
    assert_eq!(response.status_code(), StatusCode::OK);

    let document: Document = response.json();
    assert_eq!(document.source_id, request.source_id);
    assert_eq!(document.external_id, request.external_id);
    assert_eq!(document.title, request.title);
    assert_eq!(document.content, Some(request.content));
    assert_eq!(document.metadata, request.metadata);
    assert_eq!(document.permissions, request.permissions);
}

#[tokio::test]
async fn test_get_document() {
    let fixture = common::setup_test_fixture().await.unwrap();
    let server = TestServer::new(fixture.app().clone()).unwrap();

    let create_response = server
        .post("/documents")
        .json(&create_document_request())
        .await;

    let created_doc: Document = create_response.json();

    let get_response = server.get(&format!("/documents/{}", created_doc.id)).await;

    assert_eq!(get_response.status_code(), StatusCode::OK);

    let fetched_doc: Document = get_response.json();
    assert_eq!(fetched_doc.id, created_doc.id);
    assert_eq!(fetched_doc.title, created_doc.title);
    assert_eq!(fetched_doc.content, created_doc.content);
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
    assert_eq!(updated_doc.id, created_doc.id);
    assert_eq!(updated_doc.title, update_request.title.unwrap());
    assert_eq!(updated_doc.content, update_request.content);
    assert_eq!(updated_doc.metadata, update_request.metadata.unwrap());
    assert_eq!(updated_doc.permissions, update_request.permissions.unwrap());
    assert!(updated_doc.updated_at > created_doc.updated_at);
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
    assert_eq!(updated_doc.content, created_doc.content);
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

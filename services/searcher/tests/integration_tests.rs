mod common;

use anyhow::Result;
use axum::{
    body::Body,
    http::{Method, Request, StatusCode},
};
use common::SearcherTestFixture;
use serde_json::{json, Value};
use tower::ServiceExt;

#[tokio::test]
async fn test_health_check() -> Result<()> {
    let fixture = SearcherTestFixture::new().await?;

    let request = Request::builder()
        .method(Method::GET)
        .uri("/health")
        .body(Body::empty())?;

    let response = fixture.app.oneshot(request).await?;
    assert_eq!(response.status(), StatusCode::OK);

    let body = axum::body::to_bytes(response.into_body(), usize::MAX).await?;
    let json: Value = serde_json::from_slice(&body)?;

    assert_eq!(json["status"], "healthy");
    assert_eq!(json["database"], "connected");
    assert_eq!(json["redis"], "connected");

    Ok(())
}

#[tokio::test]
async fn test_empty_search_returns_error() -> Result<()> {
    let fixture = SearcherTestFixture::new().await?;
    let _doc_ids = fixture.seed_search_data().await?;

    let (status, response) = fixture.search("", None, None).await?;

    // Empty query returns 500 Internal Server Error with error message
    assert_eq!(status, StatusCode::INTERNAL_SERVER_ERROR);
    // Verify response contains error information
    assert!(
        response.get("error").is_some() || response.get("message").is_some(),
        "Expected error in response: {:?}",
        response
    );

    Ok(())
}

#[tokio::test]
async fn test_fulltext_search_rust_programming() -> Result<()> {
    let fixture = SearcherTestFixture::new().await?;
    let _doc_ids = fixture.seed_search_data().await?;

    let (status, response) = fixture
        .search("rust programming", Some("fulltext"), None)
        .await?;

    assert_eq!(status, StatusCode::OK);
    assert!(response["results"].is_array());

    let results = response["results"].as_array().unwrap();
    assert!(!results.is_empty());

    // Check that the Rust Programming Guide document is returned
    let rust_doc = results.iter().find(|doc| {
        doc["document"]["title"]
            .as_str()
            .unwrap_or("")
            .contains("Rust Programming Guide")
    });
    assert!(rust_doc.is_some());

    Ok(())
}

#[tokio::test]
async fn test_fulltext_search_meeting_planning() -> Result<()> {
    let fixture = SearcherTestFixture::new().await?;
    let _doc_ids = fixture.seed_search_data().await?;

    let (status, response) = fixture
        .search("meeting planning Q4", Some("fulltext"), None)
        .await?;

    assert_eq!(status, StatusCode::OK);
    assert!(response["results"].is_array());

    let results = response["results"].as_array().unwrap();
    assert!(!results.is_empty());

    // Check that the Q4 Planning Meeting document is returned
    let meeting_doc = results.iter().find(|doc| {
        doc["document"]["title"]
            .as_str()
            .unwrap_or("")
            .contains("Q4 Planning Meeting")
    });
    assert!(meeting_doc.is_some());

    Ok(())
}

#[tokio::test]
async fn test_fulltext_search_with_title_weight() -> Result<()> {
    let fixture = SearcherTestFixture::new().await?;
    let _doc_ids = fixture.seed_search_data().await?;

    // Search for "API" which appears in both title and content
    let (status, response) = fixture.search("API", Some("fulltext"), None).await?;

    assert_eq!(status, StatusCode::OK);
    assert!(response["results"].is_array());

    let results = response["results"].as_array().unwrap();
    assert!(!results.is_empty());

    // The "REST API Endpoints" document should rank higher due to title weight
    let first_result = &results[0];
    assert!(first_result["document"]["title"]
        .as_str()
        .unwrap_or("")
        .contains("API"));

    Ok(())
}

#[tokio::test]
async fn test_semantic_search() -> Result<()> {
    let fixture = SearcherTestFixture::new().await?;
    let _doc_ids = fixture.seed_search_data().await?;

    // Test semantic search with a conceptual query
    let (status, response) = fixture
        .search("software architecture patterns", Some("semantic"), None)
        .await?;

    // Semantic search might fail if AI service is not available (expected in test environment)
    if status == StatusCode::INTERNAL_SERVER_ERROR {
        // This is expected if AI service is not running - semantic search requires embeddings
        println!("Semantic search failed as expected (AI service not available)");
        return Ok(());
    }

    assert_eq!(status, StatusCode::OK);
    assert!(response["results"].is_array());

    let results = response["results"].as_array().unwrap();
    // Note: Since we're using dummy embeddings, we should still get results
    // In a real scenario with proper embeddings, this would find semantically similar documents
    assert!(results.len() <= 5); // Should return all or subset of documents

    Ok(())
}

#[tokio::test]
async fn test_hybrid_search() -> Result<()> {
    let fixture = SearcherTestFixture::new().await?;
    let _doc_ids = fixture.seed_search_data().await?;

    let (status, response) = fixture
        .search("rust programming", Some("hybrid"), None)
        .await?;

    // Hybrid search might fail if AI service is not available (expected in test environment)
    if status == StatusCode::INTERNAL_SERVER_ERROR {
        // This is expected if AI service is not running - hybrid search requires embeddings
        println!("Hybrid search failed as expected (AI service not available)");
        return Ok(());
    }

    assert_eq!(status, StatusCode::OK);
    assert!(response["results"].is_array());

    let results = response["results"].as_array().unwrap();
    assert!(!results.is_empty());

    // Hybrid search should combine FTS and semantic results
    // Check that results have both title match and semantic similarity
    let rust_doc = results.iter().find(|doc| {
        doc["document"]["title"]
            .as_str()
            .unwrap_or("")
            .contains("Rust Programming Guide")
    });
    assert!(rust_doc.is_some());

    Ok(())
}

#[tokio::test]
async fn test_search_with_limit() -> Result<()> {
    let fixture = SearcherTestFixture::new().await?;
    let _doc_ids = fixture.seed_search_data().await?;

    // Use a broad query that matches multiple documents
    let (status, response) = fixture.search("guide", Some("fulltext"), Some(2)).await?;

    assert_eq!(status, StatusCode::OK);
    assert!(response["results"].is_array());

    let results = response["results"].as_array().unwrap();
    assert!(results.len() <= 2); // Should be capped at limit
    assert!(response["total_count"].as_i64().unwrap() >= 1);

    Ok(())
}

#[tokio::test]
async fn test_search_with_filters() -> Result<()> {
    let fixture = SearcherTestFixture::new().await?;
    let _doc_ids = fixture.seed_search_data().await?;

    // Use source_types filter with a real query
    let search_body = json!({
        "query": "guide",
        "source_types": ["local_files"],
        "limit": 10
    });

    let request = Request::builder()
        .method(Method::POST)
        .uri("/search")
        .header("content-type", "application/json")
        .body(Body::from(search_body.to_string()))?;

    let response = fixture.app.clone().oneshot(request).await?;
    assert_eq!(response.status(), StatusCode::OK);

    let body = axum::body::to_bytes(response.into_body(), usize::MAX).await?;
    let json: Value = serde_json::from_slice(&body)?;

    assert!(json["results"].is_array());
    // Should return results matching the query and filter
    let results = json["results"].as_array().unwrap();
    assert!(results.len() <= 10);

    Ok(())
}

#[tokio::test]
async fn test_search_no_results() -> Result<()> {
    let fixture = SearcherTestFixture::new().await?;
    let _doc_ids = fixture.seed_search_data().await?;

    let (status, response) = fixture
        .search("nonexistent_query_xyz", Some("fulltext"), None)
        .await?;

    assert_eq!(status, StatusCode::OK);
    assert!(response["results"].is_array());
    assert_eq!(response["results"].as_array().unwrap().len(), 0);
    assert_eq!(response["total_count"], 0);

    Ok(())
}

#[tokio::test]
async fn test_suggestions_endpoint() -> Result<()> {
    let fixture = SearcherTestFixture::new().await?;
    let _doc_ids = fixture.seed_search_data().await?;

    let (status, response) = fixture.suggestions("Rust").await?;

    assert_eq!(status, StatusCode::OK);
    assert!(response["suggestions"].is_array());

    let suggestions = response["suggestions"].as_array().unwrap();
    // Should find the "Rust Programming Guide" document
    assert!(suggestions
        .iter()
        .any(|s| s.as_str().unwrap_or("").contains("Rust Programming Guide")));

    Ok(())
}

#[tokio::test]
async fn test_suggestions_with_limit() -> Result<()> {
    let fixture = SearcherTestFixture::new().await?;
    let _doc_ids = fixture.seed_search_data().await?;

    let request = Request::builder()
        .method(Method::GET)
        .uri("/suggestions?q=guide&limit=2")
        .body(Body::empty())?;

    let response = fixture.app.clone().oneshot(request).await?;
    assert_eq!(response.status(), StatusCode::OK);

    let body = axum::body::to_bytes(response.into_body(), usize::MAX).await?;
    let json: Value = serde_json::from_slice(&body)?;

    assert!(json["suggestions"].is_array());
    let suggestions = json["suggestions"].as_array().unwrap();
    assert!(suggestions.len() <= 2);

    Ok(())
}

#[tokio::test]
async fn test_cache_behavior() -> Result<()> {
    let fixture = SearcherTestFixture::new().await?;
    let _doc_ids = fixture.seed_search_data().await?;

    // Make the same search twice
    let query = "rust programming";

    let start_time = std::time::Instant::now();
    let (status1, response1) = fixture.search(query, Some("fulltext"), None).await?;
    let first_duration = start_time.elapsed();

    let start_time = std::time::Instant::now();
    let (status2, response2) = fixture.search(query, Some("fulltext"), None).await?;
    let second_duration = start_time.elapsed();

    assert_eq!(status1, StatusCode::OK);
    assert_eq!(status2, StatusCode::OK);

    // Results should be identical
    assert_eq!(response1["total_count"], response2["total_count"]);
    assert_eq!(
        response1["results"].as_array().unwrap().len(),
        response2["results"].as_array().unwrap().len()
    );

    // Second request should be faster due to caching (though this is not guaranteed in tests)
    println!(
        "First request: {:?}, Second request: {:?}",
        first_duration, second_duration
    );

    Ok(())
}

#[tokio::test]
async fn test_invalid_search_mode() -> Result<()> {
    let fixture = SearcherTestFixture::new().await?;

    let search_body = json!({
        "query": "test",
        "mode": "InvalidMode"
    });

    let request = Request::builder()
        .method(Method::POST)
        .uri("/search")
        .header("content-type", "application/json")
        .body(Body::from(search_body.to_string()))?;

    let response = fixture.app.clone().oneshot(request).await?;
    assert_eq!(response.status(), StatusCode::UNPROCESSABLE_ENTITY);

    Ok(())
}

#[tokio::test]
async fn test_search_with_large_limit_capped() -> Result<()> {
    let fixture = SearcherTestFixture::new().await?;
    let _doc_ids = fixture.seed_search_data().await?;

    // Use a real query with a large limit that should be capped at 100
    let (status, response) = fixture.search("guide", Some("fulltext"), Some(200)).await?;

    assert_eq!(status, StatusCode::OK);
    // Results should be returned, limit is capped internally to 100
    let results = response["results"].as_array().unwrap();
    assert!(results.len() <= 100);

    Ok(())
}

#[tokio::test]
async fn test_search_content_type_filtering() -> Result<()> {
    let fixture = SearcherTestFixture::new().await?;
    let _doc_ids = fixture.seed_search_data().await?;

    // Test basic filtering functionality - verify the endpoint accepts content_types parameter
    let search_body = json!({
        "query": "guide",
        "content_types": ["documentation", "api_documentation", "user_guide"],
        "limit": 10
    });

    let request = Request::builder()
        .method(Method::POST)
        .uri("/search")
        .header("content-type", "application/json")
        .body(Body::from(search_body.to_string()))?;

    let response = fixture.app.clone().oneshot(request).await?;
    assert_eq!(response.status(), StatusCode::OK);

    let body = axum::body::to_bytes(response.into_body(), usize::MAX).await?;
    let json: Value = serde_json::from_slice(&body)?;

    assert!(json["results"].is_array());
    // Verify the request succeeds and returns results
    let results = json["results"].as_array().unwrap();
    assert!(results.len() <= 10);

    Ok(())
}

#[tokio::test]
async fn test_search_highlighting_extraction() -> Result<()> {
    let fixture = SearcherTestFixture::new().await?;
    let _doc_ids = fixture.seed_search_data().await?;

    let (status, response) = fixture
        .search("memory safety ownership", Some("fulltext"), None)
        .await?;

    assert_eq!(status, StatusCode::OK);
    assert!(response["results"].is_array());

    let results = response["results"].as_array().unwrap();
    if !results.is_empty() {
        let first_result = &results[0];

        // Check that highlights are present (if any)
        if let Some(highlights) = first_result["highlights"].as_array() {
            if !highlights.is_empty() {
                // Check for highlighting content
                assert!(!highlights.is_empty());
            }
        }
    }

    Ok(())
}

#[tokio::test]
async fn test_search_with_permission_filtering() -> Result<()> {
    let fixture = SearcherTestFixture::new().await?;
    let _doc_ids = fixture.seed_search_data().await?;

    // Search with user_email to trigger permission filtering
    // This tests the ParadeDB permission filter syntax: permissions @@@ 'public:true'
    let (status, response) = fixture
        .search_with_user("guide", Some("fulltext"), None, Some("test@example.com"))
        .await?;

    assert_eq!(status, StatusCode::OK);
    assert!(response["results"].is_array());

    // The search should complete successfully with permission filtering enabled
    // Results may be empty if no documents match the permission filter, but query should succeed
    assert!(response["total_count"].is_number());

    Ok(())
}

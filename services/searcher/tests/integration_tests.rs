mod common;

use anyhow::Result;
use axum::{
    body::Body,
    http::{Method, Request, StatusCode},
};
use common::SearcherTestFixture;
use serde_json::{json, Value};
use tower::ServiceExt;

/// Extract result titles from a search response in order.
fn result_titles(response: &Value) -> Vec<String> {
    response["results"]
        .as_array()
        .unwrap()
        .iter()
        .map(|r| r["document"]["title"].as_str().unwrap().to_string())
        .collect()
}

/// Assert that scores in results are positive and in descending order.
fn assert_scores_descending(response: &Value) {
    let results = response["results"].as_array().unwrap();
    let scores: Vec<f32> = results
        .iter()
        .map(|r| r["score"].as_f64().unwrap() as f32)
        .collect();
    for score in &scores {
        assert!(*score > 0.0, "Expected positive score, got {}", score);
    }
    for pair in scores.windows(2) {
        assert!(
            pair[0] >= pair[1],
            "Scores not descending: {} < {}",
            pair[0],
            pair[1]
        );
    }
}

/// Assert all results have the given match_type.
fn assert_match_type(response: &Value, expected: &str) {
    for result in response["results"].as_array().unwrap() {
        assert_eq!(
            result["match_type"].as_str().unwrap(),
            expected,
            "Expected match_type '{}', got '{}'",
            expected,
            result["match_type"]
        );
    }
}

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

    assert_eq!(status, StatusCode::INTERNAL_SERVER_ERROR);
    assert!(
        response.get("error").is_some() || response.get("message").is_some(),
        "Expected error in response: {:?}",
        response
    );

    Ok(())
}

#[tokio::test]
async fn test_fulltext_search() -> Result<()> {
    let fixture = SearcherTestFixture::new().await?;
    let _doc_ids = fixture.seed_search_data().await?;

    // Query 1: "rust programming" — title match should put Rust guide first
    let (status, response) = fixture
        .search("rust programming", Some("fulltext"), None)
        .await?;
    assert_eq!(status, StatusCode::OK);
    let titles = result_titles(&response);
    assert!(
        !titles.is_empty(),
        "Expected results for 'rust programming'"
    );
    assert_eq!(
        titles[0], "Rust Programming Guide",
        "Expected Rust Programming Guide as first result, got: {:?}",
        titles
    );
    assert_match_type(&response, "fulltext");
    assert_scores_descending(&response);

    // Query 2: "API" — title weight should boost REST API Endpoints
    let (status, response) = fixture.search("API", Some("fulltext"), None).await?;
    assert_eq!(status, StatusCode::OK);
    let titles = result_titles(&response);
    assert!(!titles.is_empty(), "Expected results for 'API'");
    assert_eq!(
        titles[0], "REST API Endpoints",
        "Expected REST API Endpoints as first result, got: {:?}",
        titles
    );
    assert_match_type(&response, "fulltext");
    assert_scores_descending(&response);

    // Query 3: "meeting planning Q4" — should find Q4 Planning Meeting first
    let (status, response) = fixture
        .search("meeting planning Q4", Some("fulltext"), None)
        .await?;
    assert_eq!(status, StatusCode::OK);
    let titles = result_titles(&response);
    assert!(
        !titles.is_empty(),
        "Expected results for 'meeting planning Q4'"
    );
    assert_eq!(
        titles[0], "Q4 Planning Meeting",
        "Expected Q4 Planning Meeting as first result, got: {:?}",
        titles
    );
    assert_match_type(&response, "fulltext");
    assert_scores_descending(&response);

    // Query 4: "search" — should match multiple docs
    let (status, response) = fixture.search("search", Some("fulltext"), None).await?;
    assert_eq!(status, StatusCode::OK);
    let titles = result_titles(&response);
    assert!(
        titles.len() >= 2,
        "Expected at least 2 results for 'search', got: {:?}",
        titles
    );
    assert_match_type(&response, "fulltext");
    assert_scores_descending(&response);

    Ok(())
}

#[tokio::test]
async fn test_semantic_search() -> Result<()> {
    let fixture = SearcherTestFixture::new().await?;
    let _doc_ids = fixture.seed_search_data().await?;

    // "software architecture patterns" (34 chars)
    // Mock embedding: embedding[i] = (34 + i) / 1024.0
    let (status, response) = fixture
        .search("software architecture patterns", Some("semantic"), None)
        .await?;
    assert_eq!(
        status,
        StatusCode::OK,
        "Semantic search should succeed with mock AI server"
    );
    let titles = result_titles(&response);
    assert!(
        !titles.is_empty(),
        "Expected semantic results for 'software architecture patterns'"
    );
    assert_match_type(&response, "semantic");
    assert_scores_descending(&response);

    // "memory safety systems programming" (38 chars)
    let (status, response) = fixture
        .search("memory safety systems programming", Some("semantic"), None)
        .await?;
    assert_eq!(status, StatusCode::OK);
    let titles = result_titles(&response);
    assert!(
        !titles.is_empty(),
        "Expected semantic results for 'memory safety systems programming'"
    );
    assert_match_type(&response, "semantic");
    assert_scores_descending(&response);

    Ok(())
}

#[tokio::test]
async fn test_hybrid_search() -> Result<()> {
    let fixture = SearcherTestFixture::new().await?;
    let _doc_ids = fixture.seed_search_data().await?;

    // "rust programming" — FTS title match + semantic similarity should both contribute
    let (status, response) = fixture
        .search("rust programming", Some("hybrid"), None)
        .await?;
    assert_eq!(
        status,
        StatusCode::OK,
        "Hybrid search should succeed with mock AI server"
    );
    let titles = result_titles(&response);
    assert!(
        !titles.is_empty(),
        "Expected hybrid results for 'rust programming'"
    );
    assert_eq!(
        titles[0], "Rust Programming Guide",
        "Rust Programming Guide should be first in hybrid for 'rust programming', got: {:?}",
        titles
    );
    assert_scores_descending(&response);

    // "search engine architecture" — strong FTS match on Doc 3
    let (status, response) = fixture
        .search("search engine architecture", Some("hybrid"), None)
        .await?;
    assert_eq!(status, StatusCode::OK);
    let titles = result_titles(&response);
    assert!(
        !titles.is_empty(),
        "Expected hybrid results for 'search engine architecture'"
    );
    assert_eq!(
        titles[0], "Search Engine Architecture",
        "Search Engine Architecture should be first in hybrid, got: {:?}",
        titles
    );
    assert_scores_descending(&response);

    Ok(())
}

#[tokio::test]
async fn test_search_with_limit() -> Result<()> {
    let fixture = SearcherTestFixture::new().await?;
    let _doc_ids = fixture.seed_search_data().await?;

    let (status, response) = fixture.search("guide", Some("fulltext"), Some(2)).await?;

    assert_eq!(status, StatusCode::OK);
    assert!(response["results"].is_array());

    let results = response["results"].as_array().unwrap();
    assert!(results.len() <= 2);
    assert!(response["total_count"].as_i64().unwrap() >= 1);

    Ok(())
}

#[tokio::test]
async fn test_content_type_filtering() -> Result<()> {
    let fixture = SearcherTestFixture::new().await?;
    let _doc_ids = fixture.seed_search_data().await?;

    // Filter to documentation only — should match Doc 1 ("Rust Programming Guide")
    let (status, response) = fixture
        .search_with_body(json!({
            "query": "guide",
            "content_types": ["documentation"],
            "limit": 10
        }))
        .await?;
    assert_eq!(status, StatusCode::OK);
    let results = response["results"].as_array().unwrap();
    assert!(
        !results.is_empty(),
        "Expected results for documentation content type"
    );
    for result in results {
        assert_eq!(
            result["document"]["content_type"].as_str().unwrap(),
            "documentation",
            "All results should have content_type 'documentation'"
        );
    }

    // Filter to nonexistent content type — should return 0 results
    let (status, response) = fixture
        .search_with_body(json!({
            "query": "guide",
            "content_types": ["nonexistent_type"],
            "limit": 10
        }))
        .await?;
    assert_eq!(status, StatusCode::OK);
    let results = response["results"].as_array().unwrap();
    assert!(
        results.is_empty(),
        "Expected 0 results for nonexistent content type, got {}",
        results.len()
    );

    Ok(())
}

#[tokio::test]
async fn test_permission_filtering() -> Result<()> {
    let fixture = SearcherTestFixture::new().await?;
    let _doc_ids = fixture.seed_search_data().await?;

    // user1 has access to all 5 docs (is in every document's users list)
    let (status, response) = fixture
        .search_with_user("guide", Some("fulltext"), None, Some("user1"))
        .await?;
    assert_eq!(status, StatusCode::OK);
    let results = response["results"].as_array().unwrap();
    assert!(
        !results.is_empty(),
        "user1 should see results (has access to all docs)"
    );

    // nobody@example.com has no access to any document
    let (status, response) = fixture
        .search_with_user("guide", Some("fulltext"), None, Some("nobody@example.com"))
        .await?;
    assert_eq!(status, StatusCode::OK);
    let results = response["results"].as_array().unwrap();
    assert!(
        results.is_empty(),
        "nobody@example.com should see 0 results, got {}",
        results.len()
    );

    Ok(())
}

#[tokio::test]
async fn test_highlighting() -> Result<()> {
    let fixture = SearcherTestFixture::new().await?;
    let _doc_ids = fixture.seed_search_data().await?;

    let (status, response) = fixture
        .search("memory safety", Some("fulltext"), None)
        .await?;
    assert_eq!(status, StatusCode::OK);

    let results = response["results"].as_array().unwrap();
    assert!(!results.is_empty(), "Expected results for 'memory safety'");

    // Doc 1 (Rust Programming Guide) should be first — it contains "memory safety"
    assert_eq!(
        results[0]["document"]["title"].as_str().unwrap(),
        "Rust Programming Guide"
    );

    let highlights = results[0]["highlights"].as_array().unwrap();
    assert!(
        !highlights.is_empty(),
        "Expected non-empty highlights for 'memory safety' query"
    );

    let highlight_text = highlights
        .iter()
        .map(|h| h.as_str().unwrap_or(""))
        .collect::<Vec<_>>()
        .join(" ");
    assert!(
        highlight_text.contains("**memory**") || highlight_text.contains("**safety**"),
        "Expected bold markers in highlights, got: {}",
        highlight_text
    );

    Ok(())
}

#[tokio::test]
async fn test_attribute_filtering() -> Result<()> {
    let fixture = SearcherTestFixture::new().await?;
    let _doc_ids = fixture.seed_search_data().await?;

    // category=programming should match Docs 1, 3, 4
    let (status, response) = fixture
        .search_with_body(json!({
            "query": "search OR programming OR API OR guide",
            "attribute_filters": {"category": "programming"},
            "limit": 10
        }))
        .await?;
    assert_eq!(status, StatusCode::OK);
    let results = response["results"].as_array().unwrap();
    assert!(
        !results.is_empty(),
        "Expected results for category=programming"
    );
    let titles: Vec<&str> = results
        .iter()
        .map(|r| r["document"]["title"].as_str().unwrap())
        .collect();
    for title in &titles {
        assert!(
            [
                "Rust Programming Guide",
                "Search Engine Architecture",
                "REST API Endpoints"
            ]
            .contains(title),
            "Unexpected document '{}' in category=programming results",
            title
        );
    }

    // language=rust should match only Doc 1
    let (status, response) = fixture
        .search_with_body(json!({
            "query": "programming OR guide",
            "attribute_filters": {"language": "rust"},
            "limit": 10
        }))
        .await?;
    assert_eq!(status, StatusCode::OK);
    let results = response["results"].as_array().unwrap();
    assert_eq!(
        results.len(),
        1,
        "Expected exactly 1 result for language=rust, got: {:?}",
        results
            .iter()
            .map(|r| r["document"]["title"].as_str().unwrap())
            .collect::<Vec<_>>()
    );
    assert_eq!(
        results[0]["document"]["title"].as_str().unwrap(),
        "Rust Programming Guide"
    );

    // Nonexistent attribute value — 0 results
    let (status, response) = fixture
        .search_with_body(json!({
            "query": "guide",
            "attribute_filters": {"category": "nonexistent"},
            "limit": 10
        }))
        .await?;
    assert_eq!(status, StatusCode::OK);
    let results = response["results"].as_array().unwrap();
    assert!(
        results.is_empty(),
        "Expected 0 results for nonexistent attribute, got {}",
        results.len()
    );

    Ok(())
}

#[tokio::test]
async fn test_cache_behavior() -> Result<()> {
    let fixture = SearcherTestFixture::new().await?;
    let _doc_ids = fixture.seed_search_data().await?;

    let query = "rust programming";

    let (status1, response1) = fixture.search(query, Some("fulltext"), None).await?;
    assert_eq!(status1, StatusCode::OK);

    // Verify a search cache key exists in Redis after the first query
    let mut conn = fixture
        .test_env
        .redis_client
        .get_multiplexed_async_connection()
        .await?;
    let keys: Vec<String> = redis::cmd("KEYS")
        .arg("search:*")
        .query_async(&mut conn)
        .await?;
    assert!(
        !keys.is_empty(),
        "Expected at least one search:* cache key in Redis after first query"
    );

    // Second identical query should return the same results
    let (status2, response2) = fixture.search(query, Some("fulltext"), None).await?;
    assert_eq!(status2, StatusCode::OK);

    assert_eq!(response1["total_count"], response2["total_count"]);
    let titles1 = result_titles(&response1);
    let titles2 = result_titles(&response2);
    assert_eq!(titles1, titles2, "Cached results should be identical");

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
async fn test_typeahead_subsequence_match() -> Result<()> {
    let fixture = SearcherTestFixture::new().await?;
    let _doc_ids = fixture.seed_search_data().await?;

    // "q4 planning" should match "Q4 Planning Meeting" via normalized subsequence
    let (status, response) = fixture.typeahead("q4 planning", None).await?;
    assert_eq!(status, StatusCode::OK);
    let results = response["results"].as_array().unwrap();
    assert!(
        results
            .iter()
            .any(|r| r["title"].as_str().unwrap() == "Q4 Planning Meeting"),
        "Expected 'Q4 Planning Meeting' in typeahead results, got: {:?}",
        results
    );

    // Mid-title match: "planning" should also match "Q4 Planning Meeting"
    let (status, response) = fixture.typeahead("planning", None).await?;
    assert_eq!(status, StatusCode::OK);
    let results = response["results"].as_array().unwrap();
    assert!(
        results
            .iter()
            .any(|r| r["title"].as_str().unwrap() == "Q4 Planning Meeting"),
        "Expected 'Q4 Planning Meeting' for mid-title query 'planning', got: {:?}",
        results
    );

    Ok(())
}

#[tokio::test]
async fn test_typeahead_special_chars_normalized() -> Result<()> {
    let fixture = SearcherTestFixture::new().await?;
    let _doc_ids = fixture.seed_search_data().await?;

    // "rest api" should match "REST API Endpoints"
    let (status, response) = fixture.typeahead("rest api", None).await?;
    assert_eq!(status, StatusCode::OK);
    let results = response["results"].as_array().unwrap();
    assert!(
        results
            .iter()
            .any(|r| r["title"].as_str().unwrap() == "REST API Endpoints"),
        "Expected 'REST API Endpoints' in typeahead results, got: {:?}",
        results
    );

    Ok(())
}

#[tokio::test]
async fn test_typeahead_empty_query() -> Result<()> {
    let fixture = SearcherTestFixture::new().await?;
    let _doc_ids = fixture.seed_search_data().await?;

    let (status, response) = fixture.typeahead("", None).await?;
    assert_eq!(status, StatusCode::OK);
    let results = response["results"].as_array().unwrap();
    assert!(
        results.is_empty(),
        "Empty query should return empty results"
    );

    Ok(())
}

#[tokio::test]
async fn test_typeahead_limit_respected() -> Result<()> {
    let fixture = SearcherTestFixture::new().await?;
    let _doc_ids = fixture.seed_search_data().await?;

    // "guide" matches multiple docs; limit=1 should return at most 1
    let (status, response) = fixture.typeahead("guide", Some(1)).await?;
    assert_eq!(status, StatusCode::OK);
    let results = response["results"].as_array().unwrap();
    assert!(
        results.len() <= 1,
        "Expected at most 1 result with limit=1, got {}",
        results.len()
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

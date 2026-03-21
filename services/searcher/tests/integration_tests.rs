mod common;

use anyhow::Result;
use axum::{
    body::Body,
    http::{Method, Request, StatusCode},
};
use common::SearcherTestFixture;
use serde_json::{json, Value};
use shared::db::repositories::{GroupRepository, PersonRepository, PersonUpsert};
use shared::models::DocumentPermissions;
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
    // Cross-fields scoring: only "Rust Programming Guide" matches both terms;
    // "Rust Prevention and Corrosion Control" is filtered by the score threshold
    assert_eq!(
        titles.len(),
        1,
        "Expected exactly 1 result for 'rust programming', got: {:?}",
        titles
    );
    // Score ratio: cross-fields should produce a clear gap between #1 and #2
    let results = response["results"].as_array().unwrap();
    if results.len() > 1 {
        let top_score = results[0]["score"].as_f64().unwrap();
        let second_score = results[1]["score"].as_f64().unwrap();
        assert!(
            top_score > second_score * 1.2,
            "Top score ({}) should be >1.2x second score ({})",
            top_score,
            second_score
        );
    }
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

    // Query 3: "meeting planning Q4" — should find Q4-related docs at the top
    let (status, response) = fixture
        .search("meeting planning Q4", Some("fulltext"), None)
        .await?;
    assert_eq!(status, StatusCode::OK);
    let titles = result_titles(&response);
    assert!(
        !titles.is_empty(),
        "Expected results for 'meeting planning Q4'"
    );
    assert!(
        titles.iter().take(2).any(|t| t == "Q4 Planning Meeting"),
        "Expected Q4 Planning Meeting in top 2 results, got: {:?}",
        titles
    );
    assert_match_type(&response, "fulltext");
    assert_scores_descending(&response);

    // Query 4: "search" — should match multiple docs, and verify facets
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

    // Facets: include_facets defaults to true, so every response should have them
    let facets = response["facets"]
        .as_array()
        .expect("Expected facets array in response");
    assert!(
        !facets.is_empty(),
        "Expected non-empty facets for broad 'search' query"
    );
    for facet in facets {
        assert!(
            facet["name"].as_str().is_some(),
            "Facet should have a 'name' field"
        );
        let values = facet["values"]
            .as_array()
            .expect("Facet should have a 'values' array");
        assert!(!values.is_empty(), "Facet values should be non-empty");
        for fv in values {
            assert!(
                fv["value"].as_str().is_some(),
                "Facet value should have a 'value' string"
            );
            let count = fv["count"]
                .as_i64()
                .expect("Facet value should have a 'count' integer");
            assert!(count > 0, "Facet count should be positive, got {}", count);
        }
    }

    // Query 5: phrase ranking — "blue square nda"
    // BlueSquare NDA should rank first (phrase match on "blue square" in title & content).
    // "Square Root Mathematics" only token-matches "square", so it scores much lower.
    let (status, response) = fixture
        .search("blue square nda", Some("fulltext"), None)
        .await?;
    assert_eq!(status, StatusCode::OK);
    let titles = result_titles(&response);
    // Cross-fields + phrase bonus: BlueSquare NDA matches all 3 terms plus phrase "blue square".
    // Square Root Mathematics matches "square" only. Geometry of Quadrilaterals matches "square"
    // via content ("squaring the side length").
    assert_eq!(
        titles,
        vec![
            "BlueSquare NDA",
            "Square Root Mathematics",
            "Geometry of Quadrilaterals"
        ],
        "Expected exact ranking for 'blue square nda'"
    );
    let results = response["results"].as_array().unwrap();
    let top_score = results[0]["score"].as_f64().unwrap();
    let second_score = results[1]["score"].as_f64().unwrap();
    assert!(
        top_score > second_score * 2.0,
        "Phrase match ({}) should be >2x the token-only match ({})",
        top_score,
        second_score
    );
    assert_match_type(&response, "fulltext");
    assert_scores_descending(&response);

    // Query 6: phrase ranking — "crm sales report"
    // "CRM Sales Reports" should rank first (phrase match on "crm sales report").
    // "Urban Crime Reports" only token-matches "report(s)", so it scores much lower.
    let (status, response) = fixture
        .search("crm sales report", Some("fulltext"), None)
        .await?;
    assert_eq!(status, StatusCode::OK);
    let titles = result_titles(&response);
    assert!(
        !titles.is_empty(),
        "Expected results for 'crm sales report'"
    );
    assert_eq!(
        titles[0], "CRM Sales Reports",
        "Expected CRM Sales Reports as first result, got: {:?}",
        titles
    );
    let results = response["results"].as_array().unwrap();
    let top_score = results[0]["score"].as_f64().unwrap();
    if results.len() > 1 {
        let second_score = results[1]["score"].as_f64().unwrap();
        assert!(
            top_score > second_score,
            "Phrase match score ({}) should be higher than the token-only match score ({})",
            top_score,
            second_score
        );
    }
    // With stemming: "sales" stems to "sale", which does NOT match "salesman".
    // So "Death of a Salesman Book Report" only matches "report" (1/3 terms),
    // scoring below the threshold. Only "CRM Sales Reports" survives.
    assert_eq!(
        titles,
        vec!["CRM Sales Reports"],
        "Expected exact ranking for 'crm sales report'"
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

    // "square" matches multiple docs; limit=2 should cap results
    let (status, response) = fixture.search("square", Some("fulltext"), Some(2)).await?;

    assert_eq!(status, StatusCode::OK);
    assert!(response["results"].is_array());

    let results = response["results"].as_array().unwrap();
    assert!(results.len() <= 2);
    assert!(response["total_count"].as_i64().unwrap() >= 1);

    // Pagination: page 1 (offset=0, limit=2)
    let (status, page1_response) = fixture
        .search_with_body(json!({
            "query": "square",
            "mode": "fulltext",
            "limit": 2,
            "offset": 0
        }))
        .await?;
    assert_eq!(status, StatusCode::OK);
    let page1_titles = result_titles(&page1_response);
    assert!(
        page1_response["has_more"].as_bool().unwrap_or(false),
        "First page should have has_more=true since 'square' matches >2 docs"
    );

    // Pagination: page 2 (offset=2, limit=2)
    let (status, page2_response) = fixture
        .search_with_body(json!({
            "query": "square",
            "mode": "fulltext",
            "limit": 2,
            "offset": 2
        }))
        .await?;
    assert_eq!(status, StatusCode::OK);
    let page2_titles = result_titles(&page2_response);

    // No overlapping titles between pages
    for title in &page1_titles {
        assert!(
            !page2_titles.contains(title),
            "Duplicate result '{}' across pages. Page1: {:?}, Page2: {:?}",
            title,
            page1_titles,
            page2_titles
        );
    }

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

    // user1 has access to all docs (is in every document's users list)
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

    // Find "Rust Programming Guide" — it contains "memory safety" and should have highlights
    let rust_guide = results
        .iter()
        .find(|r| r["document"]["title"].as_str().unwrap() == "Rust Programming Guide")
        .expect("Expected Rust Programming Guide in results for 'memory safety'");

    let highlights = rust_guide["highlights"].as_array().unwrap();
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

    // Highlight count should respect the SQL snippet limit (3)
    assert!(
        highlights.len() <= 3,
        "Expected at most 3 highlight snippets, got {}",
        highlights.len()
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
async fn test_score_threshold_filters_low_relevance() -> Result<()> {
    let fixture = SearcherTestFixture::new().await?;
    let _doc_ids = fixture.seed_search_data().await?;

    // "ownership borrowing lifetimes" — only "Rust Programming Guide" has all three terms.
    // The 15% score threshold should prune docs that only weakly token-match one term.
    let (status, response) = fixture
        .search("ownership borrowing lifetimes", Some("fulltext"), None)
        .await?;
    assert_eq!(status, StatusCode::OK);
    let titles = result_titles(&response);
    assert!(
        !titles.is_empty(),
        "Expected results for 'ownership borrowing lifetimes'"
    );
    assert_eq!(
        titles[0], "Rust Programming Guide",
        "Rust Programming Guide should be first for 'ownership borrowing lifetimes', got: {:?}",
        titles
    );
    // The threshold should keep the result set small — only docs scoring >= 15% of the top score
    assert!(
        titles.len() <= 5,
        "Expected at most 5 results after score threshold pruning for a very specific query, got {}: {:?}",
        titles.len(),
        titles
    );

    Ok(())
}

#[tokio::test]
async fn test_recency_boosting() -> Result<()> {
    let fixture = SearcherTestFixture::new().await?;
    let _doc_ids = fixture.seed_search_data().await?;

    let source_id = "01JGF7V3E0Y2R1X8P5Q7W9T4N7";
    let pool = fixture.test_env.db_pool.pool();

    // Insert two documents with the same unique keyword "xylophone" but different
    // metadata.updated_at timestamps (recency is determined by metadata, not the DB column).
    // Doc A: metadata says updated today. Doc B: metadata says updated 365 days ago.
    // With recency boosting (weight=0.2, half_life=30d), Doc A should score higher.
    let content_storage = shared::ContentStorage::new(pool.clone());
    let now = chrono::Utc::now();
    let old = now - chrono::Duration::days(365);
    for (ext_id, title, ts) in [
        (
            "recency_recent",
            "Recent Xylophone Manual",
            now.format("%Y-%m-%dT%H:%M:%SZ").to_string(),
        ),
        (
            "recency_old",
            "Old Xylophone Manual",
            old.format("%Y-%m-%dT%H:%M:%SZ").to_string(),
        ),
    ] {
        let doc_id = ulid::Ulid::new().to_string();
        let content = "The xylophone is a musical instrument in the percussion family. This manual covers xylophone tuning, maintenance, and performance techniques.";
        let content_id = content_storage.store_text(content.to_string()).await?;
        let metadata = json!({"updated_at": ts});
        sqlx::query(
            r#"
            INSERT INTO documents (id, source_id, external_id, title, content_id, content_type, content, metadata, permissions, created_at, updated_at)
            VALUES ($1, $2, $3, $4, $5, 'documentation', $6, $7, '{"users":["user1"]}', NOW(), NOW())
            "#,
        )
        .bind(&doc_id)
        .bind(source_id)
        .bind(ext_id)
        .bind(title)
        .bind(&content_id)
        .bind(content)
        .bind(&metadata)
        .execute(pool)
        .await?;
    }

    let (status, response) = fixture
        .search("xylophone manual", Some("fulltext"), None)
        .await?;
    assert_eq!(status, StatusCode::OK);

    let titles = result_titles(&response);
    assert_eq!(
        titles.len(),
        2,
        "Expected 2 results for 'xylophone manual', got: {:?}",
        titles
    );
    assert_eq!(
        titles[0], "Recent Xylophone Manual",
        "Recent document should rank first due to recency boost, got: {:?}",
        titles
    );

    // Verify the recent doc actually has a higher score
    let results = response["results"].as_array().unwrap();
    let recent_score = results[0]["score"].as_f64().unwrap();
    let old_score = results[1]["score"].as_f64().unwrap();
    assert!(
        recent_score > old_score,
        "Recent doc score ({}) should be higher than old doc score ({})",
        recent_score,
        old_score
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

async fn seed_people(pool: &sqlx::PgPool) {
    let person_repo = PersonRepository::new(pool);
    person_repo
        .upsert_people_batch(&[
            PersonUpsert {
                email: "alice.smith@example.com".to_string(),
                display_name: Some("Alice Smith".to_string()),
            },
            PersonUpsert {
                email: "bob.jones@example.com".to_string(),
                display_name: Some("Bob Jones".to_string()),
            },
            PersonUpsert {
                email: "sam.wilson@example.com".to_string(),
                display_name: Some("Sam Wilson".to_string()),
            },
            PersonUpsert {
                email: "samantha.lee@example.com".to_string(),
                display_name: Some("Samantha Lee".to_string()),
            },
        ])
        .await
        .expect("Failed to seed people");
}

#[tokio::test]
async fn test_person_search_by_name() -> Result<()> {
    let fixture = SearcherTestFixture::new().await?;
    let pool = fixture.test_env.db_pool.pool();
    seed_people(pool).await;

    let person_repo = PersonRepository::new(pool);

    // Search for "sam" — should match Sam Wilson (via email token "sam")
    let results = person_repo.search_people("sam", 10).await?;
    assert!(
        !results.is_empty(),
        "Expected at least 1 result for 'sam', got 0",
    );
    let emails: Vec<&str> = results.iter().map(|r| r.email.as_str()).collect();
    assert!(emails.contains(&"sam.wilson@example.com"));

    // Search for "samantha" — should match Samantha Lee
    let results = person_repo.search_people("samantha", 10).await?;
    assert!(!results.is_empty(), "Expected results for 'samantha'");
    assert_eq!(results[0].email, "samantha.lee@example.com");

    // Search for "alice" — should match Alice Smith
    let results = person_repo.search_people("alice", 10).await?;
    assert!(!results.is_empty(), "Expected results for 'alice'");
    assert_eq!(results[0].email, "alice.smith@example.com");

    // Search for a non-existent name
    let results = person_repo.search_people("zzzznotaperson", 10).await?;
    assert!(results.is_empty());

    Ok(())
}

#[tokio::test]
async fn test_person_is_known() -> Result<()> {
    let fixture = SearcherTestFixture::new().await?;
    let pool = fixture.test_env.db_pool.pool();
    seed_people(pool).await;

    let person_repo = PersonRepository::new(pool);

    assert!(person_repo.is_known_person("alice").await?);
    assert!(person_repo.is_known_person("bob").await?);
    assert!(person_repo.is_known_person("sam").await?);
    assert!(!person_repo.is_known_person("zzzznotaperson").await?);

    Ok(())
}

#[tokio::test]
async fn test_people_search_endpoint() -> Result<()> {
    let fixture = SearcherTestFixture::new().await?;
    seed_people(fixture.test_env.db_pool.pool()).await;

    let request = Request::builder()
        .method(Method::GET)
        .uri("/people/search?q=sam&limit=10")
        .body(Body::empty())?;

    let response = fixture.app.clone().oneshot(request).await?;
    assert_eq!(response.status(), StatusCode::OK);

    let body = axum::body::to_bytes(response.into_body(), usize::MAX).await?;
    let json: Value = serde_json::from_slice(&body)?;

    let people = json["people"].as_array().expect("Expected people array");
    assert!(
        !people.is_empty(),
        "Expected at least 1 person for 'sam', got 0",
    );

    // Verify response structure
    let first = &people[0];
    assert!(first.get("id").is_some());
    assert!(first.get("email").is_some());
    assert!(first.get("score").is_some());

    Ok(())
}

// ============================================================================
// Group Permission Tests
// ============================================================================

const TEST_SOURCE_ID: &str = "01JGF7V3E0Y2R1X8P5Q7W9T4N7";

/// Insert a document with specific permissions for group permission testing
async fn insert_group_test_document(
    pool: &sqlx::PgPool,
    external_id: &str,
    title: &str,
    content: &str,
    permissions: DocumentPermissions,
) -> String {
    let doc_id = ulid::Ulid::new().to_string();
    let content_storage = shared::ContentStorage::new(pool.clone());
    let content_id = content_storage
        .store_text(content.to_string())
        .await
        .unwrap();
    let permissions_json = serde_json::to_value(&permissions).unwrap();

    sqlx::query(
        r#"
        INSERT INTO documents (id, source_id, external_id, title, content_id, content_type, content, metadata, permissions, attributes, created_at, updated_at)
        VALUES ($1, $2, $3, $4, $5, 'document', $6, '{}', $7, '{}', NOW(), NOW())
        "#,
    )
    .bind(&doc_id)
    .bind(TEST_SOURCE_ID)
    .bind(external_id)
    .bind(title)
    .bind(&content_id)
    .bind(content)
    .bind(&permissions_json)
    .execute(pool)
    .await
    .unwrap();

    doc_id
}

#[tokio::test]
async fn test_search_respects_group_permissions() -> Result<()> {
    let fixture = SearcherTestFixture::new().await?;
    let pool = fixture.test_env.db_pool.pool();

    // Insert a document only accessible to the "engineering" group
    insert_group_test_document(
        pool,
        "group-doc-1",
        "Secret Engineering Architecture",
        "This is a secret engineering architecture document about microservices",
        DocumentPermissions {
            public: false,
            users: vec![],
            groups: vec!["engineering@example.com".into()],
        },
    )
    .await;

    // Set up group membership: alice is in engineering, bob is not
    let group_repo = GroupRepository::new(pool);
    let group = group_repo
        .upsert_group(
            TEST_SOURCE_ID,
            "engineering@example.com",
            Some("Engineering"),
            None,
        )
        .await?;
    group_repo
        .sync_group_members(&group.id, &["alice@example.com".into()])
        .await?;

    // Alice (in engineering group) should find the document
    let (status, body) = fixture
        .search_with_user(
            "secret engineering architecture",
            Some("fulltext"),
            Some(10),
            Some("alice@example.com"),
        )
        .await?;
    assert_eq!(status, StatusCode::OK);
    let results = body["results"].as_array().unwrap();
    assert!(
        !results.is_empty(),
        "alice (group member) should find the group-shared document"
    );

    // Bob (not in any group) should NOT find the document
    let (status, body) = fixture
        .search_with_user(
            "secret engineering architecture",
            Some("fulltext"),
            Some(10),
            Some("bob@example.com"),
        )
        .await?;
    assert_eq!(status, StatusCode::OK);
    let results = body["results"].as_array().unwrap();
    assert!(
        results.is_empty(),
        "bob (not in group) should NOT find the group-shared document"
    );

    Ok(())
}

#[tokio::test]
async fn test_search_domain_wide_access() -> Result<()> {
    let fixture = SearcherTestFixture::new().await?;
    let pool = fixture.test_env.db_pool.pool();

    // Insert a document shared with the entire example.com domain
    insert_group_test_document(
        pool,
        "domain-doc-1",
        "Company Wide Quarterly Results Announcement",
        "This document contains company wide quarterly results announcement",
        DocumentPermissions {
            public: false,
            users: vec![],
            groups: vec!["example.com".into()],
        },
    )
    .await;

    // alice@example.com should find it (domain match)
    let (status, body) = fixture
        .search_with_user(
            "company wide quarterly results",
            Some("fulltext"),
            Some(10),
            Some("alice@example.com"),
        )
        .await?;
    assert_eq!(status, StatusCode::OK);
    let results = body["results"].as_array().unwrap();
    assert!(
        !results.is_empty(),
        "alice@example.com should find the domain-shared document"
    );

    // alice@other.com should NOT find it
    let (status, body) = fixture
        .search_with_user(
            "company wide quarterly results",
            Some("fulltext"),
            Some(10),
            Some("alice@other.com"),
        )
        .await?;
    assert_eq!(status, StatusCode::OK);
    let results = body["results"].as_array().unwrap();
    assert!(
        results.is_empty(),
        "alice@other.com should NOT find the domain-shared document"
    );

    Ok(())
}

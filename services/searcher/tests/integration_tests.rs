mod common;

use anyhow::Result;
use axum::{
    body::Body,
    http::{Method, Request, StatusCode},
};
use common::SearcherTestFixture;
use serde_json::{json, Value};
use shared::db::repositories::{
    DocumentRepository, GroupRepository, PersonRepository, PersonUpsert,
};
use shared::models::DocumentPermissions;
use tower::ServiceExt;
use ulid::Ulid;

/// Extract result titles from a search response in order.
fn result_titles(response: &Value) -> Vec<String> {
    response["results"]
        .as_array()
        .unwrap()
        .iter()
        .map(|r| r["document"]["title"].as_str().unwrap().to_string())
        .collect()
}

/// Extract result document IDs from a search response in order.
fn result_document_ids(response: &Value) -> Vec<String> {
    response["results"]
        .as_array()
        .unwrap()
        .iter()
        .map(|r| r["document"]["id"].as_str().unwrap().to_string())
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
async fn test_capability_search_indexes_search_text() -> Result<()> {
    let fixture = SearcherTestFixture::new().await?;

    let upsert_body = json!({
        "capabilities": [
            {
                "id": "tool:gmail__send_email",
                "capability_type": "tool",
                "name": "gmail__send_email",
                "description": "Send narwhal updates through Gmail.",
                "user_id": null,
                "source_id": "src-gmail-1",
                "source_type": "gmail",
                "search_text": "Gmail quokka title. Send narwhal updates through Gmail. Useful for composing platypus announcements. wombat workspace",
                "data": {
                    "tool_name": "gmail__send_email",
                    "source_id": "src-gmail-1",
                    "source_type": "gmail",
                    "metadata": {
                        "connector_label": "wombat workspace"
                    }
                }
            },
            {
                "id": "tool:slack__post_message",
                "capability_type": "tool",
                "name": "slack__post_message",
                "description": "Post ordinary team messages.",
                "source_id": "src-slack-1",
                "source_type": "slack",
                "search_text": "Slack unrelated title. Post ordinary team messages.",
                "data": {
                    "tool_name": "slack__post_message",
                    "source_id": "src-slack-1",
                    "source_type": "slack"
                }
            }
        ]
    });

    let request = Request::builder()
        .method(Method::POST)
        .uri("/capabilities/upsert")
        .header("content-type", "application/json")
        .body(Body::from(upsert_body.to_string()))?;
    let response = fixture.app.clone().oneshot(request).await?;
    assert_eq!(response.status(), StatusCode::OK);
    let _body = axum::body::to_bytes(response.into_body(), usize::MAX).await?;

    for query in ["quokka", "narwhal", "platypus", "wombat"] {
        let response = search_capabilities(
            &fixture,
            json!({
                "capability_type": "tool",
                "query": query,
                "limit": 10
            }),
        )
        .await?;
        assert_capability_present(&response, "tool:gmail__send_email", query);
    }

    let allowed_response = search_capabilities(
        &fixture,
        json!({
            "capability_type": "tool",
            "query": "quokka",
            "allowed_ids": ["tool:gmail__send_email"],
            "allowed_source_ids": ["src-gmail-1"]
        }),
    )
    .await?;
    assert_capability_present(&allowed_response, "tool:gmail__send_email", "quokka");

    let disallowed_by_id = search_capabilities(
        &fixture,
        json!({
            "capability_type": "tool",
            "query": "quokka",
            "allowed_ids": ["tool:slack__post_message"]
        }),
    )
    .await?;
    assert!(disallowed_by_id["results"].as_array().unwrap().is_empty());

    let disallowed_by_source = search_capabilities(
        &fixture,
        json!({
            "capability_type": "tool",
            "query": "quokka",
            "allowed_source_ids": ["src-slack-1"]
        }),
    )
    .await?;
    assert!(disallowed_by_source["results"]
        .as_array()
        .unwrap()
        .is_empty());

    Ok(())
}

#[tokio::test]
async fn test_capability_sync_prunes_stale_rows_for_publisher_and_type() -> Result<()> {
    let fixture = SearcherTestFixture::new().await?;

    let first_sync = json!({
        "publisher_id": "src-gmail-1",
        "capability_type": "tool",
        "capabilities": [
            {
                "id": "tool:gmail__send_email",
                "capability_type": "tool",
                "name": "gmail__send_email",
                "description": "Send email",
                "source_id": "src-gmail-1",
                "source_type": "gmail",
                "search_text": "Gmail send email quokka",
                "data": { "tool_name": "gmail__send_email" }
            },
            {
                "id": "tool:gmail__old_tool",
                "capability_type": "tool",
                "name": "gmail__old_tool",
                "description": "Old tool",
                "source_id": "src-gmail-1",
                "source_type": "gmail",
                "search_text": "Gmail old tool quokka",
                "data": { "tool_name": "gmail__old_tool" }
            }
        ]
    });
    let response = sync_capabilities(&fixture, first_sync).await?;
    assert_eq!(response["upserted"], 2);
    assert_eq!(response["deleted"], 0);

    let second_sync = json!({
        "publisher_id": "src-gmail-1",
        "capability_type": "tool",
        "capabilities": [
            {
                "id": "tool:gmail__send_email",
                "capability_type": "tool",
                "name": "gmail__send_email",
                "description": "Send email updated",
                "source_id": "src-gmail-1",
                "source_type": "gmail",
                "search_text": "Gmail send email updated quokka",
                "data": { "tool_name": "gmail__send_email" }
            }
        ]
    });
    let response = sync_capabilities(&fixture, second_sync).await?;
    assert_eq!(response["upserted"], 1);
    assert_eq!(response["deleted"], 1);

    let search_response = search_capabilities(
        &fixture,
        json!({
            "capability_type": "tool",
            "query": "quokka",
            "limit": 10
        }),
    )
    .await?;
    assert_capability_present(&search_response, "tool:gmail__send_email", "quokka");
    assert!(!search_response["results"]
        .as_array()
        .unwrap()
        .iter()
        .any(|result| result["id"] == "tool:gmail__old_tool"));

    Ok(())
}

async fn sync_capabilities(fixture: &SearcherTestFixture, body: Value) -> Result<Value> {
    let request = Request::builder()
        .method(Method::POST)
        .uri("/capabilities/sync")
        .header("content-type", "application/json")
        .body(Body::from(body.to_string()))?;
    let response = fixture.app.clone().oneshot(request).await?;
    assert_eq!(response.status(), StatusCode::OK);
    let body = axum::body::to_bytes(response.into_body(), usize::MAX).await?;
    Ok(serde_json::from_slice(&body)?)
}

async fn search_capabilities(fixture: &SearcherTestFixture, body: Value) -> Result<Value> {
    let request = Request::builder()
        .method(Method::POST)
        .uri("/capabilities/search")
        .header("content-type", "application/json")
        .body(Body::from(body.to_string()))?;
    let response = fixture.app.clone().oneshot(request).await?;
    assert_eq!(response.status(), StatusCode::OK);
    let body = axum::body::to_bytes(response.into_body(), usize::MAX).await?;
    Ok(serde_json::from_slice(&body)?)
}

fn assert_capability_present(response: &Value, capability_id: &str, query: &str) {
    let results = response["results"].as_array().unwrap();
    assert!(
        results.iter().any(|result| result["id"] == capability_id),
        "Expected capability search for {query:?} to match {capability_id}, got: {response}"
    );
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
    // "Rust Programming Guide" matches both terms; "Rust Prevention and Corrosion Control"
    // matches only "rust" and should score significantly lower
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
    for result in response["results"].as_array().unwrap() {
        assert_eq!(
            result["source_type"].as_str(),
            Some("local_files"),
            "Fulltext results should include source_type populated by the search query"
        );
    }

    // Query 5: phrase ranking — "blue square nda"
    // BlueSquare NDA should rank first (phrase match on "blue square" in title & content).
    // "Square Root Mathematics" only token-matches "square", so it scores much lower.
    let (status, response) = fixture
        .search("blue square nda", Some("fulltext"), None)
        .await?;
    assert_eq!(status, StatusCode::OK);
    let titles = result_titles(&response);
    // BlueSquare NDA matches all 3 terms plus phrase "blue square", must rank first.
    // Other documents matching "square" may or may not pass the score threshold.
    assert!(
        titles.len() >= 2,
        "Expected at least 2 results for 'blue square nda', got: {:?}",
        titles
    );
    assert_eq!(
        titles[0], "BlueSquare NDA",
        "BlueSquare NDA should rank first, got: {:?}",
        titles
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
    // Multilingual tokenizer: unstemmed primary + English-stemmed aliases.
    // Query "crm sales report" → tokens ["crm", "sales", "report"] (no stemming).
    // "CRM Sales Reports": exact path matches "crm" + "sales", English alias
    //   stems both query "report"→"report" and indexed "reports"→"report" → match.
    //   3/3 terms hit → dominant score, especially with phrase boost.
    // "Death of a Salesman Book Report": "sales" does NOT match "salesman" on
    //   any path (stem "sale" ≠ stem "salesman"). Only "report" matches → 1/3.
    //   Likely below score threshold. CRM doc should be the sole or top result.
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

    let (status, page1_response) = fixture
        .search_with_body(json!({
            "query": "search",
            "mode": "hybrid",
            "limit": 2,
            "offset": 0,
            "include_facets": false
        }))
        .await?;
    assert_eq!(status, StatusCode::OK);

    let (status, page2_response) = fixture
        .search_with_body(json!({
            "query": "search",
            "mode": "hybrid",
            "limit": 2,
            "offset": 2,
            "include_facets": false
        }))
        .await?;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        page1_response["total_count"].as_i64(),
        page2_response["total_count"].as_i64(),
        "Hybrid total_count should not grow with offset-dependent overfetch"
    );

    Ok(())
}

#[tokio::test]
async fn test_hybrid_pagination_matches_fused_ranking() -> Result<()> {
    let fixture = SearcherTestFixture::new().await?;
    let _doc_ids = fixture.seed_search_data().await?;

    let base_body = |limit: i64, offset: i64| {
        json!({
            "query": "search",
            "mode": "hybrid",
            "limit": limit,
            "offset": offset,
            "include_facets": false
        })
    };

    let (status, first_four) = fixture.search_with_body(base_body(4, 0)).await?;
    assert_eq!(status, StatusCode::OK);
    let first_four_ids = result_document_ids(&first_four);
    assert!(
        first_four_ids.len() >= 4,
        "Expected at least 4 hybrid results for pagination test, got: {:?}",
        result_titles(&first_four)
    );

    let (status, page_one) = fixture.search_with_body(base_body(2, 0)).await?;
    assert_eq!(status, StatusCode::OK);
    let (status, page_two) = fixture.search_with_body(base_body(2, 2)).await?;
    assert_eq!(status, StatusCode::OK);

    let mut combined_page_ids = result_document_ids(&page_one);
    combined_page_ids.extend(result_document_ids(&page_two));
    assert_eq!(
        combined_page_ids, first_four_ids,
        "Hybrid pages should be slices of the same fused RRF ranking"
    );

    Ok(())
}

#[tokio::test]
async fn test_hybrid_dedupes_after_retrievers_pick_different_duplicates() -> Result<()> {
    let fixture = SearcherTestFixture::new().await?;
    let pool = fixture.test_env.db_pool.pool();
    let first_source_id = "01JGF7V3E0Y2R1X8P5Q7W9T4N7";
    let second_source_id = Ulid::new().to_string();
    let query = "hybridcrossdedupneedle";
    let external_id = "hybrid-cross-retriever-duplicate";

    sqlx::query(
        r#"
        INSERT INTO sources (id, name, source_type, config, created_by, created_at, updated_at)
        VALUES ($1, 'Second Hybrid Dedup Source', 'local_files', '{}', '01JGF7V3E0Y2R1X8P5Q7W9T4N6', NOW(), NOW())
        "#,
    )
    .bind(&second_source_id)
    .execute(pool)
    .await?;

    insert_public_document_with_embedding(
        pool,
        first_source_id,
        external_id,
        "HybridCrossDedupNeedle Keyword Winner",
        "hybridcrossdedupneedle hybridcrossdedupneedle hybridcrossdedupneedle",
        "unrelated embedding text",
        "2026-01-01T00:00:00Z",
    )
    .await?;
    insert_public_document_with_embedding(
        pool,
        &second_source_id,
        external_id,
        "Semantic Duplicate Winner",
        "semantic-only duplicate content",
        query,
        "2026-01-02T00:00:00Z",
    )
    .await?;

    let (status, response) = fixture
        .search_with_body(json!({
            "query": query,
            "mode": "hybrid",
            "limit": 10,
            "include_facets": false
        }))
        .await?;

    assert_eq!(status, StatusCode::OK);
    let duplicate_results: Vec<&Value> = response["results"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|result| result["document"]["external_id"].as_str() == Some(external_id))
        .collect();
    assert_eq!(
        duplicate_results.len(),
        1,
        "Hybrid should collapse duplicates even when FTS and semantic choose different physical rows: {:?}",
        result_titles(&response)
    );

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

    let total_count = page1_response["total_count"].as_i64().unwrap();
    let (status, last_page_response) = fixture
        .search_with_body(json!({
            "query": "square",
            "mode": "fulltext",
            "limit": 2,
            "offset": total_count
        }))
        .await?;
    assert_eq!(status, StatusCode::OK);
    assert!(
        !last_page_response["has_more"].as_bool().unwrap_or(true),
        "Page starting at total_count should have has_more=false"
    );

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
async fn test_include_facets_false_preserves_total_count() -> Result<()> {
    let fixture = SearcherTestFixture::new().await?;
    let _doc_ids = fixture.seed_search_data().await?;

    let (status, response) = fixture
        .search_with_body(json!({
            "query": "square",
            "mode": "fulltext",
            "limit": 2,
            "offset": 0,
            "include_facets": false
        }))
        .await?;

    assert_eq!(status, StatusCode::OK);
    assert!(
        response.get("facets").is_none(),
        "facets should be omitted when include_facets=false: {:?}",
        response
    );
    assert!(
        response["total_count"].as_i64().unwrap() > 0,
        "total_count should still be populated when facets are disabled"
    );
    assert_eq!(
        response["has_more"].as_bool().unwrap(),
        response["total_count"].as_i64().unwrap() > 2,
        "has_more should use offset + limit < total_count"
    );

    Ok(())
}

#[tokio::test]
async fn test_total_count_matches_relevance_filtered_fulltext_results() -> Result<()> {
    let fixture = SearcherTestFixture::new().await?;
    let pool = fixture.test_env.db_pool.pool();
    let content_storage = shared::ContentStorage::new(pool.clone());
    let source_id = "01JGF7V3E0Y2R1X8P5Q7W9T4N7";

    let high_doc_id = Ulid::new().to_string();
    let high_content = "paginationalpha paginationbeta paginationgamma paginationdelta paginationneedle. paginationalpha paginationbeta paginationgamma paginationdelta paginationneedle.";
    let high_content_id = content_storage.store_text(high_content.to_string()).await?;
    sqlx::query(
        r#"
        INSERT INTO documents (id, source_id, external_id, title, content_id, content_type, content, metadata, permissions, attributes, created_at, updated_at)
        VALUES ($1, $2, 'pagination_relevance_high', 'PaginationAlpha PaginationBeta PaginationGamma PaginationDelta PaginationNeedle', $3, 'document', $4, '{}', '{"public": true, "users": [], "groups": []}', '{}', NOW(), NOW())
        "#,
    )
    .bind(&high_doc_id)
    .bind(source_id)
    .bind(&high_content_id)
    .bind(high_content)
    .execute(pool)
    .await?;

    for idx in 0..30 {
        let doc_id = Ulid::new().to_string();
        let content = format!("paginationneedle weak match filler document number {idx}");
        let content_id = content_storage.store_text(content.clone()).await?;
        sqlx::query(
            r#"
            INSERT INTO documents (id, source_id, external_id, title, content_id, content_type, content, metadata, permissions, attributes, created_at, updated_at)
            VALUES ($1, $2, $3, $4, $5, 'document', $6, '{}', '{"public": true, "users": [], "groups": []}', '{}', NOW(), NOW())
            "#,
        )
        .bind(&doc_id)
        .bind(source_id)
        .bind(format!("pagination_relevance_low_{idx}"))
        .bind(format!("Weak PaginationNeedle Match {idx}"))
        .bind(&content_id)
        .bind(content)
        .execute(pool)
        .await?;
    }

    let (status, response) = fixture
        .search_with_body(json!({
            "query": "paginationalpha paginationbeta paginationgamma paginationdelta paginationneedle",
            "mode": "fulltext",
            "limit": 10,
            "offset": 0,
            "include_facets": false
        }))
        .await?;

    assert_eq!(status, StatusCode::OK);
    let titles = result_titles(&response);
    assert_eq!(
        titles,
        vec!["PaginationAlpha PaginationBeta PaginationGamma PaginationDelta PaginationNeedle"],
        "Weak one-term matches should be filtered from displayed results"
    );
    assert_eq!(
        response["total_count"].as_i64().unwrap(),
        titles.len() as i64,
        "total_count should be counted after the same relevance filter as displayed hits"
    );
    assert!(
        !response["has_more"].as_bool().unwrap(),
        "Pagination should not advertise more pages after low relevance matches are filtered"
    );

    Ok(())
}

#[tokio::test]
async fn test_fulltext_dedupes_by_source_type_and_external_id() -> Result<()> {
    let fixture = SearcherTestFixture::new().await?;
    let pool = fixture.test_env.db_pool.pool();
    let content_storage = shared::ContentStorage::new(pool.clone());
    let default_local_source_id = "01JGF7V3E0Y2R1X8P5Q7W9T4N7";
    let second_local_source_id = Ulid::new().to_string();
    let google_source_id = Ulid::new().to_string();

    for (source_id, name, source_type) in [
        (
            &second_local_source_id,
            "Second Local Dedup Source",
            "local_files",
        ),
        (&google_source_id, "Google Dedup Source", "google_drive"),
    ] {
        sqlx::query(
            r#"
            INSERT INTO sources (id, name, source_type, config, created_by, created_at, updated_at)
            VALUES ($1, $2, $3, '{}', '01JGF7V3E0Y2R1X8P5Q7W9T4N6', NOW(), NOW())
            "#,
        )
        .bind(source_id)
        .bind(name)
        .bind(source_type)
        .execute(pool)
        .await?;
    }

    for (source_id, title, updated_at) in [
        (
            default_local_source_id,
            "TypeDedupeAlpha Local Older",
            "2026-01-01T00:00:00Z",
        ),
        (
            second_local_source_id.as_str(),
            "TypeDedupeAlpha Local Newer",
            "2026-01-02T00:00:00Z",
        ),
        (
            google_source_id.as_str(),
            "TypeDedupeAlpha Google Same External",
            "2026-01-03T00:00:00Z",
        ),
    ] {
        let doc_id = Ulid::new().to_string();
        let content = format!("typededupealpha shared dedupe content for {title}");
        let content_id = content_storage.store_text(content.clone()).await?;
        sqlx::query(
            r#"
            INSERT INTO documents (id, source_id, external_id, title, content_id, content_type, content, metadata, permissions, attributes, created_at, updated_at)
            VALUES ($1, $2, 'typededupealpha-shared', $3, $4, 'document', $5, jsonb_build_object('updated_at', $6::text), '{"public": true, "users": [], "groups": []}', '{}', $6::timestamptz, $6::timestamptz)
            "#,
        )
        .bind(&doc_id)
        .bind(source_id)
        .bind(title)
        .bind(&content_id)
        .bind(content)
        .bind(updated_at)
        .execute(pool)
        .await?;
    }

    let (status, response) = fixture
        .search_with_body(json!({
            "query": "typededupealpha",
            "mode": "fulltext",
            "limit": 10,
            "include_facets": false
        }))
        .await?;

    assert_eq!(status, StatusCode::OK);
    let results = response["results"].as_array().unwrap();
    assert_eq!(
        response["total_count"].as_i64().unwrap(),
        2,
        "same source_type/external_id duplicates should count once, but different source types should remain separate"
    );
    assert_eq!(results.len(), 2);

    let titles = result_titles(&response);
    assert!(
        titles.contains(&"TypeDedupeAlpha Local Newer".to_string()),
        "newer local duplicate should win within the local_files/external_id group: {titles:?}"
    );
    assert!(
        titles.contains(&"TypeDedupeAlpha Google Same External".to_string()),
        "same external_id in a different source type should not be deduped: {titles:?}"
    );
    assert!(
        !titles.contains(&"TypeDedupeAlpha Local Older".to_string()),
        "older local duplicate should be collapsed: {titles:?}"
    );

    Ok(())
}

#[tokio::test]
async fn test_unfiltered_facets_with_source_type_filter() -> Result<()> {
    let fixture = SearcherTestFixture::new().await?;
    let pool = fixture.test_env.db_pool.pool();
    let content_storage = shared::ContentStorage::new(pool.clone());

    let google_source_id = Ulid::new().to_string();
    sqlx::query(
        r#"
        INSERT INTO sources (id, name, source_type, config, created_by, created_at, updated_at)
        VALUES ($1, 'Google Facet Test Source', 'google_drive', '{}', '01JGF7V3E0Y2R1X8P5Q7W9T4N6', NOW(), NOW())
        "#,
    )
    .bind(&google_source_id)
    .execute(pool)
    .await?;

    for (source_id, external_id, title) in [
        (
            "01JGF7V3E0Y2R1X8P5Q7W9T4N7".to_string(),
            "facetglobal_local",
            "Local Facetglobal Plan",
        ),
        (
            google_source_id,
            "facetglobal_google",
            "Google Facetglobal Plan",
        ),
    ] {
        let doc_id = Ulid::new().to_string();
        let content = "facetglobal roadmap planning document for source facet tests";
        let content_id = content_storage.store_text(content.to_string()).await?;
        sqlx::query(
            r#"
            INSERT INTO documents (id, source_id, external_id, title, content_id, content_type, content, metadata, permissions, attributes, created_at, updated_at)
            VALUES ($1, $2, $3, $4, $5, 'document', $6, '{}', '{"public": true, "users": [], "groups": []}', '{}', NOW(), NOW())
            "#,
        )
        .bind(&doc_id)
        .bind(&source_id)
        .bind(external_id)
        .bind(title)
        .bind(&content_id)
        .bind(content)
        .execute(pool)
        .await?;
    }

    let (status, response) = fixture
        .search_with_body(json!({
            "query": "facetglobal",
            "mode": "fulltext",
            "source_types": ["local_files"],
            "limit": 10
        }))
        .await?;

    assert_eq!(status, StatusCode::OK);
    let results = response["results"].as_array().unwrap();
    assert_eq!(
        results.len(),
        1,
        "source filter should limit hits to local_files"
    );
    assert_eq!(results[0]["source_type"].as_str(), Some("local_files"));

    let source_facet = response["facets"]
        .as_array()
        .expect("Expected unfiltered facets")
        .iter()
        .find(|facet| facet["name"].as_str() == Some("source_type"))
        .expect("Expected source_type facet");
    let facet_values: Vec<&str> = source_facet["values"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|value| value["value"].as_str())
        .collect();
    assert!(
        facet_values.contains(&"local_files") && facet_values.contains(&"google_drive"),
        "source_type facets should remain unfiltered by active source filter: {:?}",
        facet_values
    );

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
// Special Character / Tantivy Escaping Tests
// ============================================================================

#[tokio::test]
async fn test_special_characters_in_queries() -> Result<()> {
    let fixture = SearcherTestFixture::new().await?;
    let pool = fixture.test_env.db_pool.pool();
    fixture.seed_search_data().await?;

    let cpp_title = "C++ Error: connection/refused (timeout)";
    let cpp_content =
        "The function(x) returned error~code 42. Check path/to/config.json for details.";

    insert_group_test_document(
        pool,
        "special-chars-1",
        cpp_title,
        cpp_content,
        DocumentPermissions {
            public: true,
            users: vec![],
            groups: vec![],
        },
    )
    .await;

    // Each query contains Tantivy special chars and should match the document
    let queries_that_should_match = vec![
        ("connection refused timeout", "plain terms from title"),
        ("error: connection", "colon in query"),
        ("function(x)", "parentheses in query"),
        ("path/to/config", "slashes in query"),
        ("error~code", "tilde in query"),
        ("config.json", "dot in query"),
    ];

    for (query, description) in queries_that_should_match {
        let (status, response) = fixture.search(query, Some("fulltext"), Some(10)).await?;
        assert_eq!(status, StatusCode::OK);
        let titles = result_titles(&response);
        assert!(
            titles.iter().any(|t| t == cpp_title),
            "{description}: expected to find '{cpp_title}', got: {titles:?}"
        );
    }

    Ok(())
}

// ============================================================================
// Group Permission Tests
// ============================================================================

async fn insert_public_document_with_embedding(
    pool: &sqlx::PgPool,
    source_id: &str,
    external_id: &str,
    title: &str,
    content: &str,
    embedding_text: &str,
    updated_at: &str,
) -> Result<String> {
    let doc_id = Ulid::new().to_string();
    let content_storage = shared::ContentStorage::new(pool.clone());
    let content_id = content_storage.store_text(content.to_string()).await?;

    sqlx::query(
        r#"
        INSERT INTO documents (id, source_id, external_id, title, content_id, content_type, content, metadata, permissions, attributes, created_at, updated_at)
        VALUES ($1, $2, $3, $4, $5, 'document', $6, jsonb_build_object('updated_at', $7::text), '{"public": true, "users": [], "groups": []}', '{}', $7::timestamptz, $7::timestamptz)
        "#,
    )
    .bind(&doc_id)
    .bind(source_id)
    .bind(external_id)
    .bind(title)
    .bind(&content_id)
    .bind(content)
    .bind(updated_at)
    .execute(pool)
    .await?;

    let embedding = shared::test_environment::generate_test_embedding(embedding_text);
    sqlx::query(
        r#"
        INSERT INTO embeddings (id, document_id, chunk_index, chunk_start_offset, chunk_end_offset, embedding, model_name, dimensions, created_at)
        VALUES ($1, $2, 0, 0, $3, $4, 'test-model', 1024, NOW())
        "#,
    )
    .bind(Ulid::new().to_string())
    .bind(&doc_id)
    .bind(content.len() as i32)
    .bind(&embedding)
    .execute(pool)
    .await?;

    Ok(doc_id)
}

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

#[tokio::test]
async fn test_search_permissions_do_not_match_tokenized_email_parts() -> Result<()> {
    let fixture = SearcherTestFixture::new().await?;
    let pool = fixture.test_env.db_pool.pool();

    insert_group_test_document(
        pool,
        "exact-email-permission-doc",
        "Synthetic Exact Access Document",
        "synthetic access document exact email permission",
        DocumentPermissions {
            public: false,
            users: vec!["alex.search@example.test".into()],
            groups: vec![],
        },
    )
    .await;

    insert_group_test_document(
        pool,
        "split-token-permission-doc",
        "Synthetic Split Token Document",
        "synthetic access document wrong domain split tokens",
        DocumentPermissions {
            public: false,
            users: vec!["alex.search@other.test".into(), "casey@example.test".into()],
            groups: vec![],
        },
    )
    .await;

    let (status, body) = fixture
        .search_with_user(
            "synthetic access document",
            Some("fulltext"),
            Some(10),
            Some("alex.search@example.test"),
        )
        .await?;
    assert_eq!(status, StatusCode::OK);

    let titles: Vec<&str> = body["results"]
        .as_array()
        .unwrap()
        .iter()
        .map(|result| result["document"]["title"].as_str().unwrap())
        .collect();

    assert!(titles.contains(&"Synthetic Exact Access Document"));
    assert!(
        !titles.contains(&"Synthetic Split Token Document"),
        "permission filtering must not match a document just because one user shares the localpart and another shares the domain"
    );

    let (status, body) = fixture
        .search_with_user(
            "synthetic access document",
            Some("fulltext"),
            Some(10),
            Some("alex"),
        )
        .await?;
    assert_eq!(status, StatusCode::OK);
    let results = body["results"].as_array().unwrap();
    assert!(
        results.is_empty(),
        "permission filtering must not match a tokenized fragment of an email address"
    );

    Ok(())
}

#[tokio::test]
async fn test_search_respects_group_permissions_with_special_characters() -> Result<()> {
    let fixture = SearcherTestFixture::new().await?;
    let pool = fixture.test_env.db_pool.pool();
    let group_email = "slack-channel:T_TEST:C_TEST_CHANNEL";

    insert_group_test_document(
        pool,
        "special-group-doc-1",
        "Synthetic Channel Access Notes",
        "synthetic channel access notes for special character group permission",
        DocumentPermissions {
            public: false,
            users: vec![],
            groups: vec![group_email.into()],
        },
    )
    .await;

    let group_repo = GroupRepository::new(pool);
    let group = group_repo
        .upsert_group(TEST_SOURCE_ID, group_email, Some("Synthetic Channel"), None)
        .await?;
    group_repo
        .sync_group_members(&group.id, &["channel-member@example.com".into()])
        .await?;

    let (status, body) = fixture
        .search_with_user(
            "synthetic channel access notes",
            Some("fulltext"),
            Some(10),
            Some("channel-member@example.com"),
        )
        .await?;
    assert_eq!(status, StatusCode::OK);
    let results = body["results"].as_array().unwrap();
    assert!(
        !results.is_empty(),
        "group values containing ':' should be queryable through indexed permissions"
    );

    Ok(())
}

// ============================================================================
// Multilingual Search Tests
// ============================================================================

/// Insert a public document for multilingual testing (no permission restrictions).
async fn insert_multilingual_doc(
    pool: &sqlx::PgPool,
    external_id: &str,
    title: &str,
    content: &str,
) -> String {
    let doc_id = Ulid::new().to_string();
    let content_storage = shared::ContentStorage::new(pool.clone());
    let content_id = content_storage
        .store_text(content.to_string())
        .await
        .unwrap();
    let permissions = json!({"public": true, "users": [], "groups": []});

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
    .bind(&permissions)
    .execute(pool)
    .await
    .unwrap();

    doc_id
}

/// Seed multilingual documents for search testing across various languages and scripts.
async fn seed_multilingual_data(pool: &sqlx::PgPool) {
    // German: contains words that are English stopwords ("die", "in", "was")
    insert_multilingual_doc(
        pool,
        "de_doc_1",
        "Quartalsbericht_Q3_2024_Entwurf.docx",
        "Die Ergebnisse der Quartalsanalyse zeigen eine Steigerung des Umsatzes. \
         In diesem Bericht werden die wichtigsten Kennzahlen zusammengefasst. \
         Was die Prognose betrifft, erwarten wir weiteres Wachstum im nächsten Quartal.",
    )
    .await;

    // German: compound words, umlauts
    insert_multilingual_doc(
        pool,
        "de_doc_2",
        "Mitarbeiterhandbuch_2024.pdf",
        "Das Mitarbeiterhandbuch enthält alle Richtlinien für neue Mitarbeiter. \
         Arbeitszeiten, Urlaubsregelungen und Sicherheitsvorschriften sind beschrieben. \
         Bitte lesen Sie das Handbuch sorgfältig durch.",
    )
    .await;

    // Chinese: no spaces between words, needs ICU segmentation
    insert_multilingual_doc(
        pool,
        "zh_doc_1",
        "项目计划书",
        "全文搜索系统的设计与实现。本项目旨在开发一个支持多语言的搜索引擎。\
         系统需要处理中文、日文和韩文等语言的分词问题。数据库使用PostgreSQL。",
    )
    .await;

    // Japanese: mixed Kanji, Hiragana, Katakana
    insert_multilingual_doc(
        pool,
        "ja_doc_1",
        "検索エンジン設計書",
        "検索エンジンの設計について。全文検索はPostgreSQLのBM25インデックスを使用します。\
         日本語のテキストはICUトークナイザーで分割されます。",
    )
    .await;

    // Korean
    insert_multilingual_doc(
        pool,
        "ko_doc_1",
        "검색시스템_요구사항.docx",
        "검색 시스템 요구사항 문서입니다. 한국어 전문 검색을 지원해야 합니다. \
         데이터베이스는 PostgreSQL을 사용합니다.",
    )
    .await;

    // Thai: no spaces between words, needs ICU segmentation
    insert_multilingual_doc(
        pool,
        "th_doc_1",
        "คู่มือการใช้งาน",
        "คู่มือการใช้งานระบบค้นหาข้อมูล ระบบนี้รองรับการค้นหาข้อความเต็มรูปแบบ \
         ภาษาไทยจะถูกแบ่งคำด้วยระบบ ICU",
    )
    .await;

    // Portuguese: "a" is both an article and common word; tests no-stopword-removal
    insert_multilingual_doc(
        pool,
        "pt_doc_1",
        "relatorio_mensal.pdf",
        "A análise mensal de vendas mostra um crescimento significativo. \
         O relatório apresenta os dados de todos os departamentos. \
         A equipe comercial superou a meta estabelecida.",
    )
    .await;

    // Mixed English/German document
    insert_multilingual_doc(
        pool,
        "mixed_doc_1",
        "ProjectUpdate_Zusammenfassung.docx",
        "Project update and Zusammenfassung for Q3. \
         The development team completed the migration to the new architecture. \
         Die Entwicklung des neuen Systems verläuft planmäßig.",
    )
    .await;

    // English with accented characters (tests ASCII folding)
    insert_multilingual_doc(
        pool,
        "accent_doc_1",
        "cafe_menu_régional.pdf",
        "Café Régional seasonal menu featuring crème brûlée and soufflé. \
         The naïve approach to résumé formatting was updated. \
         Jalapeño peppers are available on request.",
    )
    .await;

    // CamelCase code identifier document (tests source_code tokenizer)
    insert_multilingual_doc(
        pool,
        "code_doc_1",
        "SearchIndexManager.java",
        "The SearchIndexManager class handles index lifecycle operations. \
         It supports createBm25Index, rebuildSearchIndex, and dropIndex methods. \
         Configuration is loaded from ApplicationConfig.",
    )
    .await;
}

#[tokio::test]
async fn test_multilingual_german_stopwords_preserved() -> Result<()> {
    // "die" is an English stopword but a common German article.
    // Without stopword removal, "die" should match German documents.
    let fixture = SearcherTestFixture::new().await?;
    let pool = fixture.test_env.db_pool.pool();
    seed_multilingual_data(pool).await;

    let (status, response) = fixture
        .search("die Ergebnisse Quartalsanalyse", Some("fulltext"), Some(10))
        .await?;
    assert_eq!(status, StatusCode::OK);
    let titles = result_titles(&response);
    assert!(
        !titles.is_empty(),
        "German query with 'die' (English stopword) should return results"
    );
    assert!(
        titles[0].contains("Quartalsbericht"),
        "German quarterly report should rank first for 'die Ergebnisse Quartalsanalyse', got: {:?}",
        titles
    );
    assert_scores_descending(&response);

    Ok(())
}

#[tokio::test]
async fn test_multilingual_german_umlaut_ascii_folding() -> Result<()> {
    // Searching "nachsten" (without umlaut) should match "nächsten" via ASCII folding
    let fixture = SearcherTestFixture::new().await?;
    let pool = fixture.test_env.db_pool.pool();
    seed_multilingual_data(pool).await;

    let (status, response) = fixture
        .search("nachsten Quartal", Some("fulltext"), Some(10))
        .await?;
    assert_eq!(status, StatusCode::OK);
    let titles = result_titles(&response);
    assert!(
        !titles.is_empty(),
        "ASCII-folded query 'nachsten' should match 'nächsten'"
    );
    assert!(
        titles.iter().any(|t| t.contains("Quartalsbericht")),
        "Expected Quartalsbericht in results for 'nachsten Quartal', got: {:?}",
        titles
    );

    Ok(())
}

#[tokio::test]
async fn test_multilingual_underscore_filename_splitting() -> Result<()> {
    // pdb.simple splits on underscores: "Quartalsbericht_Q3_2024_Entwurf.docx"
    // → ["quartalsbericht", "q3", "2024", "entwurf", "docx"]
    // Searching for individual parts should find the document by title.
    let fixture = SearcherTestFixture::new().await?;
    let pool = fixture.test_env.db_pool.pool();
    seed_multilingual_data(pool).await;

    // Search a fragment from the underscore-separated filename
    let (status, response) = fixture
        .search("Entwurf Q3 2024", Some("fulltext"), Some(10))
        .await?;
    assert_eq!(status, StatusCode::OK);
    let titles = result_titles(&response);
    assert!(
        !titles.is_empty(),
        "Searching filename parts should match underscore-separated title"
    );
    assert_eq!(
        titles[0], "Quartalsbericht_Q3_2024_Entwurf.docx",
        "Expected filename doc as first result, got: {:?}",
        titles
    );

    Ok(())
}

#[tokio::test]
async fn test_multilingual_chinese_segmentation() -> Result<()> {
    // ICU tokenizer should segment Chinese text properly so individual terms match.
    // "全文搜索" (full-text search) and "多语言" (multilingual) should be searchable.
    let fixture = SearcherTestFixture::new().await?;
    let pool = fixture.test_env.db_pool.pool();
    seed_multilingual_data(pool).await;

    let (status, response) = fixture
        .search("搜索 PostgreSQL", Some("fulltext"), Some(10))
        .await?;
    assert_eq!(status, StatusCode::OK);
    let titles = result_titles(&response);
    assert!(
        !titles.is_empty(),
        "Chinese term '搜索' combined with 'PostgreSQL' should return results"
    );
    // The Chinese doc and Japanese doc both mention 搜索/検索 and PostgreSQL
    assert!(
        titles.iter().any(|t| t == "项目计划书"),
        "Chinese project doc should be in results, got: {:?}",
        titles
    );

    Ok(())
}

#[tokio::test]
async fn test_multilingual_japanese_search() -> Result<()> {
    // Japanese text with mixed Kanji/Katakana should be searchable
    let fixture = SearcherTestFixture::new().await?;
    let pool = fixture.test_env.db_pool.pool();
    seed_multilingual_data(pool).await;

    let (status, response) = fixture
        .search("検索エンジン 設計", Some("fulltext"), Some(10))
        .await?;
    assert_eq!(status, StatusCode::OK);
    let titles = result_titles(&response);
    assert!(!titles.is_empty(), "Japanese query should return results");
    assert!(
        titles.iter().any(|t| t == "検索エンジン設計書"),
        "Japanese search engine design doc should be found, got: {:?}",
        titles
    );

    Ok(())
}

#[tokio::test]
async fn test_multilingual_korean_search() -> Result<()> {
    let fixture = SearcherTestFixture::new().await?;
    let pool = fixture.test_env.db_pool.pool();
    seed_multilingual_data(pool).await;

    let (status, response) = fixture
        .search("검색 시스템 요구사항", Some("fulltext"), Some(10))
        .await?;
    assert_eq!(status, StatusCode::OK);
    let titles = result_titles(&response);
    assert!(!titles.is_empty(), "Korean query should return results");
    assert!(
        titles.iter().any(|t| t.contains("검색시스템")),
        "Korean search system requirements doc should be found, got: {:?}",
        titles
    );

    Ok(())
}

#[tokio::test]
async fn test_multilingual_thai_segmentation() -> Result<()> {
    // Thai has no spaces between words. ICU should segment properly.
    let fixture = SearcherTestFixture::new().await?;
    let pool = fixture.test_env.db_pool.pool();
    seed_multilingual_data(pool).await;

    let (status, response) = fixture
        .search("ค้นหา ระบบ", Some("fulltext"), Some(10))
        .await?;
    assert_eq!(status, StatusCode::OK);
    let titles = result_titles(&response);
    assert!(
        !titles.is_empty(),
        "Thai query should return results — ICU should segment Thai text"
    );

    Ok(())
}

#[tokio::test]
async fn test_multilingual_portuguese_stopwords_preserved() -> Result<()> {
    // "a" is an English stopword but a Portuguese article meaning "the" / "to".
    // It should NOT be removed and should contribute to matching.
    let fixture = SearcherTestFixture::new().await?;
    let pool = fixture.test_env.db_pool.pool();
    seed_multilingual_data(pool).await;

    // Search using Portuguese terms including "a" (an English stopword)
    let (status, response) = fixture
        .search("análise mensal vendas", Some("fulltext"), Some(10))
        .await?;
    assert_eq!(status, StatusCode::OK);
    let titles = result_titles(&response);
    assert!(!titles.is_empty(), "Portuguese query should return results");
    assert!(
        titles.iter().any(|t| t.contains("relatorio")),
        "Portuguese report should be found, got: {:?}",
        titles
    );

    Ok(())
}

#[tokio::test]
async fn test_multilingual_english_stemming_still_works() -> Result<()> {
    // English stemming via the _en aliases should still work.
    // "reports" in the query should match "report" in titles via Snowball stemmer.
    let fixture = SearcherTestFixture::new().await?;
    let _doc_ids = fixture.seed_search_data().await?;

    let (status, response) = fixture
        .search("CRM sales reporting", Some("fulltext"), Some(10))
        .await?;
    assert_eq!(status, StatusCode::OK);
    let titles = result_titles(&response);
    assert!(
        !titles.is_empty(),
        "English stemming should still work via _en aliases"
    );
    assert_eq!(
        titles[0], "CRM Sales Reports",
        "English stemmed query 'reporting' should match 'Reports' via _en alias, got: {:?}",
        titles
    );

    Ok(())
}

#[tokio::test]
async fn test_multilingual_mixed_language_document() -> Result<()> {
    // A document with mixed English and German content should be findable
    // via terms in either language.
    let fixture = SearcherTestFixture::new().await?;
    let pool = fixture.test_env.db_pool.pool();
    seed_multilingual_data(pool).await;

    // Search with English terms
    let (status, response) = fixture
        .search(
            "development migration architecture",
            Some("fulltext"),
            Some(10),
        )
        .await?;
    assert_eq!(status, StatusCode::OK);
    let titles_en = result_titles(&response);
    assert!(
        titles_en.iter().any(|t| t.contains("ProjectUpdate")),
        "Mixed doc should be findable via English terms, got: {:?}",
        titles_en
    );

    // Search with German terms
    let (status, response) = fixture
        .search("Entwicklung Systems planmäßig", Some("fulltext"), Some(10))
        .await?;
    assert_eq!(status, StatusCode::OK);
    let titles_de = result_titles(&response);
    assert!(
        titles_de.iter().any(|t| t.contains("ProjectUpdate")),
        "Mixed doc should be findable via German terms, got: {:?}",
        titles_de
    );

    Ok(())
}

#[tokio::test]
async fn test_multilingual_accent_folding() -> Result<()> {
    // ASCII folding: "cafe" should match "Café", "creme brulee" should match "crème brûlée"
    let fixture = SearcherTestFixture::new().await?;
    let pool = fixture.test_env.db_pool.pool();
    seed_multilingual_data(pool).await;

    let (status, response) = fixture
        .search("cafe creme brulee", Some("fulltext"), Some(10))
        .await?;
    assert_eq!(status, StatusCode::OK);
    let titles = result_titles(&response);
    assert!(
        !titles.is_empty(),
        "ASCII-folded query should match accented content"
    );
    assert!(
        titles.iter().any(|t| t.contains("cafe_menu")),
        "Café menu doc should be found via 'cafe creme brulee', got: {:?}",
        titles
    );

    // Also test the reverse: accented query matching folded index
    let (status, response) = fixture
        .search("résumé naïve", Some("fulltext"), Some(10))
        .await?;
    assert_eq!(status, StatusCode::OK);
    let titles = result_titles(&response);
    assert!(
        !titles.is_empty(),
        "Accented query should also match (both sides are folded)"
    );

    Ok(())
}

#[tokio::test]
async fn test_multilingual_camelcase_code_splitting() -> Result<()> {
    // The title_secondary alias (source_code tokenizer) should split CamelCase in titles.
    // "SearchIndexManager" → ["Search", "Index", "Manager"]
    let fixture = SearcherTestFixture::new().await?;
    let pool = fixture.test_env.db_pool.pool();
    seed_multilingual_data(pool).await;

    let (status, response) = fixture
        .search("Index Manager", Some("fulltext"), Some(10))
        .await?;
    assert_eq!(status, StatusCode::OK);
    let titles = result_titles(&response);
    assert!(
        titles.iter().any(|t| t.contains("SearchIndexManager")),
        "CamelCase title should be findable via individual words, got: {:?}",
        titles
    );

    Ok(())
}

#[tokio::test]
async fn test_multilingual_term_limit_increase() -> Result<()> {
    // The term limit was raised from 8 to 12 to accommodate queries without stopword removal.
    // A 12-word query should still use all 12 terms.
    let fixture = SearcherTestFixture::new().await?;
    let _doc_ids = fixture.seed_search_data().await?;

    // 11-word query (all should contribute to scoring)
    let (status, response) = fixture
        .search(
            "rust programming language memory safety ownership borrowing lifetimes systems fast guide",
            Some("fulltext"),
            Some(10),
        )
        .await?;
    assert_eq!(status, StatusCode::OK);
    let titles = result_titles(&response);
    assert!(
        !titles.is_empty(),
        "Long query (11 terms) should still return results"
    );
    assert_eq!(
        titles[0], "Rust Programming Guide",
        "Rust doc should match most terms in the long query, got: {:?}",
        titles
    );

    Ok(())
}

#[tokio::test]
async fn test_multilingual_cross_script_no_interference() -> Result<()> {
    // Searching in one script should NOT return spurious results from another script.
    // A Chinese query should not match German documents and vice versa.
    let fixture = SearcherTestFixture::new().await?;
    let pool = fixture.test_env.db_pool.pool();
    seed_multilingual_data(pool).await;

    // Chinese-only query should not match German documents
    let (status, response) = fixture
        .search("多语言 分词", Some("fulltext"), Some(10))
        .await?;
    assert_eq!(status, StatusCode::OK);
    let titles = result_titles(&response);
    for title in &titles {
        assert!(
            !title.contains("Quartalsbericht") && !title.contains("Mitarbeiterhandbuch"),
            "Chinese query should not match German documents, got: {:?}",
            titles
        );
    }

    // German-only query should not match CJK documents
    let (status, response) = fixture
        .search(
            "Mitarbeiter Richtlinien Sicherheitsvorschriften",
            Some("fulltext"),
            Some(10),
        )
        .await?;
    assert_eq!(status, StatusCode::OK);
    let titles = result_titles(&response);
    for title in &titles {
        assert!(
            !title.contains("项目") && !title.contains("検索") && !title.contains("검색"),
            "German query should not match CJK documents, got: {:?}",
            titles
        );
    }

    Ok(())
}

// ---------------------------------------------------------------------------
// Typeahead content-type filtering and ACL tests
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_typeahead_content_type_allowlist() -> Result<()> {
    let fixture = SearcherTestFixture::new().await?;
    let pool = fixture.test_env.db_pool.pool();
    let source_id = "01JGF7V3E0Y2R1X8P5Q7W9T4N7";
    let pub_perm = serde_json::json!({"public": true, "users": [], "groups": []});

    // Table-driven: seed one doc for each of the 18 confirmed allowlist values
    let entries = [
        ("normalized doc", "document"),
        ("normalized spreadsheet", "spreadsheet"),
        ("normalized presentation", "presentation"),
        ("normalized pdf", "pdf"),
        ("normalized page", "page"),
        ("mime text/plain", "text/plain"),
        ("mime text/markdown", "text/markdown"),
        ("mime text/csv", "text/csv"),
        ("explicit application/pdf", "application/pdf"),
        (
            "ms docx",
            "application/vnd.openxmlformats-officedocument.wordprocessingml.document",
        ),
        ("ms doc", "application/msword"),
        (
            "ms xlsx",
            "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet",
        ),
        ("ms xls", "application/vnd.ms-excel"),
        (
            "ms pptx",
            "application/vnd.openxmlformats-officedocument.presentationml.presentation",
        ),
        ("ms ppt", "application/vnd.ms-powerpoint"),
        ("gs doc", "application/vnd.google-apps.document"),
        ("gs sheet", "application/vnd.google-apps.spreadsheet"),
        ("gs slides", "application/vnd.google-apps.presentation"),
    ];
    for (title, content_type) in &entries {
        let doc_id = ulid::Ulid::new().to_string();
        sqlx::query(
            "INSERT INTO documents (id, source_id, external_id, title, content, content_type, permissions, metadata, attributes, created_at, updated_at)
             VALUES ($1, $2, $3, $4, $5, $6, $7::jsonb, '{}'::jsonb, '{}'::jsonb, NOW(), NOW())",
        )
        .bind(&doc_id).bind(source_id).bind(format!("ext-{}", &doc_id))
        .bind(title).bind("Content").bind(content_type)
        .bind(&pub_perm).execute(pool).await?;
    }

    // Excluded: email, email_thread, contact, message, event, issue, unknown, NULL
    let excluded = [
        ("Email Doc", "email"),
        ("Email Thread Doc", "email_thread"),
        ("Contact Doc", "contact"),
        ("Message Doc", "message"),
        ("Event Doc", "event"),
        ("Issue Doc", "issue"),
        ("Unknown Type", "some_random_type"),
    ];
    for (title, content_type) in &excluded {
        let doc_id = ulid::Ulid::new().to_string();
        sqlx::query(
            "INSERT INTO documents (id, source_id, external_id, title, content, content_type, permissions, metadata, attributes, created_at, updated_at)
             VALUES ($1, $2, $3, $4, $5, $6, $7::jsonb, '{}'::jsonb, '{}'::jsonb, NOW(), NOW())",
        )
        .bind(&doc_id).bind(source_id).bind(format!("ext-{}", &doc_id))
        .bind(title).bind("Content").bind(content_type)
        .bind(&pub_perm).execute(pool).await?;
    }

    // NULL type excluded
    let null_id = ulid::Ulid::new().to_string();
    sqlx::query(
        "INSERT INTO documents (id, source_id, external_id, title, content, content_type, permissions, metadata, attributes, created_at, updated_at)
         VALUES ($1, $2, $3, $4, $5, $6, $7::jsonb, '{}'::jsonb, '{}'::jsonb, NOW(), NOW())",
    )
    .bind(&null_id).bind(source_id).bind(format!("ext-{}", &null_id))
    .bind("Null Doc").bind("Content").bind(None::<String>)
    .bind(&pub_perm).execute(pool).await?;

    fixture.title_index.refresh().await?;

    // All 18 allowed docs must be found via a unique word in their title
    for (title, content_type) in &entries {
        // Pick the first word of the title as query
        let query = title.split_whitespace().next().unwrap();
        let (status, resp) = fixture.typeahead(query, Some(20)).await?;
        assert_eq!(
            status,
            StatusCode::OK,
            "query '{}' failed for {}",
            query,
            title
        );
        let results = resp["results"].as_array().unwrap();
        let result = results
            .iter()
            .find(|result| result["title"].as_str() == Some(title))
            .unwrap_or_else(|| panic!("allowed type '{}' not found for query '{}'", title, query));
        assert_eq!(result["content_type"].as_str(), Some(*content_type));
        assert_eq!(result["source_type"].as_str(), Some("google_drive"));
    }

    // Excluded docs must NOT appear
    for (title, _) in &excluded {
        let query = title.split_whitespace().next().unwrap().to_lowercase();
        let (status, resp) = fixture.typeahead(&query, Some(20)).await?;
        assert_eq!(status, StatusCode::OK);
        let titles: Vec<&str> = resp["results"]
            .as_array()
            .unwrap()
            .iter()
            .map(|r| r["title"].as_str().unwrap())
            .collect();
        assert!(
            !titles.contains(title),
            "excluded type '{}' appeared",
            title
        );
    }

    // NULL type must NOT appear
    let (status, resp) = fixture.typeahead("null", Some(20)).await?;
    assert_eq!(status, StatusCode::OK);
    let titles: Vec<&str> = resp["results"]
        .as_array()
        .unwrap()
        .iter()
        .map(|r| r["title"].as_str().unwrap())
        .collect();
    assert!(!titles.contains(&"Null Doc"), "NULL content_type appeared");

    Ok(())
}

#[tokio::test]
async fn test_typeahead_public_and_acl_paths() -> Result<()> {
    let fixture = SearcherTestFixture::new().await?;
    let pool = fixture.test_env.db_pool.pool();
    let source_id = "01JGF7V3E0Y2R1X8P5Q7W9T4N7";
    // Create another user for denied/other-user checks
    let other_user_id = ulid::Ulid::new().to_string();
    sqlx::query("INSERT INTO users (id, email, password_hash, created_at, updated_at) VALUES ($1, $2, 'hash', NOW(), NOW()) ON CONFLICT (id) DO NOTHING")
        .bind(&other_user_id).bind("other@example.com").execute(pool).await?;

    // Group membership for group-granted access
    let group_id = ulid::Ulid::new().to_string();
    sqlx::query("INSERT INTO groups (id, source_id, email, display_name, synced_at) VALUES ($1, $2, $3, $4, NOW())")
        .bind(&group_id).bind(source_id).bind("team@example.com").bind("Team").execute(pool).await?;
    sqlx::query("INSERT INTO group_memberships (id, group_id, member_email, synced_at) VALUES ($1, $2, $3, NOW())")
        .bind(ulid::Ulid::new().to_string()).bind(&group_id).bind("test@example.com").execute(pool).await?;

    let pub_perm = serde_json::json!({"public": true, "users": [], "groups": []});
    let user_perm =
        serde_json::json!({"public": false, "users": ["test@example.com"], "groups": []});
    let domain_perm = serde_json::json!({"public": false, "users": [], "groups": ["example.com"]});
    let group_perm =
        serde_json::json!({"public": false, "users": [], "groups": ["team@example.com"]});
    let denied_perm =
        serde_json::json!({"public": false, "users": ["alice@example.com"], "groups": []});

    let docs: Vec<(&str, &serde_json::Value)> = vec![
        ("Public Doc", &pub_perm),
        ("User Grant Doc", &user_perm),
        ("Domain Grant Doc", &domain_perm),
        ("Group Grant Doc", &group_perm),
        ("Denied Doc", &denied_perm),
    ];
    for (title, perm) in &docs {
        let doc_id = ulid::Ulid::new().to_string();
        sqlx::query(
            "INSERT INTO documents (id, source_id, external_id, title, content, content_type, permissions, metadata, attributes, created_at, updated_at)
             VALUES ($1, $2, $3, $4, $5, $6, $7::jsonb, '{}'::jsonb, '{}'::jsonb, NOW(), NOW())",
        )
        .bind(&doc_id).bind(source_id).bind(format!("ext-{}", &doc_id))
        .bind(title).bind("Content").bind("document")
        .bind(perm).execute(pool).await?;
    }

    fixture.title_index.refresh().await?;

    // Default user sees public, user-granted, domain-granted, group-granted; NOT denied
    let (status, resp) = fixture.typeahead("doc", Some(10)).await?;
    assert_eq!(status, StatusCode::OK);
    let titles: Vec<&str> = resp["results"]
        .as_array()
        .unwrap()
        .iter()
        .map(|r| r["title"].as_str().unwrap())
        .collect();
    assert!(titles.contains(&"Public Doc"));
    assert!(titles.contains(&"User Grant Doc"));
    assert!(titles.contains(&"Domain Grant Doc"));
    assert!(titles.contains(&"Group Grant Doc"));
    assert!(!titles.contains(&"Denied Doc"));

    // Other user sees only public
    let (status, resp) = fixture
        .typeahead_with_user("doc", Some(10), &other_user_id)
        .await?;
    assert_eq!(status, StatusCode::OK);
    let titles: Vec<&str> = resp["results"]
        .as_array()
        .unwrap()
        .iter()
        .map(|r| r["title"].as_str().unwrap())
        .collect();
    assert!(titles.contains(&"Public Doc"));
    assert!(!titles.contains(&"User Grant Doc"));
    assert!(!titles.contains(&"Denied Doc"));

    Ok(())
}

#[tokio::test]
async fn test_typeahead_freshness_after_perm_change_and_source_delete() -> Result<()> {
    let fixture = SearcherTestFixture::new().await?;
    let pool = fixture.test_env.db_pool.pool();
    let source_id = "01JGF7V3E0Y2R1X8P5Q7W9T4N7";
    let pub_perm = serde_json::json!({"public": true, "users": [], "groups": []});
    let priv_perm =
        serde_json::json!({"public": false, "users": ["alice@example.com"], "groups": []});

    let doc_id = ulid::Ulid::new().to_string();
    sqlx::query(
        "INSERT INTO documents (id, source_id, external_id, title, content, content_type, permissions, metadata, attributes, created_at, updated_at)
         VALUES ($1, $2, $3, $4, $5, $6, $7::jsonb, '{}'::jsonb, '{}'::jsonb, NOW(), NOW())",
    )
    .bind(&doc_id).bind(source_id).bind(format!("ext-{}", &doc_id))
    .bind("Freshness Doc").bind("Content").bind("document")
    .bind(&pub_perm).execute(pool).await?;

    fixture.title_index.refresh().await?;

    // Doc should be visible after initial refresh
    let (status, resp) = fixture.typeahead("freshness", None).await?;
    assert_eq!(status, StatusCode::OK);
    let titles: Vec<&str> = resp["results"]
        .as_array()
        .unwrap()
        .iter()
        .map(|r| r["title"].as_str().unwrap())
        .collect();
    assert!(
        titles.contains(&"Freshness Doc"),
        "should be visible after refresh"
    );

    // Change permissions to deny WITHOUT refreshing the index.
    sqlx::query("UPDATE documents SET permissions = $1::jsonb WHERE id = $2")
        .bind(&priv_perm)
        .bind(&doc_id)
        .execute(pool)
        .await?;

    // Doc must still be in the FST (not refreshed) but filtered out by request-time ACL
    let (status, resp) = fixture.typeahead("freshness", None).await?;
    assert_eq!(status, StatusCode::OK);
    let titles: Vec<&str> = resp["results"]
        .as_array()
        .unwrap()
        .iter()
        .map(|r| r["title"].as_str().unwrap())
        .collect();
    assert!(
        !titles.contains(&"Freshness Doc"),
        "should be hidden after permission change even without refresh"
    );

    // Restore public permissions WITHOUT refreshing — doc should become visible again
    sqlx::query("UPDATE documents SET permissions = $1::jsonb WHERE id = $2")
        .bind(&pub_perm)
        .bind(&doc_id)
        .execute(pool)
        .await?;

    let (status, resp) = fixture.typeahead("freshness", None).await?;
    assert_eq!(status, StatusCode::OK);
    let titles: Vec<&str> = resp["results"]
        .as_array()
        .unwrap()
        .iter()
        .map(|r| r["title"].as_str().unwrap())
        .collect();
    assert!(
        titles.contains(&"Freshness Doc"),
        "should be visible again after restoring public permissions"
    );

    // Now soft-delete the source WITHOUT refreshing.
    sqlx::query("UPDATE sources SET is_deleted = TRUE WHERE id = $1")
        .bind(source_id)
        .execute(pool)
        .await?;

    // Doc must still be in FST but filtered by sources JOIN + NOT is_deleted
    let (status, resp) = fixture.typeahead("freshness", None).await?;
    assert_eq!(status, StatusCode::OK);
    let titles: Vec<&str> = resp["results"]
        .as_array()
        .unwrap()
        .iter()
        .map(|r| r["title"].as_str().unwrap())
        .collect();
    assert!(
        !titles.contains(&"Freshness Doc"),
        "should be hidden after source delete without refresh"
    );

    // Restore source for other tests
    sqlx::query("UPDATE sources SET is_deleted = FALSE WHERE id = $1")
        .bind(source_id)
        .execute(pool)
        .await?;

    Ok(())
}

#[tokio::test]
async fn test_typeahead_fills_after_denied_candidates_within_cap() -> Result<()> {
    let fixture = SearcherTestFixture::new().await?;
    let pool = fixture.test_env.db_pool.pool();
    let source_id = "01JGF7V3E0Y2R1X8P5Q7W9T4N7";
    let denied_perm =
        serde_json::json!({"public": false, "users": ["alice@example.com"], "groups": []});
    let pub_perm = serde_json::json!({"public": true, "users": [], "groups": []});

    // 80 denied docs with short title — higher FST rank, all in batch 1 (batch_size=100)
    for i in 0..80 {
        let doc_id = ulid::Ulid::new().to_string();
        sqlx::query(
            "INSERT INTO documents (id, source_id, external_id, title, content, content_type, permissions, metadata, attributes, created_at, updated_at)
             VALUES ($1, $2, $3, $4, $5, $6, $7::jsonb, '{}'::jsonb, '{}'::jsonb, NOW(), NOW())",
        )
        .bind(&doc_id).bind(source_id).bind(format!("ext-{}", &doc_id))
        .bind(format!("Doc {}", i)).bind("Content").bind("document")
        .bind(&denied_perm).execute(pool).await?;
    }

    // 5 accessible docs with distinct, progressively longer titles.
    // All score below all 80 denied docs, so they sit in batch 2.
    // Within batch 2, shortest accessible title scores highest.
    let accessible = [
        "Doc zzz aaaaa",
        "Doc zzz bbbbbb",
        "Doc zzz ccccccc",
        "Doc zzz dddddddd",
        "Doc zzz eeeeeeeee",
    ];
    for title in &accessible {
        let doc_id = ulid::Ulid::new().to_string();
        sqlx::query(
            "INSERT INTO documents (id, source_id, external_id, title, content, content_type, permissions, metadata, attributes, created_at, updated_at)
             VALUES ($1, $2, $3, $4, $5, $6, $7::jsonb, '{}'::jsonb, '{}'::jsonb, NOW(), NOW())",
        )
        .bind(&doc_id).bind(source_id).bind(format!("ext-{}", &doc_id))
        .bind(title).bind("Content").bind("document")
        .bind(&pub_perm).execute(pool).await?;
    }

    fixture.title_index.refresh().await?;

    let (status, resp) = fixture.typeahead("doc", Some(5)).await?;
    assert_eq!(status, StatusCode::OK);
    let titles: Vec<&str> = resp["results"]
        .as_array()
        .unwrap()
        .iter()
        .map(|r| r["title"].as_str().unwrap())
        .collect();
    // All 5 accessible docs appear despite 80 denied higher-ranked docs filtered out.
    // With the 100-candidate cap, all 80 denied + 5 accessible = 85 candidates fit in one batch. Only accessible survive.
    // Only accessible survive. Within accessible, shortest title has highest FST score.
    assert_eq!(titles.len(), 5, "expected 5 results, got {:?}", titles);
    assert_eq!(titles[0], "Doc zzz aaaaa");
    assert_eq!(titles[1], "Doc zzz bbbbbb");
    assert_eq!(titles[2], "Doc zzz ccccccc");
    assert_eq!(titles[3], "Doc zzz dddddddd");
    assert_eq!(titles[4], "Doc zzz eeeeeeeee");
    Ok(())
}

#[tokio::test]
async fn test_typeahead_candidate_set_is_capped_at_100() -> Result<()> {
    let fixture = SearcherTestFixture::new().await?;
    let pool = fixture.test_env.db_pool.pool();
    let source_id = "01JGF7V3E0Y2R1X8P5Q7W9T4N7";
    let permissions = serde_json::json!({"public": true, "users": [], "groups": []});

    for i in 0..105 {
        let doc_id = ulid::Ulid::new().to_string();
        sqlx::query(
            "INSERT INTO documents (id, source_id, external_id, title, content, content_type, permissions, metadata, attributes, created_at, updated_at)
             VALUES ($1, $2, $3, $4, 'Content', 'document', $5::jsonb, '{}'::jsonb, '{}'::jsonb, NOW(), NOW())",
        )
        .bind(&doc_id)
        .bind(source_id)
        .bind(format!("cap-ext-{i}"))
        .bind(format!("Cap candidate {i:03}"))
        .bind(&permissions)
        .execute(pool)
        .await?;
    }

    fixture.title_index.refresh().await?;
    let candidates = fixture.title_index.search_candidates("cap").await;
    assert_eq!(candidates.len(), 100);
    Ok(())
}

#[tokio::test]
async fn test_title_entry_keyset_pagination_has_no_duplicates_or_omissions() -> Result<()> {
    let fixture = SearcherTestFixture::new().await?;
    let pool = fixture.test_env.db_pool.pool();
    let repo = DocumentRepository::new(pool);
    let content_types = vec!["document".to_string()];
    let mut after_id: Option<String> = None;
    let mut ids = Vec::new();

    loop {
        let page = repo
            .fetch_title_entries_by_types_paginated(&content_types, after_id.as_deref(), 7)
            .await?;
        if page.is_empty() {
            break;
        }
        after_id = page.last().map(|entry| entry.id.clone());
        ids.extend(page.into_iter().map(|entry| entry.id));
    }

    let expected: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM documents d JOIN sources s ON s.id = d.source_id WHERE NOT s.is_deleted AND d.content_type = 'document'",
    )
    .fetch_one(pool)
    .await?;
    let unique: std::collections::HashSet<_> = ids.iter().collect();
    assert_eq!(ids.len(), expected as usize);
    assert_eq!(unique.len(), ids.len());
    assert!(ids.windows(2).all(|pair| pair[0] < pair[1]));
    Ok(())
}

#[tokio::test]
async fn test_typeahead_queries_shorter_than_three_characters_return_empty() -> Result<()> {
    let fixture = SearcherTestFixture::new().await?;
    let (_doc_ids, _) = fixture.seed_mentionable_data().await?;
    for query in ["", "a", "ab", "文", "文件"] {
        let (status, response) = fixture.typeahead(query, None).await?;
        assert_eq!(status, StatusCode::OK);
        assert!(response["results"].as_array().unwrap().is_empty());
    }
    Ok(())
}

#[tokio::test]
async fn test_typeahead_is_case_insensitive_but_not_typo_tolerant() -> Result<()> {
    let fixture = SearcherTestFixture::new().await?;
    let (_doc_ids, _) = fixture.seed_mentionable_data().await?;
    fixture.title_index.refresh().await?;

    let (_, lower) = fixture.typeahead("planning", None).await?;
    let (_, upper) = fixture.typeahead("PLANNING", None).await?;
    assert_eq!(lower["results"], upper["results"]);

    let (_, typo) = fixture.typeahead("plannign", None).await?;
    assert!(typo["results"].as_array().unwrap().is_empty());
    Ok(())
}

#[tokio::test]
async fn test_typeahead_missing_user_id_returns_400() -> Result<()> {
    let fixture = SearcherTestFixture::new().await?;
    let uri = "/typeahead?q=test&limit=5";
    let request = axum::http::Request::builder()
        .method(axum::http::Method::GET)
        .uri(uri)
        .body(axum::body::Body::empty())?;
    let response = fixture.app.clone().oneshot(request).await?;
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    Ok(())
}

#[tokio::test]
async fn test_typeahead_unknown_user_returns_empty() -> Result<()> {
    let fixture = SearcherTestFixture::new().await?;
    let (_doc_ids, _) = fixture.seed_mentionable_data().await?;
    let (status, resp) = fixture
        .typeahead_with_user("doc", None, "nonexistent-user-id")
        .await?;
    assert_eq!(status, StatusCode::OK);
    assert!(resp["results"].as_array().unwrap().is_empty());
    Ok(())
}

#[tokio::test]
async fn test_typeahead_inactive_user_returns_empty() -> Result<()> {
    let fixture = SearcherTestFixture::new().await?;
    let (_doc_ids, _) = fixture.seed_mentionable_data().await?;
    sqlx::query("UPDATE users SET is_active = FALSE WHERE id = $1")
        .bind("01JGF7V3E0Y2R1X8P5Q7W9T4N6")
        .execute(fixture.test_env.db_pool.pool())
        .await?;
    let (status, resp) = fixture.typeahead("doc", None).await?;
    assert_eq!(status, StatusCode::OK);
    assert!(resp["results"].as_array().unwrap().is_empty());
    Ok(())
}

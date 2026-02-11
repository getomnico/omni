use anyhow::Result;
use axum::{
    body::Body,
    http::{Method, Request, StatusCode},
    Router,
};
use omni_searcher::{
    create_app, suggested_questions::SuggestedQuestionsGenerator, typeahead::TitleIndex, AppState,
};
use serde_json::{json, Value};
use shared::storage::postgres::PostgresStorage;
use shared::test_environment::TestEnvironment;
use shared::test_utils::create_test_documents_with_embeddings;
use shared::{AIClient, ObjectStorage, SearcherConfig};
use std::sync::Arc;
use tower::ServiceExt;

/// Test fixture for searcher service integration tests
pub struct SearcherTestFixture {
    pub test_env: TestEnvironment,
    pub app: Router,
    pub title_index: Arc<TitleIndex>,
}

impl SearcherTestFixture {
    pub async fn new() -> Result<Self> {
        let test_env = TestEnvironment::new().await?;

        // Create test AI client and config
        let ai_client = AIClient::new(test_env.mock_ai_server.base_url.clone());
        let config = SearcherConfig {
            port: 8002,
            database: test_env.database_config(),
            redis: test_env.redis_config(),
            ai_service_url: test_env.mock_ai_server.base_url.clone(),
            hybrid_search_fts_weight: 0.6,
            hybrid_search_semantic_weight: 0.4,
            semantic_search_timeout_ms: 5000,
            rag_context_window: 2,
        };

        // Create content storage using PostgresStorage directly
        let content_storage: Arc<dyn ObjectStorage> =
            Arc::new(PostgresStorage::new(test_env.db_pool.pool().clone()));

        // Create suggested questions generator
        let suggested_questions_generator = Arc::new(SuggestedQuestionsGenerator::new(
            test_env.redis_client.clone(),
            test_env.db_pool.clone(),
            content_storage.clone(),
            ai_client.clone(),
        ));

        let title_index = Arc::new(TitleIndex::new(test_env.db_pool.clone()));

        let app_state = AppState {
            db_pool: test_env.db_pool.clone(),
            redis_client: test_env.redis_client.clone(),
            ai_client,
            config,
            content_storage,
            suggested_questions_generator,
            title_index: title_index.clone(),
        };

        let app = create_app(app_state);

        Ok(Self {
            test_env,
            app,
            title_index,
        })
    }

    /// Populate the database with test data including embeddings
    pub async fn seed_search_data(&self) -> Result<Vec<String>> {
        let ids = create_test_documents_with_embeddings(self.test_env.db_pool.pool()).await?;
        self.title_index.refresh().await?;
        Ok(ids)
    }

    /// Helper method to make search requests
    pub async fn search(
        &self,
        query: &str,
        mode: Option<&str>,
        limit: Option<u32>,
    ) -> Result<(StatusCode, Value)> {
        self.search_with_user(query, mode, limit, None).await
    }

    /// Helper method to make search requests with user_email for permission filtering
    pub async fn search_with_user(
        &self,
        query: &str,
        mode: Option<&str>,
        limit: Option<u32>,
        user_email: Option<&str>,
    ) -> Result<(StatusCode, Value)> {
        let mut search_body = json!({
            "query": query
        });

        if let Some(mode) = mode {
            search_body["mode"] = json!(mode);
        }

        if let Some(limit) = limit {
            search_body["limit"] = json!(limit);
        }

        if let Some(email) = user_email {
            search_body["user_email"] = json!(email);
        }

        let request = Request::builder()
            .method(Method::POST)
            .uri("/search")
            .header("content-type", "application/json")
            .body(Body::from(search_body.to_string()))?;

        let response = self.app.clone().oneshot(request).await?;
        let status = response.status();
        let body = axum::body::to_bytes(response.into_body(), usize::MAX).await?;
        let body_str = String::from_utf8_lossy(&body);

        let json: Value = serde_json::from_slice(&body).map_err(|e| {
            eprintln!(
                "Failed to parse JSON response. Status: {}, Body: '{}'",
                status, body_str
            );
            e
        })?;

        Ok((status, json))
    }

    /// Helper method to make search requests with a raw JSON body
    pub async fn search_with_body(&self, body: Value) -> Result<(StatusCode, Value)> {
        let request = Request::builder()
            .method(Method::POST)
            .uri("/search")
            .header("content-type", "application/json")
            .body(Body::from(body.to_string()))?;

        let response = self.app.clone().oneshot(request).await?;
        let status = response.status();
        let body = axum::body::to_bytes(response.into_body(), usize::MAX).await?;
        let body_str = String::from_utf8_lossy(&body);

        let json: Value = serde_json::from_slice(&body).map_err(|e| {
            eprintln!(
                "Failed to parse JSON response. Status: {}, Body: '{}'",
                status, body_str
            );
            e
        })?;

        Ok((status, json))
    }

    /// Helper method to make suggestions requests
    pub async fn suggestions(&self, query: &str) -> Result<(StatusCode, Value)> {
        let request = Request::builder()
            .method(Method::GET)
            .uri(&format!("/suggestions?q={}", urlencoding::encode(query)))
            .body(Body::empty())?;

        let response = self.app.clone().oneshot(request).await?;
        let status = response.status();
        let body = axum::body::to_bytes(response.into_body(), usize::MAX).await?;
        let body_str = String::from_utf8_lossy(&body);

        let json: Value = serde_json::from_slice(&body).map_err(|e| {
            eprintln!(
                "Failed to parse JSON response. Status: {}, Body: '{}'",
                status, body_str
            );
            e
        })?;

        Ok((status, json))
    }

    /// Helper method to make typeahead requests
    pub async fn typeahead(
        &self,
        query: &str,
        limit: Option<usize>,
    ) -> Result<(StatusCode, Value)> {
        let uri = if let Some(limit) = limit {
            format!(
                "/typeahead?q={}&limit={}",
                urlencoding::encode(query),
                limit
            )
        } else {
            format!("/typeahead?q={}", urlencoding::encode(query))
        };

        let request = Request::builder()
            .method(Method::GET)
            .uri(&uri)
            .body(Body::empty())?;

        let response = self.app.clone().oneshot(request).await?;
        let status = response.status();
        let body = axum::body::to_bytes(response.into_body(), usize::MAX).await?;
        let body_str = String::from_utf8_lossy(&body);

        let json: Value = serde_json::from_slice(&body).map_err(|e| {
            eprintln!(
                "Failed to parse JSON response. Status: {}, Body: '{}'",
                status, body_str
            );
            e
        })?;

        Ok((status, json))
    }
}

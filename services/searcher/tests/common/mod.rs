use anyhow::Result;
use axum::{
    body::Body,
    http::{Method, Request, StatusCode},
    Router,
};
use omni_searcher::{
    create_app, operator_registry::OperatorRegistry,
    suggested_questions::SuggestedQuestionsGenerator, typeahead::TitleIndex, AppState,
};
use serde_json::{json, Value};
use shared::storage::postgres::PostgresStorage;
use shared::test_environment::TestEnvironment;
use shared::test_utils::create_test_documents_with_embeddings;
use shared::{models::DocumentPermissions, AIClient, ObjectStorage, SearcherConfig};
use std::sync::Arc;
use tower::ServiceExt;
use ulid::Ulid;

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
            rrf_k: 60.0,
            semantic_search_timeout_ms: 5000,
            rag_context_window: 2,
            recency_boost_weight: 0.2,
            recency_half_life_days: 30.0,
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
            operator_registry: Arc::new(OperatorRegistry::new(test_env.redis_client.clone())),
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

    /// Seed a set of documents with mentionable content types for typeahead ACL tests.
    /// Also creates a second user for permission-denied scenarios.
    /// Returns (all_doc_ids, other_user_id).
    pub async fn seed_mentionable_data(&self) -> Result<(Vec<String>, String)> {
        let pool = self.test_env.db_pool.pool();
        let other_user_id = Ulid::new().to_string();
        let source_id = "01JGF7V3E0Y2R1X8P5Q7W9T4N7";

        // Create a second user
        sqlx::query(
            r#"INSERT INTO users (id, email, password_hash, created_at, updated_at)
               VALUES ($1, $2, 'hash', NOW(), NOW())
               ON CONFLICT (id) DO NOTHING"#,
        )
        .bind(&other_user_id)
        .bind("other@example.com")
        .execute(pool)
        .await?;

        let mut doc_ids = Vec::new();

        async fn insert_doc(
            pool: &sqlx::PgPool,
            source_id: &str,
            title: &str,
            content_type: &str,
            content: &str,
            permissions: &DocumentPermissions,
        ) -> anyhow::Result<String> {
            let doc_id = Ulid::new().to_string();
            let perms_json = serde_json::to_value(permissions).unwrap();
            sqlx::query(
                r#"INSERT INTO documents
                   (id, source_id, external_id, title, content, content_type, permissions, metadata, attributes, created_at, updated_at)
                   VALUES ($1, $2, $3, $4, $5, $6, $7::jsonb, '{}'::jsonb, '{}'::jsonb, NOW(), NOW())"#,
            )
            .bind(&doc_id)
            .bind(source_id)
            .bind(format!("ext-{}", &doc_id))
            .bind(title)
            .bind(content)
            .bind(content_type)
            .bind(&perms_json)
            .execute(pool)
            .await?;
            Ok(doc_id)
        }

        let pub_perm = DocumentPermissions {
            public: true,
            users: vec![],
            groups: vec![],
        };
        let user_perm = DocumentPermissions {
            public: false,
            users: vec!["test@example.com".to_string()],
            groups: vec![],
        };
        let group_perm = DocumentPermissions {
            public: false,
            users: vec![],
            groups: vec!["team@example.com".to_string()],
        };
        let denied_perm = DocumentPermissions {
            public: false,
            users: vec!["alice@example.com".to_string()],
            groups: vec![],
        };

        // Allowed types — should appear in FST
        doc_ids.push(
            insert_doc(
                pool,
                source_id,
                "Q4 Planning Meeting",
                "document",
                "Planning content",
                &pub_perm,
            )
            .await?,
        );
        doc_ids.push(
            insert_doc(
                pool,
                source_id,
                "Budget Spreadsheet",
                "spreadsheet",
                "Budget numbers",
                &pub_perm,
            )
            .await?,
        );
        doc_ids.push(
            insert_doc(
                pool,
                source_id,
                "Sales Deck",
                "presentation",
                "Sales slides",
                &pub_perm,
            )
            .await?,
        );
        doc_ids.push(
            insert_doc(
                pool,
                source_id,
                "Annual Report PDF",
                "pdf",
                "PDF content",
                &pub_perm,
            )
            .await?,
        );
        doc_ids.push(
            insert_doc(
                pool,
                source_id,
                "Confluence Page",
                "page",
                "Page content",
                &pub_perm,
            )
            .await?,
        );
        doc_ids.push(
            insert_doc(
                pool,
                source_id,
                "Notes Text",
                "text/plain",
                "Plain text",
                &pub_perm,
            )
            .await?,
        );
        doc_ids.push(
            insert_doc(
                pool,
                source_id,
                "REST API Endpoints",
                "document",
                "API docs",
                &pub_perm,
            )
            .await?,
        );
        doc_ids.push(
            insert_doc(
                pool,
                source_id,
                "Getting Started Guide",
                "document",
                "Guide content",
                &pub_perm,
            )
            .await?,
        );
        doc_ids.push(
            insert_doc(
                pool,
                source_id,
                "Rust Programming Guide",
                "document",
                "Rust content",
                &pub_perm,
            )
            .await?,
        );
        // User/grant permission variations (non-public)
        doc_ids.push(
            insert_doc(
                pool,
                source_id,
                "User Document",
                "document",
                "User-only content",
                &user_perm,
            )
            .await?,
        );
        doc_ids.push(
            insert_doc(
                pool,
                source_id,
                "Group Document",
                "document",
                "Group-only content",
                &group_perm,
            )
            .await?,
        );
        doc_ids.push(
            insert_doc(
                pool,
                source_id,
                "Denied Document",
                "document",
                "Denied content",
                &denied_perm,
            )
            .await?,
        );
        // Excluded types — should NOT appear
        doc_ids.push(
            insert_doc(
                pool,
                source_id,
                "Email Thread",
                "email_thread",
                "Email content",
                &pub_perm,
            )
            .await?,
        );
        doc_ids.push(
            insert_doc(
                pool,
                source_id,
                "Contact Record",
                "contact",
                "Contact info",
                &pub_perm,
            )
            .await?,
        );
        doc_ids.push(
            insert_doc(
                pool,
                source_id,
                "Chat Message",
                "message",
                "Chat content",
                &pub_perm,
            )
            .await?,
        );
        doc_ids.push(
            insert_doc(
                pool,
                source_id,
                "Calendar Event",
                "event",
                "Event details",
                &pub_perm,
            )
            .await?,
        );
        doc_ids.push(
            insert_doc(
                pool,
                source_id,
                "Jira Issue",
                "issue",
                "Issue content",
                &pub_perm,
            )
            .await?,
        );
        doc_ids.push(
            insert_doc(
                pool,
                source_id,
                "NULL Type Doc",
                "email",
                "Should be excluded",
                &pub_perm,
            )
            .await?,
        );

        self.title_index.refresh().await?;

        Ok((doc_ids, other_user_id))
    }

    /// Helper method to make typeahead requests with the default test user.
    pub async fn typeahead(
        &self,
        query: &str,
        limit: Option<usize>,
    ) -> Result<(StatusCode, Value)> {
        self.typeahead_with_user(query, limit, "01JGF7V3E0Y2R1X8P5Q7W9T4N6")
            .await
    }

    /// Helper method to make typeahead requests with a specific user_id.
    pub async fn typeahead_with_user(
        &self,
        query: &str,
        limit: Option<usize>,
        user_id: &str,
    ) -> Result<(StatusCode, Value)> {
        let uri = if let Some(limit) = limit {
            format!(
                "/typeahead?q={}&limit={}&user_id={}",
                urlencoding::encode(query),
                limit,
                urlencoding::encode(user_id),
            )
        } else {
            format!(
                "/typeahead?q={}&user_id={}",
                urlencoding::encode(query),
                urlencoding::encode(user_id),
            )
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

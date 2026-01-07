use anyhow::Result;
use redis::Client as RedisClient;
use sqlx::PgPool;
use testcontainers::{
    core::{ContainerPort, WaitFor},
    runners::AsyncRunner,
    ContainerAsync, GenericImage, ImageExt,
};
use testcontainers_modules::redis::Redis;
use tokio::time::{sleep, Duration};

use crate::{
    config::{DatabaseConfig, RedisConfig},
    db::pool::DatabasePool,
};

/// Test environment that manages all external dependencies via testcontainers
pub struct TestEnvironment {
    pub db_pool: DatabasePool,
    pub redis_client: RedisClient,
    pub mock_ai_server: MockAIServer,
    redis_port: u16,
    _postgres_container: ContainerAsync<GenericImage>,
    _redis_container: ContainerAsync<Redis>,
}

impl TestEnvironment {
    /// Create a new test environment with all dependencies
    pub async fn new() -> Result<Self> {
        tracing_subscriber::fmt::try_init().ok();

        // Start PostgreSQL with pgvector and pg_bm25 extensions (ParadeDB image)
        let postgres_image = GenericImage::new("paradedb/paradedb", "0.20.6-pg17")
            .with_wait_for(WaitFor::message_on_stderr(
                "database system is ready to accept connections",
            ))
            .with_exposed_port(ContainerPort::Tcp(5432))
            .with_env_var("POSTGRES_DB", "omni_test")
            .with_env_var("POSTGRES_USER", "omni")
            .with_env_var("POSTGRES_PASSWORD", "omni_password");
        let postgres_container = postgres_image.start().await?;
        let postgres_port = postgres_container
            .get_host_port_ipv4(ContainerPort::Tcp(5432))
            .await?;

        // Start Redis
        let redis_container = Redis::default().start().await?;
        let redis_port = redis_container
            .get_host_port_ipv4(ContainerPort::Tcp(6379))
            .await?;

        // Create database connection
        let database_url = format!(
            "postgresql://omni:omni_password@localhost:{}/omni_test",
            postgres_port
        );
        let db_pool = DatabasePool::new(&database_url).await?;

        // Run migrations
        let mut current_dir = std::env::current_dir()?;
        loop {
            let migration_dir = current_dir.join("services/migrations");
            if migration_dir.exists() {
                let migrator = sqlx::migrate::Migrator::new(migration_dir).await?;
                migrator.run(db_pool.pool()).await?;
                break;
            }
            if !current_dir.pop() {
                return Err(anyhow::anyhow!(
                    "Could not find migrations directory services/migrations"
                ));
            }
        }

        // Seed test data
        Self::seed_database(db_pool.pool()).await?;

        // Create Redis connection
        let redis_url = format!("redis://localhost:{}", redis_port);
        let redis_client = RedisClient::open(redis_url)?;

        // Clear Redis database
        let mut conn = redis_client.get_multiplexed_async_connection().await?;
        redis::cmd("FLUSHDB")
            .query_async::<String>(&mut conn)
            .await?;

        // Start mock AI server
        let mock_ai_server = MockAIServer::start().await?;

        Ok(Self {
            db_pool,
            redis_client,
            mock_ai_server,
            redis_port,
            _postgres_container: postgres_container,
            _redis_container: redis_container,
        })
    }

    /// Get database configuration for services
    pub fn database_config(&self) -> DatabaseConfig {
        DatabaseConfig {
            database_url: self.db_pool.database_url().to_string(),
            max_connections: 5,
            acquire_timeout_seconds: 30,
            require_ssl: false,
        }
    }

    /// Get Redis configuration for services
    pub fn redis_config(&self) -> RedisConfig {
        RedisConfig {
            redis_url: format!("redis://localhost:{}", self.redis_port),
        }
    }

    /// Seed the test database with minimal required data
    async fn seed_database(pool: &PgPool) -> Result<()> {
        let user_id = "01JGF7V3E0Y2R1X8P5Q7W9T4N6";
        let source_id = "01JGF7V3E0Y2R1X8P5Q7W9T4N7";

        sqlx::query(
            r#"
            INSERT INTO users (id, email, password_hash, created_at, updated_at)
            VALUES ($1, 'test@example.com', 'hash', NOW(), NOW())
            ON CONFLICT (id) DO NOTHING
            "#,
        )
        .bind(user_id)
        .execute(pool)
        .await?;

        sqlx::query(
            r#"
            INSERT INTO sources (id, name, source_type, config, created_by, created_at, updated_at)
            VALUES ($1, 'Test Source', 'local_files', '{}', $2, NOW(), NOW())
            ON CONFLICT (id) DO NOTHING
            "#,
        )
        .bind(source_id)
        .bind(user_id)
        .execute(pool)
        .await?;

        Ok(())
    }
}

/// Mock AI server for testing
pub struct MockAIServer {
    pub base_url: String,
    _server_handle: tokio::task::JoinHandle<()>,
}

impl MockAIServer {
    /// Start the mock AI server
    pub async fn start() -> Result<Self> {
        use axum::{
            response::Json,
            routing::{get, post},
            Router,
        };
        use serde::{Deserialize, Serialize};
        use tokio::net::TcpListener;

        #[derive(Deserialize)]
        struct EmbeddingRequest {
            texts: Vec<String>,
            task: Option<String>,
            chunk_size: Option<i32>,
            chunking_mode: Option<String>,
        }

        #[derive(Serialize)]
        struct EmbeddingResponse {
            embeddings: Vec<Vec<Vec<f32>>>, // embeddings per text per chunk
            chunks_count: Vec<i32>,         // number of chunks per text
            chunks: Vec<Vec<(i32, i32)>>,   // character offset spans for each chunk
            model_name: String,             // name of the model used for embeddings
        }

        #[derive(Deserialize)]
        struct RagRequest {
            query: String,
            context: Vec<String>,
        }

        #[derive(Serialize)]
        struct RagResponse {
            answer: String,
        }

        #[derive(Deserialize)]
        struct GenerateRequest {
            prompt: String,
        }

        #[derive(Serialize)]
        struct GenerateResponse {
            response: String,
        }

        // Mock embeddings endpoint - returns a fixed 1024-dim vector
        async fn mock_embeddings(Json(req): Json<EmbeddingRequest>) -> Json<EmbeddingResponse> {
            let mut embeddings = Vec::new();
            let mut chunks_count = Vec::new();
            let mut chunks = Vec::new();

            for text in &req.texts {
                // Generate a deterministic embedding based on text hash
                let mut embedding = vec![0.0; 1024];
                let hash = text.len() as f32;
                for i in 0..1024 {
                    embedding[i] = (hash + i as f32) / 1024.0;
                }

                // For simplicity, treat each text as a single chunk
                embeddings.push(vec![embedding]);
                chunks_count.push(1);
                chunks.push(vec![(0, text.len() as i32)]);
            }

            Json(EmbeddingResponse {
                embeddings,
                chunks_count,
                chunks,
                model_name: "test-model".to_string(),
            })
        }

        // Mock RAG endpoint
        async fn mock_rag(Json(req): Json<RagRequest>) -> Json<RagResponse> {
            let answer = format!(
                "Based on the context about '{}', here is the answer: {}",
                req.query,
                req.context.join(" ")
            );
            Json(RagResponse { answer })
        }

        // Mock generate endpoint
        async fn mock_generate(Json(req): Json<GenerateRequest>) -> Json<GenerateResponse> {
            let response = format!("Mock AI response for: {}", req.prompt);
            Json(GenerateResponse { response })
        }

        // Health check endpoint
        async fn health() -> (axum::http::StatusCode, &'static str) {
            (axum::http::StatusCode::OK, "OK")
        }

        let app = Router::new()
            .route("/embeddings", post(mock_embeddings))
            .route("/rag", post(mock_rag))
            .route("/generate", post(mock_generate))
            .route("/health", get(health));

        // Find available port
        let listener = TcpListener::bind("127.0.0.1:0").await?;
        let addr = listener.local_addr()?;
        let port = addr.port();

        let server_handle = tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });

        // Wait for server to start
        sleep(Duration::from_millis(100)).await;

        Ok(Self {
            base_url: format!("http://127.0.0.1:{}", port),
            _server_handle: server_handle,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_environment_setup() {
        let env = TestEnvironment::new().await.unwrap();

        // Test database connection
        let result = sqlx::query("SELECT 1 as test")
            .fetch_one(env.db_pool.pool())
            .await;
        assert!(result.is_ok());

        // Test Redis connection
        let mut conn = env
            .redis_client
            .get_multiplexed_async_connection()
            .await
            .unwrap();
        let result: String = redis::cmd("PING").query_async(&mut conn).await.unwrap();
        assert_eq!(result, "PONG");

        // Test mock AI server
        let client = reqwest::Client::new();
        let response = client
            .get(&format!("{}/health", env.mock_ai_server.base_url))
            .send()
            .await
            .unwrap();
        assert!(response.status().is_success());
    }
}

use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use std::net::IpAddr;
use std::process::Command;

use anyhow::Result;
use redis::Client as RedisClient;
use sqlx::PgPool;
use testcontainers::{
    ContainerAsync, GenericImage, ImageExt,
    core::{ContainerPort, WaitFor},
    runners::AsyncRunner,
};
use testcontainers_modules::{localstack::LocalStack, redis::Redis};
use tokio::time::{Duration, sleep};

use crate::{
    config::{DatabaseConfig, RedisConfig},
    db::pool::DatabasePool,
};

/// Test environment that manages all external dependencies via testcontainers
pub struct TestEnvironment {
    pub db_pool: DatabasePool,
    pub redis_client: RedisClient,
    pub mock_ai_server: MockAIServer,
    pub s3_endpoint: String,
    #[allow(dead_code)]
    redis_url: String,
    _postgres_container: ContainerAsync<GenericImage>,
    _redis_container: ContainerAsync<Redis>,
    _localstack_container: ContainerAsync<LocalStack>,
}

fn is_direct_bridge_mode() -> bool {
    std::env::var("TESTCONTAINERS_DIRECT_BRIDGE")
        .map(|v| v == "true" || v == "1")
        .unwrap_or(false)
}

/// The migrator runs as a separate container, so it can never reach Postgres
/// through localhost. In default mapped-port mode it must reach the Docker
/// host via the host gateway; in direct bridge mode it can use the Postgres
/// container's bridge IP directly.
fn migrator_postgres_address_inner(
    pg_ip: IpAddr,
    pg_port: u16,
    direct_bridge: bool,
) -> (String, u16) {
    if direct_bridge {
        (pg_ip.to_string(), pg_port)
    } else {
        ("host.docker.internal".to_string(), pg_port)
    }
}

fn migrator_postgres_address(pg_ip: IpAddr, pg_port: u16) -> (String, u16) {
    migrator_postgres_address_inner(pg_ip, pg_port, is_direct_bridge_mode())
}

/// When direct bridge mode is active, containers are reachable via their
/// default-bridge IP rather than localhost:mapped-port. Use the internal
/// port directly, since cross-container traffic goes over the bridge network
/// without host port mapping.
async fn resolve_ip(
    container: &ContainerAsync<GenericImage>,
    internal_port: u16,
) -> Result<(IpAddr, u16)> {
    if is_direct_bridge_mode() {
        let ip = container.get_bridge_ip_address().await?;
        Ok((ip, internal_port))
    } else {
        let port = container
            .get_host_port_ipv4(ContainerPort::Tcp(internal_port))
            .await?;
        Ok((IpAddr::V4(std::net::Ipv4Addr::LOCALHOST), port))
    }
}

fn run_migrator_container(postgres_host: &str, postgres_port: u16) -> Result<()> {
    let mut current_dir = std::env::current_dir()?;
    loop {
        if current_dir.join("services/migrations/Dockerfile").exists() {
            break;
        }
        if !current_dir.pop() {
            return Err(anyhow::anyhow!(
                "Could not find services/migrations/Dockerfile"
            ));
        }
    }

    let build = Command::new("docker")
        .args([
            "build",
            "-f",
            "services/migrations/Dockerfile",
            "-t",
            "omni-migrator:test",
            ".",
        ])
        .current_dir(&current_dir)
        .output()?;
    if !build.status.success() {
        return Err(anyhow::anyhow!(
            "failed to build migrator image:\nstdout:\n{}\nstderr:\n{}",
            String::from_utf8_lossy(&build.stdout),
            String::from_utf8_lossy(&build.stderr)
        ));
    }

    let port = postgres_port.to_string();
    let run = Command::new("docker")
        .args([
            "run",
            "--rm",
            "--add-host",
            "host.docker.internal:host-gateway",
            "-e",
            &format!("DATABASE_HOST={postgres_host}"),
            "-e",
            &format!("DATABASE_PORT={port}"),
            "-e",
            "DATABASE_USERNAME=omni",
            "-e",
            "DATABASE_PASSWORD=omni_password",
            "-e",
            "DATABASE_NAME=omni_test",
            "-e",
            "DATABASE_SSL=false",
            "omni-migrator:test",
        ])
        .output()?;
    if !run.status.success() {
        return Err(anyhow::anyhow!(
            "migrator container failed:\nstdout:\n{}\nstderr:\n{}",
            String::from_utf8_lossy(&run.stdout),
            String::from_utf8_lossy(&run.stderr)
        ));
    }

    Ok(())
}

impl TestEnvironment {
    /// Create a new test environment with all dependencies
    pub async fn new() -> Result<Self> {
        tracing_subscriber::fmt::try_init().ok();

        // Start PostgreSQL with pgvector and pg_bm25 extensions (ParadeDB image)
        let postgres_image = GenericImage::new("paradedb/paradedb", "0.24.0-pg17")
            .with_wait_for(WaitFor::message_on_stderr(
                "database system is ready to accept connections",
            ))
            .with_exposed_port(ContainerPort::Tcp(5432))
            .with_env_var("POSTGRES_DB", "omni_test")
            .with_env_var("POSTGRES_USER", "omni")
            .with_env_var("POSTGRES_PASSWORD", "omni_password");
        let postgres_container = postgres_image.start().await?;
        let (pg_ip, pg_port) = resolve_ip(&postgres_container, 5432).await?;

        // Start Redis
        let redis_container = Redis::default().start().await?;
        let (redis_ip, redis_port) = resolve_ip_raw_redis(&redis_container, 6379).await?;

        // Start LocalStack (S3). Only the s3 service is needed; booting the
        // full service set is slow enough that the container log wait times
        // out before the ready banner is emitted.
        let localstack_container = LocalStack::default()
            .with_env_var("SERVICES", "s3")
            .start()
            .await?;
        let (ls_ip, ls_port) = resolve_ip_raw_localstack(&localstack_container, 4566).await?;

        let pg_host = pg_ip.to_string();
        let redis_host = redis_ip.to_string();
        let ls_host = ls_ip.to_string();

        let s3_endpoint = format!("http://{ls_host}:{ls_port}");

        // Run migrations. The migrator is a separate container: in direct
        // bridge mode it connects to Postgres's bridge IP, otherwise it uses
        // the Docker host gateway (never localhost, which would resolve to the
        // migrator container itself).
        let (migrator_host, migrator_port) = migrator_postgres_address(pg_ip, pg_port);
        run_migrator_container(&migrator_host, migrator_port)?;

        // Create database connection. ParadeDB restarts once after loading
        // extensions, so use a generous acquire timeout instead of the
        // 3-second default.
        let database_url = format!("postgresql://omni:omni_password@{pg_host}:{pg_port}/omni_test");
        let db_pool = DatabasePool::new(&database_url).await?;

        // Seed test data
        Self::seed_database(db_pool.pool()).await?;

        // Create Redis connection
        let redis_url = format!("redis://{redis_host}:{redis_port}");
        let redis_client = RedisClient::open(redis_url.as_str())?;

        // Clear Redis database
        let mut conn = redis_client.get_multiplexed_async_connection().await?;
        redis::cmd("FLUSHDB")
            .query_async::<String>(&mut conn)
            .await?;

        // Start mock AI server
        let mock_ai_server = MockAIServer::start().await?;

        unsafe {
            std::env::set_var("AWS_ACCESS_KEY_ID", "test");
            std::env::set_var("AWS_SECRET_ACCESS_KEY", "test");
            std::env::set_var("AWS_DEFAULT_REGION", "us-east-1");
        }

        Ok(Self {
            db_pool,
            redis_client,
            mock_ai_server,
            s3_endpoint,
            redis_url,
            _postgres_container: postgres_container,
            _redis_container: redis_container,
            _localstack_container: localstack_container,
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
            redis_url: self.redis_url.clone(),
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

        sqlx::query(
            r#"
            INSERT INTO embedding_providers (id, name, provider_type, config, is_current, is_deleted)
            VALUES ('01TEST_EMBED_PROV', 'test', 'local', '{"model": "test-model"}', TRUE, FALSE)
            ON CONFLICT (id) DO NOTHING
            "#,
        )
        .execute(pool)
        .await?;

        Ok(())
    }
}

/// Resolve bridge or mapped address for a `Redis` container (the module type
/// is different from `GenericImage` so we need separate helpers).
async fn resolve_ip_raw_redis(
    container: &ContainerAsync<Redis>,
    internal_port: u16,
) -> Result<(IpAddr, u16)> {
    if is_direct_bridge_mode() {
        let ip = container.get_bridge_ip_address().await?;
        Ok((ip, internal_port))
    } else {
        let port = container
            .get_host_port_ipv4(ContainerPort::Tcp(internal_port))
            .await?;
        Ok((IpAddr::V4(std::net::Ipv4Addr::LOCALHOST), port))
    }
}

async fn resolve_ip_raw_localstack(
    container: &ContainerAsync<LocalStack>,
    internal_port: u16,
) -> Result<(IpAddr, u16)> {
    if is_direct_bridge_mode() {
        let ip = container.get_bridge_ip_address().await?;
        Ok((ip, internal_port))
    } else {
        let port = container
            .get_host_port_ipv4(ContainerPort::Tcp(internal_port))
            .await?;
        Ok((IpAddr::V4(std::net::Ipv4Addr::LOCALHOST), port))
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
            Router,
            response::Json,
            routing::{get, post},
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
            embeddings: Vec<Vec<Vec<f32>>>,
            chunks_count: Vec<i32>,
            chunks: Vec<Vec<(i32, i32)>>,
            model_name: String,
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

        async fn mock_embeddings(Json(req): Json<EmbeddingRequest>) -> Json<EmbeddingResponse> {
            let mut embeddings = Vec::new();
            let mut chunks_count = Vec::new();
            let mut chunks = Vec::new();

            for text in &req.texts {
                let embedding = generate_test_embedding(text);
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

        async fn mock_rag(Json(req): Json<RagRequest>) -> Json<RagResponse> {
            let answer = format!(
                "Based on the context about '{}', here is the answer: {}",
                req.query,
                req.context.join(" ")
            );
            Json(RagResponse { answer })
        }

        async fn mock_generate(Json(req): Json<GenerateRequest>) -> Json<GenerateResponse> {
            let response = format!("Mock AI response for: {}", req.prompt);
            Json(GenerateResponse { response })
        }

        async fn health() -> (axum::http::StatusCode, &'static str) {
            (axum::http::StatusCode::OK, "OK")
        }

        let app = Router::new()
            .route("/embeddings", post(mock_embeddings))
            .route("/rag", post(mock_rag))
            .route("/generate", post(mock_generate))
            .route("/health", get(health));

        let listener = TcpListener::bind("127.0.0.1:0").await?;
        let addr = listener.local_addr()?;
        let port = addr.port();

        let server_handle = tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });

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

    #[test]
    fn migrator_host_follows_bridge_mode() {
        let pg_ip: IpAddr = "172.17.0.5".parse().unwrap();
        let pg_port: u16 = 5432;

        // Default mapped-port mode: the migrator container reaches Postgres
        // through the Docker host gateway, never localhost.
        assert_eq!(
            migrator_postgres_address_inner(pg_ip, pg_port, false),
            ("host.docker.internal".to_string(), pg_port)
        );

        // Direct bridge mode: the migrator uses the Postgres bridge IP with
        // the container's internal port.
        assert_eq!(
            migrator_postgres_address_inner(pg_ip, pg_port, true),
            (pg_ip.to_string(), pg_port)
        );
    }
}

/// Generate a deterministic 1024-dim embedding from text using word-level hashing.
/// Shared between the mock AI server and test data seeding so that semantically
/// similar texts (sharing words) produce similar embeddings.
pub fn generate_test_embedding(text: &str) -> Vec<f32> {
    let mut embedding = vec![0.0f32; 1024];
    let lower = text.to_lowercase();

    // Word-level hashing (primary signal)
    for word in lower.split_whitespace() {
        let mut hasher = DefaultHasher::new();
        word.hash(&mut hasher);
        let dim = (hasher.finish() % 1024) as usize;
        embedding[dim] += 1.0;
    }

    // Character trigram hashing (provides baseline overlap between texts)
    let chars: Vec<char> = lower.chars().collect();
    for window in chars.windows(3) {
        let mut hasher = DefaultHasher::new();
        window.hash(&mut hasher);
        let dim = (hasher.finish() % 1024) as usize;
        embedding[dim] += 0.1;
    }

    let norm: f32 = embedding.iter().map(|x| x * x).sum::<f32>().sqrt();
    if norm > 0.0 {
        for x in &mut embedding {
            *x /= norm;
        }
    }
    embedding
}

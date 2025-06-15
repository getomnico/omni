pub mod handlers;
pub mod models;
pub mod search;

use anyhow::Result as AnyhowResult;
use axum::{
    routing::{get, post},
    Router,
};
use redis::Client as RedisClient;
use shared::{AIClient, DatabasePool, SearcherConfig};
use std::net::SocketAddr;
use tower::ServiceBuilder;
use tower_http::{cors::CorsLayer, trace::TraceLayer};
use tracing::info;

pub type Result<T> = std::result::Result<T, SearcherError>;

#[derive(thiserror::Error, Debug)]
pub enum SearcherError {
    #[error("Database error: {0}")]
    Database(#[from] sqlx::Error),
    #[error("Redis error: {0}")]
    Redis(#[from] redis::RedisError),
    #[error("Serialization error: {0}")]
    Serialization(#[from] serde_json::Error),
    #[error("Internal error: {0}")]
    Internal(#[from] anyhow::Error),
}

impl axum::response::IntoResponse for SearcherError {
    fn into_response(self) -> axum::response::Response {
        let (status, message) = match self {
            SearcherError::Database(_) => (
                axum::http::StatusCode::INTERNAL_SERVER_ERROR,
                "Database error",
            ),
            SearcherError::Redis(_) => {
                (axum::http::StatusCode::INTERNAL_SERVER_ERROR, "Cache error")
            }
            SearcherError::Serialization(_) => (
                axum::http::StatusCode::BAD_REQUEST,
                "Invalid request format",
            ),
            SearcherError::Internal(_) => (
                axum::http::StatusCode::INTERNAL_SERVER_ERROR,
                "Internal server error",
            ),
        };

        let body = serde_json::json!({
            "error": message,
            "details": self.to_string()
        });

        (status, axum::Json(body)).into_response()
    }
}

#[derive(Clone)]
pub struct AppState {
    pub db_pool: DatabasePool,
    pub redis_client: RedisClient,
    pub ai_client: AIClient,
    pub config: SearcherConfig,
}

pub fn create_app(state: AppState) -> Router {
    Router::new()
        .route("/health", get(handlers::health_check))
        .route("/search", post(handlers::search))
        .route("/suggestions", get(handlers::suggestions))
        .layer(
            ServiceBuilder::new()
                .layer(TraceLayer::new_for_http())
                .layer(CorsLayer::permissive()),
        )
        .with_state(state)
}

pub async fn run_server() -> AnyhowResult<()> {
    dotenvy::dotenv().ok();

    tracing_subscriber::fmt::init();

    info!("Searcher service starting...");

    let config = SearcherConfig::from_env();

    let db_pool = DatabasePool::from_config(&config.database)
        .await
        .map_err(|e| anyhow::anyhow!("Failed to create database pool: {}", e))?;

    let redis_client = RedisClient::open(config.redis.redis_url.clone())?;
    info!("Redis client initialized");

    let ai_client = AIClient::new(config.ai_service_url.clone());
    info!("AI client initialized");

    let app_state = AppState {
        db_pool,
        redis_client,
        ai_client,
        config: config.clone(),
    };

    let app = create_app(app_state);

    let addr = SocketAddr::from(([0, 0, 0, 0], config.port));
    info!("Searcher service listening on {}", addr);

    let listener = tokio::net::TcpListener::bind(addr).await?;
    axum::serve(listener, app).await?;

    Ok(())
}

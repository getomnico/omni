pub mod handlers;
pub mod models;
pub mod search;

use anyhow::Result as AnyhowResult;
use axum::{
    routing::{get, post},
    Router,
};
use redis::Client as RedisClient;
use shared::{AIClient, DatabasePool};
use std::{env, net::SocketAddr};
use tower::ServiceBuilder;
use tower_http::{cors::CorsLayer, trace::TraceLayer};
use tracing::{error, info};

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

    let database_url = env::var("DATABASE_URL").expect("DATABASE_URL must be set");
    let redis_url = env::var("REDIS_URL").unwrap_or_else(|_| "redis://localhost:6379".to_string());
    let ai_service_url = env::var("AI_SERVICE_URL").unwrap_or_else(|_| "http://ai:3003".to_string());
    let port = env::var("PORT")
        .unwrap_or_else(|_| "3002".to_string())
        .parse::<u16>()
        .expect("PORT must be a valid number");

    let db_pool = DatabasePool::new(&database_url)
        .await
        .map_err(|e| anyhow::anyhow!("Failed to create database pool: {}", e))?;

    let redis_client = RedisClient::open(redis_url)?;
    info!("Redis client initialized");

    let ai_client = AIClient::new(ai_service_url);
    info!("AI client initialized");

    let app_state = AppState {
        db_pool,
        redis_client,
        ai_client,
    };

    let app = create_app(app_state);

    let addr = SocketAddr::from(([0, 0, 0, 0], port));
    info!("Searcher service listening on {}", addr);

    let listener = tokio::net::TcpListener::bind(addr).await?;
    axum::serve(listener, app).await?;

    Ok(())
}

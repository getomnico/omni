pub mod handlers;
pub mod models;
pub mod search;
pub mod suggested_questions;
pub mod typeahead;

use anyhow::Result as AnyhowResult;
use axum::{
    middleware,
    routing::{get, post},
    Router,
};
use redis::Client as RedisClient;
use shared::{
    telemetry::{self, TelemetryConfig},
    AIClient, DatabasePool, ObjectStorage, SearcherConfig, StorageFactory,
};
use std::net::SocketAddr;
use std::sync::Arc;
use tower::ServiceBuilder;
use tower_http::cors::CorsLayer;
use tracing::{error, info};

use crate::suggested_questions::SuggestedQuestionsGenerator;
use crate::typeahead::TitleIndex;

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
    #[error("Not found: {0}")]
    NotFound(String),
    #[error("Bad request: {0}")]
    BadRequest(String),
}

impl axum::response::IntoResponse for SearcherError {
    fn into_response(self) -> axum::response::Response {
        let (status, message) = match self {
            SearcherError::Database(_) => (
                axum::http::StatusCode::INTERNAL_SERVER_ERROR,
                "Database error".to_string(),
            ),
            SearcherError::Redis(_) => (
                axum::http::StatusCode::INTERNAL_SERVER_ERROR,
                "Cache error".to_string(),
            ),
            SearcherError::Serialization(_) => (
                axum::http::StatusCode::BAD_REQUEST,
                "Invalid request format".to_string(),
            ),
            SearcherError::Internal(_) => (
                axum::http::StatusCode::INTERNAL_SERVER_ERROR,
                "Internal server error".to_string(),
            ),
            SearcherError::NotFound(msg) => (axum::http::StatusCode::NOT_FOUND, msg),
            SearcherError::BadRequest(msg) => (axum::http::StatusCode::BAD_REQUEST, msg),
        };

        let body = serde_json::json!({
            "error": message,
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
    pub content_storage: Arc<dyn ObjectStorage>,
    pub suggested_questions_generator: Arc<SuggestedQuestionsGenerator>,
    pub title_index: Arc<TitleIndex>,
}

pub fn create_app(state: AppState) -> Router {
    Router::new()
        .route("/health", get(handlers::health_check))
        .route("/search", post(handlers::search))
        .route("/search/ai-answer", post(handlers::ai_answer))
        .route("/recent-searches", get(handlers::recent_searches))
        .route("/typeahead", get(handlers::typeahead))
        .route("/suggested-questions", post(handlers::suggested_questions))
        .layer(
            ServiceBuilder::new()
                .layer(middleware::from_fn(telemetry::middleware::trace_layer))
                .layer(CorsLayer::permissive()),
        )
        .with_state(state)
}

pub async fn run_server() -> AnyhowResult<()> {
    dotenvy::dotenv().ok();

    let telemetry_config = TelemetryConfig::from_env("omni-searcher");
    telemetry::init_telemetry(telemetry_config)?;

    info!("Searcher service starting...");

    let config = SearcherConfig::from_env();

    let db_pool = DatabasePool::from_config(&config.database)
        .await
        .map_err(|e| anyhow::anyhow!("Failed to create database pool: {}", e))?;

    let redis_client = RedisClient::open(config.redis.redis_url.clone())?;
    info!("Redis client initialized");

    let ai_client = AIClient::new(config.ai_service_url.clone());
    info!("AI client initialized");

    let content_storage = StorageFactory::from_env(db_pool.pool().clone()).await?;
    info!("Storage initialized");

    let suggested_questions_generator = Arc::new(SuggestedQuestionsGenerator::new(
        redis_client.clone(),
        db_pool.clone(),
        content_storage.clone(),
        ai_client.clone(),
    ));

    let title_index = Arc::new(TitleIndex::new(db_pool.clone()));
    if let Err(e) = title_index.refresh().await {
        error!("Failed initial typeahead index load: {}", e);
    }
    title_index.start_background_refresh(300);
    info!("Typeahead index initialized");

    let app_state = AppState {
        db_pool,
        redis_client,
        ai_client,
        config: config.clone(),
        content_storage,
        suggested_questions_generator,
        title_index,
    };

    let app = create_app(app_state);

    let addr = SocketAddr::from(([0, 0, 0, 0], config.port));
    info!("Searcher service listening on {}", addr);

    let listener = tokio::net::TcpListener::bind(addr).await?;
    axum::serve(listener, app).await?;

    Ok(())
}

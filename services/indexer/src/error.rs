use axum::{
    http::StatusCode,
    response::{IntoResponse, Response},
    Json,
};
use serde_json::json;
use tracing::error;

#[derive(Debug)]
pub enum IndexerError {
    Database(sqlx::Error),
    Redis(redis::RedisError),
    Serialization(serde_json::Error),
    NotFound(String),
    BadRequest(String),
    Internal(String),
}

impl std::fmt::Display for IndexerError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            IndexerError::Database(e) => write!(f, "Database error: {}", e),
            IndexerError::Redis(e) => write!(f, "Redis error: {}", e),
            IndexerError::Serialization(e) => write!(f, "Serialization error: {}", e),
            IndexerError::NotFound(msg) => write!(f, "Not found: {}", msg),
            IndexerError::BadRequest(msg) => write!(f, "Bad request: {}", msg),
            IndexerError::Internal(msg) => write!(f, "Internal error: {}", msg),
        }
    }
}

impl std::error::Error for IndexerError {}

impl From<sqlx::Error> for IndexerError {
    fn from(err: sqlx::Error) -> Self {
        IndexerError::Database(err)
    }
}

impl From<redis::RedisError> for IndexerError {
    fn from(err: redis::RedisError) -> Self {
        IndexerError::Redis(err)
    }
}

impl From<serde_json::Error> for IndexerError {
    fn from(err: serde_json::Error) -> Self {
        IndexerError::Serialization(err)
    }
}

impl IntoResponse for IndexerError {
    fn into_response(self) -> Response {
        let (status, error_message) = match &self {
            IndexerError::Database(_) => {
                error!("Database error: {}", self);
                (StatusCode::INTERNAL_SERVER_ERROR, "Database error")
            }
            IndexerError::Redis(_) => {
                error!("Redis error: {}", self);
                (StatusCode::INTERNAL_SERVER_ERROR, "Redis error")
            }
            IndexerError::Serialization(_) => {
                error!("Serialization error: {}", self);
                (StatusCode::BAD_REQUEST, "Invalid data format")
            }
            IndexerError::NotFound(msg) => (StatusCode::NOT_FOUND, msg.as_str()),
            IndexerError::BadRequest(msg) => (StatusCode::BAD_REQUEST, msg.as_str()),
            IndexerError::Internal(msg) => {
                error!("Internal error: {}", msg);
                (StatusCode::INTERNAL_SERVER_ERROR, "Internal server error")
            }
        };

        let body = Json(json!({
            "error": error_message,
            "timestamp": chrono::Utc::now().to_rfc3339()
        }));

        (status, body).into_response()
    }
}

pub type Result<T> = std::result::Result<T, IndexerError>;
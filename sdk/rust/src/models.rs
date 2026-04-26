use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use serde::{Deserialize, Serialize};
use serde_json::Value as JsonValue;
use shared::models::{ServiceCredentials, SyncType};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SyncRequest {
    pub sync_run_id: String,
    pub source_id: String,
    pub sync_mode: SyncType,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_sync_at: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SyncResponse {
    pub status: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
}

impl SyncResponse {
    pub fn started() -> Self {
        Self {
            status: "started".to_string(),
            message: None,
        }
    }

    pub fn error(message: impl Into<String>) -> Self {
        Self {
            status: "error".to_string(),
            message: Some(message.into()),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CancelRequest {
    pub sync_run_id: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CancelResponse {
    pub status: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SyncStatusResponse {
    pub running: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ActionRequest {
    pub action: String,
    #[serde(default)]
    pub params: JsonValue,
    #[serde(default)]
    pub credentials: Option<ServiceCredentials>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ActionResponse {
    pub status: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub result: Option<JsonValue>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

impl ActionResponse {
    pub fn success(result: JsonValue) -> Self {
        Self {
            status: "success".to_string(),
            result: Some(result),
            error: None,
        }
    }

    pub fn failure(message: impl Into<String>) -> Self {
        Self {
            status: "error".to_string(),
            result: None,
            error: Some(message.into()),
        }
    }

    pub fn not_supported(action: &str) -> Self {
        Self::failure(format!("Action not supported: {}", action))
    }

    /// Serialize this ActionResponse into an axum HTTP Response with the
    /// default status code (200 for success, 400 for error).
    pub fn into_response(self) -> Response {
        let status = match self.status.as_str() {
            "success" => StatusCode::OK,
            _ => StatusCode::BAD_REQUEST,
        };
        self.into_response_with_status(status)
    }

    /// Serialize this ActionResponse into an axum HTTP Response with a
    /// specific status code.
    pub fn into_response_with_status(self, status: StatusCode) -> Response {
        let body = serde_json::to_string(&self).unwrap_or_default();
        (
            status,
            [("content-type", mime::APPLICATION_JSON.essence_str())],
            body,
        )
            .into_response()
    }
}

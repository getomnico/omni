use serde::{Deserialize, Serialize};
use serde_json::Value as JsonValue;
use shared::models::SyncType;

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
    pub credentials: JsonValue,
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
            status: "ok".to_string(),
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
}

/// The result returned by a connector's `execute_action` method.
///
/// `Json` is the standard path — the SDK wraps it in a JSON response.
/// `Binary` is for actions that return raw bytes (e.g. file downloads)
/// — the SDK sets `Content-Type` and `Content-Length` headers automatically.
#[derive(Debug, Clone)]
pub enum ActionResult {
    /// Standard JSON action response
    Json(ActionResponse),
    /// Binary response — bytes + content type.
    Binary(Vec<u8>, String),
}

impl ActionResult {
    pub fn json(resp: ActionResponse) -> Self {
        Self::Json(resp)
    }

    pub fn binary(bytes: Vec<u8>, content_type: impl Into<String>) -> Self {
        Self::Binary(bytes, content_type.into())
    }

    pub fn not_supported(action: &str) -> Self {
        Self::Json(ActionResponse::not_supported(action))
    }
}

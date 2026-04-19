use serde::{Deserialize, Serialize};
use serde_json::Value as JsonValue;
use shared::models::SyncType;

/// Wire representation of a sync mode. Matches the Python/TypeScript SDKs'
/// `SyncMode` type and serializes as a lowercase string (`"full"`,
/// `"incremental"`, `"realtime"`).
pub type SyncMode = SyncType;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SyncRequest {
    pub sync_run_id: String,
    pub source_id: String,
    pub sync_mode: SyncMode,
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

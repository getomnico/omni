use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use serde::{Deserialize, Serialize};
use serde_json::Value as JsonValue;
use shared::models::{ServiceCredential, SyncType};
use std::collections::HashMap;

/// Declarative OAuth2 configuration that connectors put on their manifest.
/// Pure data: the web app's generic OAuth2 client uses these fields to drive
/// the standard authorization-code flow. Provider quirks that can't be
/// expressed as data (e.g., Atlassian's post-exchange `cloudId` resolution)
/// belong on the optional `enrich_endpoint`, which the connector itself
/// implements.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct OAuthManifestConfig {
    /// Provider identifier (matches `connector_configs.provider` for the
    /// client_id/client_secret lookup). Stored as `service_credentials.provider`
    /// after a successful exchange.
    pub provider: String,
    pub auth_endpoint: String,
    pub token_endpoint: String,
    /// GET endpoint that returns a JSON object with the authenticated user's
    /// email at `userinfo_email_field`.
    pub userinfo_endpoint: String,
    #[serde(default = "default_email_field")]
    pub userinfo_email_field: String,
    /// Identity-only scopes always added to every authorization request
    /// (e.g. ["email", "profile"]).
    #[serde(default)]
    pub identity_scopes: Vec<String>,
    /// Per source_type read/write scope sets.
    #[serde(default)]
    pub scopes: HashMap<String, OAuthScopeSet>,
    /// Extra static query params on the authorization URL
    /// (e.g. {"access_type": "offline", "prompt": "consent"} for Google).
    #[serde(default)]
    pub extra_auth_params: HashMap<String, String>,
    #[serde(default = "default_scope_separator")]
    pub scope_separator: String,
    /// Optional path on the connector hit after token exchange to resolve
    /// provider-specific extras (e.g. Atlassian cloudId). The connector
    /// receives `{access_token, refresh_token}` and returns
    /// `{credentials_extra?, config_extra?}` to be merged into the row.
    #[serde(default)]
    pub enrich_endpoint: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
pub struct OAuthScopeSet {
    #[serde(default)]
    pub read: Vec<String>,
    #[serde(default)]
    pub write: Vec<String>,
}

fn default_email_field() -> String {
    "email".to_string()
}

fn default_scope_separator() -> String {
    " ".to_string()
}

use crate::mcp_adapter::McpCredentials;

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
    pub credentials: Option<ServiceCredential>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ResourceRequest {
    pub uri: String,
    #[serde(default)]
    pub credentials: McpCredentials,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PromptRequest {
    pub name: String,
    #[serde(default)]
    pub arguments: Option<JsonValue>,
    #[serde(default)]
    pub credentials: McpCredentials,
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

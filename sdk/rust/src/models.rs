use serde::{Deserialize, Serialize};
pub use shared::models::{
    ActionActor, ActionContext, ActionRequest, ActionResponse, CancelRequest, CancelResponse,
    McpCredentials, PromptRequest, ResourceRequest, SkillRequest, SkillResponse, SyncRequest,
    SyncResponse, SyncStatusResponse,
};
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

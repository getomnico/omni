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
    /// OAuth Dynamic Client Registration endpoint. Set this for providers where
    /// Omni should auto-create an OAuth client instead of asking admins to
    /// configure one manually.
    #[serde(default)]
    pub registration_endpoint: Option<String>,
    /// OAuth token endpoint client authentication method. Public DCR clients
    /// usually use `none`, which tells Omni not to require or send a client
    /// secret and to treat the provider as auto-managed when
    /// `registration_endpoint` is also present.
    #[serde(default)]
    pub token_endpoint_auth_method: OAuthTokenEndpointAuthMethod,
    /// Optional OAuth resource indicator (RFC 8707) sent on auth/token requests
    /// for providers that bind tokens to a specific resource, such as a remote
    /// MCP server.
    #[serde(default)]
    pub resource: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "snake_case")]
pub enum OAuthTokenEndpointAuthMethod {
    #[default]
    ClientSecretPost,
    ClientSecretBasic,
    None,
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

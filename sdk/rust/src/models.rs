use serde::{Deserialize, Serialize};
use serde_json::Value as JsonValue;
pub use shared::models::{
    ActionRequest, ActionResponse, CancelRequest, CancelResponse, McpCredentials, PromptRequest,
    ResourceRequest, SkillRequest, SkillResponse, SyncRequest, SyncResponse, SyncStatusResponse,
    UserRole,
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
    /// Optional GET endpoint that returns the authenticated user's email at
    /// `userinfo_email_field`. When absent, the OAuth callback binds the
    /// credential to the already-authenticated Omni user.
    #[serde(default)]
    pub userinfo_endpoint: Option<String>,
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
    /// Whether dynamic registration requires an administrator-provided initial
    /// access token.
    #[serde(default)]
    pub registration_requires_initial_access_token: bool,
    /// Additional token response fields to preserve in credentials.
    #[serde(default)]
    pub token_response_fields: Vec<String>,
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
    /// Optional source config key containing an OAuth issuer URL. The web OAuth
    /// client uses standard OpenID Connect discovery when present.
    #[serde(default)]
    pub issuer_source_config_key: Option<String>,
    /// Optional template for source-scoped OAuth client configuration. The
    /// `{source_id}` placeholder is replaced for a persisted source.
    #[serde(default)]
    pub client_config_provider_template: Option<String>,
    /// Whether authorization requests must use PKCE.
    #[serde(default)]
    pub pkce_required: bool,
    /// Optional OAuth Dynamic Client Registration grant types.
    #[serde(default)]
    pub grant_types: Option<Vec<String>>,
    /// Whether OAuth endpoints from this manifest require SSRF-safe URL
    /// validation before server-side requests.
    #[serde(default)]
    pub validate_endpoint_urls: bool,
    /// Whether this connector supports OAuth credentials for org sources.
    #[serde(default = "default_supports_org_oauth")]
    pub supports_org_oauth: bool,
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

/// Notification sent by connector-manager after a user OAuth credential is
/// stored. Native MCP connectors use it to perform authenticated discovery
/// and return a refreshed manifest.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OAuthCredentialReadyRequest {
    pub source_id: String,
    #[serde(default)]
    pub user_id: Option<String>,
    pub provider: String,
    pub flow: String,
    #[serde(default)]
    pub credentials: JsonValue,
}

fn default_supports_org_oauth() -> bool {
    true
}

fn default_email_field() -> String {
    "email".to_string()
}

fn default_scope_separator() -> String {
    " ".to_string()
}

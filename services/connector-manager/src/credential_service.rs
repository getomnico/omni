use std::collections::HashMap;
use std::sync::{Arc, OnceLock};
use std::time::Duration;

use serde::Deserialize;
use serde_json::Value as JsonValue;
use shared::db::repositories::{ConnectorConfigRepository, ServiceCredentialsRepo};
use shared::models::{AuthType, ServiceCredential, Source};
use shared::DatabasePool;
use thiserror::Error;
use time::OffsetDateTime;
use tokio::sync::Mutex;
use tracing::debug;

use crate::remote_mcp::gateway::{
    pinned_http_client_for_url, read_limited_response_text, validate_endpoint_for_gateway,
};
use crate::remote_mcp::oauth::{RemoteMcpOAuthConfig, TokenEndpointAuthMethod};

/// Helper: serialises [`ServiceProvider`] to its JSON/DB string form.
fn provider_to_string(p: &shared::models::ServiceProvider) -> String {
    serde_json::to_value(p)
        .ok()
        .and_then(|v| v.as_str().map(String::from))
        .unwrap_or_default()
}

const REFRESH_SKEW_SECONDS: i64 = 300;

/// Errors returned by [`CredentialService`].
#[derive(Debug, Error)]
pub enum CredentialServiceError {
    #[error("missing OAuth field: {0}")]
    MissingField(&'static str),
    #[error("credential requires OAuth reconnect")]
    ReconnectRequired,
    #[error("OAuth refresh failed: {0}")]
    RefreshFailed(String),
    #[error("repository error: {0}")]
    Repository(String),
}

/// Wraps the dumb [`ServiceCredentialsRepo`] with OAuth token lifecycle
/// management.
///
/// * Credential reads that need a token refresh perform the HTTP request
///   **outside** any database transaction.
/// * Concurrency for the same credential (source + user) is serialised with
///   an in-process `Mutex` so rotating refresh tokens are never replayed.
/// * Client id/secret/token endpoint are resolved from the connector config
///   (`ConnectorConfigRepository`) with fallback to values stored in the
///   credential JSON, and optional overrides from remote-MCP discovery
///   metadata ([`RemoteMcpOAuthConfig`]).
pub struct CredentialService {
    db_pool: DatabasePool,
}

static CREDENTIAL_REFRESH_LOCKS: OnceLock<Mutex<HashMap<String, Arc<Mutex<()>>>>> = OnceLock::new();

impl CredentialService {
    pub fn new(db_pool: DatabasePool) -> Self {
        Self { db_pool }
    }

    // ── Public API ────────────────────────────────────────────────

    /// Fetch the org-wide credential (`user_id IS NULL`) for a source,
    /// transparently refreshing an expiring OAuth token.
    ///
    /// If the credential lacks the OAuth fields needed for a refresh
    /// (`refresh_token`, `client_id`, `token_uri`) it is returned unchanged
    /// rather than erroring.
    pub async fn get_org_credential(
        &self,
        source_id: &str,
    ) -> Result<Option<ServiceCredential>, CredentialServiceError> {
        let repo = self.repo()?;
        let credential = repo
            .find_org_credential(source_id)
            .await
            .map_err(|e| CredentialServiceError::Repository(e.to_string()))?;
        match credential {
            Some(c) => self.refresh_if_needed(&c).await.map(Some),
            None => Ok(None),
        }
    }

    /// Fetch a per-user credential row for an org-wide source,
    /// transparently refreshing an expiring OAuth token.
    ///
    /// If the credential lacks the OAuth fields needed for a refresh
    /// it is returned unchanged rather than erroring.
    pub async fn get_user_credential(
        &self,
        source_id: &str,
        user_id: &str,
    ) -> Result<Option<ServiceCredential>, CredentialServiceError> {
        let repo = self.repo()?;
        let credential = repo
            .find_user_credential(source_id, user_id)
            .await
            .map_err(|e| CredentialServiceError::Repository(e.to_string()))?;
        match credential {
            Some(c) => self.refresh_if_needed(&c).await.map(Some),
            None => Ok(None),
        }
    }

    /// Fetch the credential that "owns" a source (org-scoped → org row,
    /// user-scoped → user row keyed on the creator), refreshing if needed.
    pub async fn get_owner_credential(
        &self,
        source: &Source,
    ) -> Result<Option<ServiceCredential>, CredentialServiceError> {
        let repo = self.repo()?;
        let credential = repo
            .find_owner_credential(source)
            .await
            .map_err(|e| CredentialServiceError::Repository(e.to_string()))?;
        match credential {
            Some(c) => self.refresh_if_needed(&c).await.map(Some),
            None => Ok(None),
        }
    }

    // ── Raw (no-refresh) variants ─────────────────────────────────

    /// Fetch org credential **without** OAuth refresh.
    pub async fn raw_org_credential(
        &self,
        source_id: &str,
    ) -> Result<Option<ServiceCredential>, CredentialServiceError> {
        let repo = self.repo()?;
        repo.find_org_credential(source_id)
            .await
            .map_err(|e| CredentialServiceError::Repository(e.to_string()))
    }

    /// Fetch per-user credential **without** OAuth refresh.
    pub async fn raw_user_credential(
        &self,
        source_id: &str,
        user_id: &str,
    ) -> Result<Option<ServiceCredential>, CredentialServiceError> {
        let repo = self.repo()?;
        repo.find_user_credential(source_id, user_id)
            .await
            .map_err(|e| CredentialServiceError::Repository(e.to_string()))
    }

    /// Fetch owner credential **without** OAuth refresh — a raw passthrough
    /// for callers that only need the stored value (e.g. SDK sync-config).
    pub async fn raw_owner_credential(
        &self,
        source: &Source,
    ) -> Result<Option<ServiceCredential>, CredentialServiceError> {
        let repo = self.repo()?;
        repo.find_owner_credential(source)
            .await
            .map_err(|e| CredentialServiceError::Repository(e.to_string()))
    }

    // ── Remote-MCP refresh ────────────────────────────────────────

    /// Refresh a credential using remote-MCP-style OAuth discovery metadata.
    /// Used by the remote MCP gateway which has already parsed a
    /// [`RemoteMcpOAuthConfig`] from the endpoint's discovery response.
    ///
    /// Unlike the generic refresh path, this returns [`ReconnectRequired`]
    /// when the credential is missing a `refresh_token` because the remote
    /// MCP flow requires user re-authentication.
    pub(crate) async fn refresh_credential_with_oauth(
        &self,
        _source: &Source,
        credential: ServiceCredential,
        oauth: &RemoteMcpOAuthConfig,
    ) -> Result<ServiceCredential, CredentialServiceError> {
        if credential.auth_type != AuthType::OAuth {
            return Ok(credential);
        }

        let key = format!(
            "{}:{}",
            credential.source_id,
            credential.user_id.as_deref().unwrap_or("__org__")
        );
        let locks = CREDENTIAL_REFRESH_LOCKS.get_or_init(|| Mutex::new(HashMap::new()));
        let lock = {
            let mut guard = locks.lock().await;
            guard
                .entry(key.clone())
                .or_insert_with(|| Arc::new(Mutex::new(())))
                .clone()
        };
        let _guard = lock.lock().await;

        let repo = self.repo()?;
        let mut credential = repo
            .find_by_id(&credential.id)
            .await
            .map_err(|e| CredentialServiceError::Repository(e.to_string()))?
            .ok_or_else(|| CredentialServiceError::Repository("credential disappeared".into()))?;

        if !credential_needs_refresh(&credential, OffsetDateTime::now_utc()) {
            return Ok(credential);
        }

        let config_repo = ConnectorConfigRepository::new(self.db_pool.pool().clone());
        let connector_config = config_repo
            .get_by_provider(&oauth.provider)
            .await
            .map_err(|e| CredentialServiceError::Repository(e.to_string()))?
            .map(|row| row.config)
            .unwrap_or_else(|| serde_json::json!({}));

        let refreshed = do_oauth_refresh(
            &mut credential,
            &connector_config,
            oauth,
            true, // remote_mcp = true → errors on missing refresh_token
            None, // http_client
        )
        .await?;

        repo.update_credentials(&refreshed)
            .await
            .map_err(|e| CredentialServiceError::Repository(e.to_string()))?;

        debug!(
            source_id = %refreshed.source_id,
            "refreshed remote MCP OAuth credential"
        );
        Ok(refreshed)
    }

    // ── Refresh orchestration ─────────────────────────────────────

    /// Refresh OAuth tokens when the credential is about to expire.
    /// The HTTP POST to the token endpoint happens **outside** any DB
    /// transaction. Concurrency is serialised with an in-process mutex.
    ///
    /// If the credential lacks the required fields for a refresh
    /// (`refresh_token`, `client_id`, `token_uri`), it is returned
    /// unchanged.
    async fn refresh_if_needed(
        &self,
        credential: &ServiceCredential,
    ) -> Result<ServiceCredential, CredentialServiceError> {
        if credential.auth_type != AuthType::OAuth {
            return Ok(credential.clone());
        }
        if !credential_needs_refresh(credential, OffsetDateTime::now_utc()) {
            return Ok(credential.clone());
        }

        // Acquire the in-process mutex for this credential so that
        // concurrent requests don't race on a rotating refresh token.
        let key = format!(
            "{}:{}",
            credential.source_id,
            credential.user_id.as_deref().unwrap_or("__org__")
        );
        let locks = CREDENTIAL_REFRESH_LOCKS.get_or_init(|| Mutex::new(HashMap::new()));
        let lock = {
            let mut guard = locks.lock().await;
            guard
                .entry(key.clone())
                .or_insert_with(|| Arc::new(Mutex::new(())))
                .clone()
        };
        let _guard = lock.lock().await;

        // Re-read the credential from the DB — another thread may have
        // refreshed it while we waited for the mutex.
        let repo = self.repo()?;
        let mut credential = repo
            .find_by_id(&credential.id)
            .await
            .map_err(|e| CredentialServiceError::Repository(e.to_string()))?
            .ok_or_else(|| CredentialServiceError::Repository("credential disappeared".into()))?;

        if !credential_needs_refresh(&credential, OffsetDateTime::now_utc()) {
            return Ok(credential);
        }

        // Resolve provider string for connector-config lookup.
        let provider_str = provider_to_string(&credential.provider);
        let config_repo = ConnectorConfigRepository::new(self.db_pool.pool().clone());
        let connector_config = config_repo
            .get_by_provider(&provider_str)
            .await
            .map_err(|e| CredentialServiceError::Repository(e.to_string()))?
            .map(|row| row.config)
            .unwrap_or_else(|| serde_json::json!({}));

        // Build a RemoteMcpOAuthConfig-like view from native fields.
        // This allows us to reuse do_oauth_refresh for both native and
        // remote-MCP paths.
        let token_uri = string_from(&connector_config, "oauth_token_endpoint")
            .or_else(|| string_from(&credential.credentials, "token_uri"));

        // If we don't even have a token endpoint, there is nothing to
        // refresh — return the credential unchanged.
        let Some(token_endpoint) = token_uri else {
            return Ok(credential);
        };

        // Resolve auth method with proper precedence:
        //   connector config → credential JSON → heuristic
        let auth_method_str = string_from(&connector_config, "oauth_token_endpoint_auth_method")
            .or_else(|| string_from(&credential.credentials, "token_endpoint_auth_method"));
        let has_secret = string_from(&connector_config, "oauth_client_secret")
            .or_else(|| string_from(&credential.credentials, "client_secret"))
            .is_some();
        let auth_method = match auth_method_str.as_deref() {
            Some("client_secret_basic") => TokenEndpointAuthMethod::ClientSecretBasic,
            Some("none") => TokenEndpointAuthMethod::None,
            _ => {
                if has_secret {
                    TokenEndpointAuthMethod::ClientSecretPost
                } else {
                    TokenEndpointAuthMethod::None
                }
            }
        };

        let native_oauth = NativeOAuthParams {
            provider: provider_str,
            credential_provider: String::new(),
            token_endpoint,
            token_endpoint_auth_method: auth_method,
            resource: string_from(&credential.credentials, "resource"),
        };

        // Generic native refresh: if any required OAuth field is missing,
        // return the credential unchanged rather than erroring. The caller
        // can surface the issue naturally (e.g. a sync will fail).
        if !native_oauth_is_refreshable(&connector_config, &credential.credentials) {
            return Ok(credential);
        }
        let refresh_token = credential
            .credentials
            .get("refresh_token")
            .and_then(|v| v.as_str())
            .filter(|v| !v.is_empty())
            .map(str::to_owned)
            .unwrap();

        let refreshed = do_native_refresh(
            &mut credential,
            &connector_config,
            &native_oauth,
            &refresh_token,
            None,
        )
        .await;

        let refreshed = refreshed?;
        repo.update_credentials(&refreshed)
            .await
            .map_err(|e| CredentialServiceError::Repository(e.to_string()))?;
        debug!(
            source_id = %refreshed.source_id,
            provider = ?refreshed.provider,
            "refreshed OAuth credential"
        );
        Ok(refreshed)
    }

    fn repo(&self) -> Result<ServiceCredentialsRepo, CredentialServiceError> {
        ServiceCredentialsRepo::new(self.db_pool.pool().clone())
            .map_err(|e| CredentialServiceError::Repository(e.to_string()))
    }
}

// ── Predicates ─────────────────────────────────────────────────────

fn credential_needs_refresh(credential: &ServiceCredential, now: OffsetDateTime) -> bool {
    match credential.expires_at {
        Some(expires_at) => expires_at <= now + time::Duration::seconds(REFRESH_SKEW_SECONDS),
        None => false,
    }
}

/// Whether the credential has the minimum fields needed for a native
/// OAuth token refresh: a nonempty `refresh_token`, a resolvable
/// `client_id` (from connector config or credential JSON), and a
/// resolvable token endpoint (from connector config or credential JSON).
fn native_oauth_is_refreshable(connector_config: &JsonValue, credential_json: &JsonValue) -> bool {
    let has_refresh_token = credential_json
        .get("refresh_token")
        .and_then(|v| v.as_str())
        .is_some_and(|s| !s.is_empty());
    let has_client_id = string_from(connector_config, "oauth_client_id")
        .or_else(|| string_from(credential_json, "client_id"))
        .is_some();
    let has_token_endpoint = string_from(connector_config, "oauth_token_endpoint")
        .or_else(|| string_from(credential_json, "token_uri"))
        .is_some();
    has_refresh_token && has_client_id && has_token_endpoint
}

// ── Native OAuth params (remote-MCP-style view of native credential) ─

struct NativeOAuthParams {
    provider: String,
    credential_provider: String,
    token_endpoint: String,
    token_endpoint_auth_method: TokenEndpointAuthMethod,
    resource: Option<String>,
}

// ── Refresh logic ─────────────────────────────────────────────────

#[derive(Debug, Deserialize)]
struct RefreshResponse {
    access_token: String,
    refresh_token: Option<String>,
    token_type: Option<String>,
    expires_in: Option<i64>,
}

/// Perform a full OAuth token refresh for a native credential.
///
/// `remote_mcp` controls error behaviour: when `true`, a missing
/// `refresh_token` produces [`ReconnectRequired`]; when `false` the
/// caller is expected to have already handled the missing-token case.
async fn do_oauth_refresh(
    credential: &mut ServiceCredential,
    connector_config: &JsonValue,
    oauth: &RemoteMcpOAuthConfig,
    remote_mcp: bool,
    http_client: Option<reqwest::Client>,
) -> Result<ServiceCredential, CredentialServiceError> {
    let refresh_token = credential
        .credentials
        .get("refresh_token")
        .and_then(|v| v.as_str())
        .filter(|v| !v.is_empty())
        .ok_or_else(|| {
            if remote_mcp {
                CredentialServiceError::ReconnectRequired
            } else {
                CredentialServiceError::MissingField("refresh_token")
            }
        })?;

    let client_id = resolve_oauth_param(
        connector_config,
        &credential.credentials,
        "oauth_client_id",
        "client_id",
    )
    .ok_or(CredentialServiceError::MissingField("oauth_client_id"))?;
    let client_secret = resolve_oauth_param(
        connector_config,
        &credential.credentials,
        "oauth_client_secret",
        "client_secret",
    );
    let token_endpoint = resolve_oauth_param(
        connector_config,
        &credential.credentials,
        "oauth_token_endpoint",
        "token_uri",
    )
    .unwrap_or_else(|| oauth.token_endpoint.clone());
    let token_method = token_method_from_params(connector_config, &credential.credentials, oauth);
    let token_endpoint = &token_endpoint;

    let mut params = vec![
        ("grant_type".to_string(), "refresh_token".to_string()),
        ("refresh_token".to_string(), refresh_token.to_string()),
    ];
    let mut basic_auth: Option<(String, String)> = None;
    match token_method {
        TokenEndpointAuthMethod::None => {
            params.push(("client_id".to_string(), client_id));
        }
        TokenEndpointAuthMethod::ClientSecretPost => {
            params.push(("client_id".to_string(), client_id));
            if let Some(secret) = client_secret {
                params.push(("client_secret".to_string(), secret));
            }
        }
        TokenEndpointAuthMethod::ClientSecretBasic => {
            let secret =
                client_secret.ok_or(CredentialServiceError::MissingField("oauth_client_secret"))?;
            basic_auth = Some((client_id, secret));
        }
    }

    if let Some(resource) = &oauth.resource {
        if remote_mcp {
            validate_endpoint_for_gateway(resource)
                .await
                .map_err(|e| CredentialServiceError::RefreshFailed(e.to_string()))?;
        }
        params.push(("resource".to_string(), resource.clone()));
    }

    let client = match http_client {
        Some(c) => c,
        None => {
            if remote_mcp {
                pinned_http_client_for_url(token_endpoint)
                    .await
                    .map_err(|e| CredentialServiceError::RefreshFailed(e.to_string()))?
            } else {
                reqwest::Client::new()
            }
        }
    };
    let mut request = client
        .post(token_endpoint)
        .timeout(Duration::from_secs(20))
        .header("accept", "application/json");
    if let Some((client_id, client_secret)) = basic_auth {
        request = request.basic_auth(client_id, Some(client_secret));
    }

    apply_refresh_response(
        credential,
        token_endpoint,
        request.form(&params).send().await,
    )
    .await
}

/// Native-only refresh path without the `RemoteMcpOAuthConfig` wrapper.
async fn do_native_refresh(
    credential: &mut ServiceCredential,
    connector_config: &JsonValue,
    params: &NativeOAuthParams,
    _refresh_token: &str,
    http_client: Option<reqwest::Client>,
) -> Result<ServiceCredential, CredentialServiceError> {
    let oauth = RemoteMcpOAuthConfig {
        provider: params.provider.clone(),
        credential_provider: params.credential_provider.clone(),
        token_endpoint: params.token_endpoint.clone(),
        token_endpoint_auth_method: params.token_endpoint_auth_method.clone(),
        resource: params.resource.clone(),
    };

    // For the native path we already verified the refresh_token exists,
    // so pass remote_mcp=false so the error path isn't hit.
    do_oauth_refresh(credential, connector_config, &oauth, false, http_client).await
}

/// Resolve auth method with precedence:
/// 1. Connector config `oauth_token_endpoint_auth_method`
/// 2. Credential JSON `token_endpoint_auth_method`
/// 3. Supplied metadata (`oauth.token_endpoint_auth_method`).
///    Native metadata already contains the heuristic choice.
fn token_method_from_params(
    connector_config: &JsonValue,
    credential: &JsonValue,
    oauth: &RemoteMcpOAuthConfig,
) -> TokenEndpointAuthMethod {
    // 1. Explicit from connector config wins.
    if let Some(method) = string_from(connector_config, "oauth_token_endpoint_auth_method") {
        return TokenEndpointAuthMethod::parse(Some(&method));
    }
    // 2. Then credential JSON.
    if let Some(method) = string_from(credential, "token_endpoint_auth_method") {
        return TokenEndpointAuthMethod::parse(Some(&method));
    }
    // 3. Supplied metadata (native path caller already resolved heuristic
    //    into this value; remote-MCP comes from endpoint discovery).
    oauth.token_endpoint_auth_method.clone()
}

/// Resolve a parameter from connector config first, then credential JSON,
/// with separate key names for each source.
fn resolve_oauth_param(
    connector_config: &JsonValue,
    credential: &JsonValue,
    connector_key: &str,
    credential_key: &str,
) -> Option<String> {
    string_from(connector_config, connector_key).or_else(|| string_from(credential, credential_key))
}

/// Perform the HTTP POST, parse the response, populate the credential, and
/// return the updated credential (or an error).
async fn apply_refresh_response(
    credential: &mut ServiceCredential,
    token_endpoint: &str,
    response: Result<reqwest::Response, reqwest::Error>,
) -> Result<ServiceCredential, CredentialServiceError> {
    let response = response.map_err(|e| CredentialServiceError::RefreshFailed(e.to_string()))?;
    let status = response.status();
    let body = read_limited_response_text(response)
        .await
        .map_err(|e| CredentialServiceError::RefreshFailed(e.to_string()))?;
    if !status.is_success() {
        if is_reconnect_required_refresh_failure(status.as_u16(), &body) {
            return Err(CredentialServiceError::ReconnectRequired);
        }
        return Err(CredentialServiceError::RefreshFailed(format!(
            "token endpoint returned HTTP {status}"
        )));
    }
    let refreshed: RefreshResponse = serde_json::from_str(&body)
        .map_err(|e| CredentialServiceError::RefreshFailed(e.to_string()))?;

    let now = OffsetDateTime::now_utc();

    credential.credentials["access_token"] = JsonValue::String(refreshed.access_token);
    if let Some(refresh_token) = refreshed.refresh_token {
        credential.credentials["refresh_token"] = JsonValue::String(refresh_token);
    }
    credential.credentials["token_type"] =
        JsonValue::String(refreshed.token_type.unwrap_or_else(|| "Bearer".to_string()));
    credential.credentials["token_uri"] = JsonValue::String(token_endpoint.to_string());

    let expires_in = refreshed
        .expires_in
        .filter(|seconds| *seconds > 0)
        .unwrap_or(3600);
    credential.expires_at = Some(now + time::Duration::seconds(expires_in));
    credential.last_validated_at = Some(now);

    Ok(credential.clone())
}

fn is_reconnect_required_refresh_failure(status: u16, body: &str) -> bool {
    if !(400..500).contains(&status)
        || status == 408
        || status == 409
        || status == 425
        || status == 429
    {
        return false;
    }
    let Ok(value) = serde_json::from_str::<JsonValue>(body) else {
        return true;
    };
    let error = value
        .get("error")
        .and_then(|v| v.as_str())
        .unwrap_or_default();
    matches!(
        error,
        "invalid_grant" | "invalid_client" | "unauthorized_client" | "access_denied"
    )
}

fn string_from(value: &JsonValue, key: &str) -> Option<String> {
    value
        .get(key)
        .and_then(|v| v.as_str())
        .filter(|s| !s.is_empty())
        .map(str::to_owned)
}

// ── Tests ─────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use shared::models::{AuthType, ServiceProvider};

    fn credential(expires_at: Option<OffsetDateTime>) -> ServiceCredential {
        ServiceCredential {
            id: "cred_1".to_string(),
            source_id: "src_1".to_string(),
            user_id: Some("user_1".to_string()),
            provider: ServiceProvider::RemoteMcp,
            auth_type: AuthType::OAuth,
            principal_email: None,
            credentials: json!({"access_token":"old"}),
            config: json!({}),
            expires_at,
            last_validated_at: None,
            created_at: OffsetDateTime::now_utc(),
            updated_at: OffsetDateTime::now_utc(),
        }
    }

    // ── Predicate tests ──────────────────────────────────────────

    #[test]
    fn detects_expiring_credentials_with_skew() {
        assert!(!credential_needs_refresh(
            &credential(None),
            OffsetDateTime::now_utc()
        ));
        assert!(credential_needs_refresh(
            &credential(Some(
                OffsetDateTime::now_utc() + time::Duration::seconds(60)
            )),
            OffsetDateTime::now_utc(),
        ));
        assert!(!credential_needs_refresh(
            &credential(Some(OffsetDateTime::now_utc() + time::Duration::hours(1))),
            OffsetDateTime::now_utc(),
        ));
    }

    // ── Failure classification tests ─────────────────────────────

    #[test]
    fn refresh_failure_classification_distinguishes_reconnect_from_transient() {
        assert!(is_reconnect_required_refresh_failure(
            400,
            r#"{"error":"invalid_grant"}"#,
        ));
        assert!(is_reconnect_required_refresh_failure(
            401,
            r#"{"error":"invalid_client"}"#,
        ));
        assert!(!is_reconnect_required_refresh_failure(
            429,
            r#"{"error":"rate_limited"}"#,
        ));
        assert!(!is_reconnect_required_refresh_failure(
            500,
            r#"{"error":"server_error"}"#,
        ));
    }

    // ── Auth method resolution tests ─────────────────────────────

    #[test]
    fn auth_method_resolves_connector_config_explicit() {
        let cc = json!({"oauth_token_endpoint_auth_method": "client_secret_basic"});
        let cred = json!({});
        let oauth = RemoteMcpOAuthConfig {
            provider: "test".into(),
            credential_provider: String::new(),
            token_endpoint: "https://token.example.com".into(),
            token_endpoint_auth_method: TokenEndpointAuthMethod::ClientSecretPost,
            resource: None,
        };
        assert_eq!(
            token_method_from_params(&cc, &cred, &oauth),
            TokenEndpointAuthMethod::ClientSecretBasic
        );
    }

    #[test]
    fn auth_method_falls_back_to_credential_json() {
        let cc = json!({});
        let cred = json!({"token_endpoint_auth_method": "none"});
        let oauth = RemoteMcpOAuthConfig {
            provider: "test".into(),
            credential_provider: String::new(),
            token_endpoint: "https://token.example.com".into(),
            token_endpoint_auth_method: TokenEndpointAuthMethod::ClientSecretPost,
            resource: None,
        };
        assert_eq!(
            token_method_from_params(&cc, &cred, &oauth),
            TokenEndpointAuthMethod::None
        );
    }

    #[test]
    fn auth_method_uses_oauth_metadata_when_no_explicit_config_or_cred() {
        // No config, no credential JSON → falls through to oauth metadata
        // which is always honored before heuristic.
        let cc = json!({});
        let cred = json!({});
        let oauth = RemoteMcpOAuthConfig {
            provider: "test".into(),
            credential_provider: String::new(),
            token_endpoint: "https://token.example.com".into(),
            token_endpoint_auth_method: TokenEndpointAuthMethod::ClientSecretPost,
            resource: None,
        };
        // oauth says ClientSecretPost → use it even without a secret.
        assert_eq!(
            token_method_from_params(&cc, &cred, &oauth),
            TokenEndpointAuthMethod::ClientSecretPost
        );
    }

    #[test]
    fn auth_method_uses_oauth_metadata_when_heuristic_would_be_none() {
        // No config, no credential JSON, oauth says None → None.
        let cc = json!({});
        let cred = json!({});
        let oauth = RemoteMcpOAuthConfig {
            provider: "test".into(),
            credential_provider: String::new(),
            token_endpoint: "https://token.example.com".into(),
            token_endpoint_auth_method: TokenEndpointAuthMethod::None,
            resource: None,
        };
        assert_eq!(
            token_method_from_params(&cc, &cred, &oauth),
            TokenEndpointAuthMethod::None
        );
    }

    // ── native_oauth_is_refreshable tests ────────────────────────

    #[test]
    fn refreshable_when_all_fields_present() {
        let cc = json!({"oauth_client_id": "c1"});
        let cred = json!({
            "refresh_token": "rt",
            "token_uri": "https://tok.example.com"
        });
        assert!(native_oauth_is_refreshable(&cc, &cred));
    }

    #[test]
    fn refreshable_client_id_from_credential_fallback() {
        let cc = json!({});
        let cred = json!({
            "refresh_token": "rt",
            "client_id": "c1",
            "token_uri": "https://tok.example.com"
        });
        assert!(native_oauth_is_refreshable(&cc, &cred));
    }

    #[test]
    fn not_refreshable_when_missing_refresh_token() {
        let cc = json!({"oauth_client_id": "c1"});
        let cred = json!({
            "access_token": "tok",
            "token_uri": "https://tok.example.com"
        });
        assert!(!native_oauth_is_refreshable(&cc, &cred));
    }

    #[test]
    fn not_refreshable_when_missing_client_id() {
        let cc = json!({});
        let cred = json!({
            "refresh_token": "rt",
            "token_uri": "https://tok.example.com"
        });
        assert!(!native_oauth_is_refreshable(&cc, &cred));
    }

    #[test]
    fn not_refreshable_when_missing_token_uri() {
        let cc = json!({});
        let cred = json!({
            "refresh_token": "rt",
            "client_id": "c1"
        });
        assert!(!native_oauth_is_refreshable(&cc, &cred));
    }

    #[test]
    fn not_refreshable_when_empty_refresh_token() {
        let cc = json!({"oauth_client_id": "c1"});
        let cred = json!({
            "refresh_token": "",
            "token_uri": "https://tok.example.com"
        });
        // empty string should be treated as missing
        assert!(!native_oauth_is_refreshable(&cc, &cred));
    }

    // ── End-to-end HTTP refresh test ─────────────────────────────

    #[tokio::test]
    async fn refreshes_public_client_tokens_and_preserves_resource_binding() {
        use std::collections::HashMap;
        use std::sync::Arc;

        use axum::{
            extract::{Form, State},
            routing::post,
            Json, Router,
        };
        use tokio::sync::Mutex as TokioMutex;

        type CapturedForm = Arc<TokioMutex<Option<HashMap<String, String>>>>;

        async fn token_endpoint(
            State(captured): State<CapturedForm>,
            Form(form): Form<HashMap<String, String>>,
        ) -> Json<JsonValue> {
            *captured.lock().await = Some(form);
            Json(json!({
                "access_token": "access-new",
                "refresh_token": "refresh-new",
                "token_type": "Bearer",
                "expires_in": 120
            }))
        }

        let captured: CapturedForm = Arc::new(TokioMutex::new(None));
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let app = Router::new()
            .route("/token", post(token_endpoint))
            .with_state(captured.clone());
        let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });

        // This test reaches do_oauth_refresh via do_native_refresh with a
        // plain client to avoid the pinned-client loopback restriction.
        let resource = "https://windshift.example/mcp";
        let mut credential = ServiceCredential {
            id: "cred_1".to_string(),
            source_id: "src_1".to_string(),
            user_id: Some("user_1".to_string()),
            provider: ServiceProvider::RemoteMcp,
            auth_type: AuthType::OAuth,
            principal_email: None,
            credentials: json!({
                "access_token": "access-old",
                "refresh_token": "refresh-old",
                "client_id": "client-1",
                "token_uri": format!("http://{address}/token"),
                "token_endpoint_auth_method": "none",
                "resource": resource,
            }),
            config: json!({}),
            expires_at: Some(OffsetDateTime::now_utc()),
            last_validated_at: None,
            created_at: OffsetDateTime::now_utc(),
            updated_at: OffsetDateTime::now_utc(),
        };

        let native_params = NativeOAuthParams {
            provider: "remote_mcp:test".to_string(),
            credential_provider: String::new(),
            token_endpoint: format!("http://{address}/token"),
            token_endpoint_auth_method: TokenEndpointAuthMethod::None,
            resource: Some(resource.to_string()),
        };

        let result = do_native_refresh(
            &mut credential,
            &json!({}),
            &native_params,
            "refresh-old",
            Some(reqwest::Client::new()),
        )
        .await
        .unwrap();

        server.abort();

        assert_eq!(result.credentials["access_token"], "access-new");
        assert_eq!(result.credentials["refresh_token"], "refresh-new");
        assert_eq!(result.credentials["token_type"], "Bearer");
        assert_eq!(
            result.credentials["token_uri"],
            format!("http://{address}/token")
        );
        assert!(
            result.expires_at.unwrap() > OffsetDateTime::now_utc() + time::Duration::seconds(100)
        );
        assert!(result.last_validated_at.is_some());

        let form = captured.lock().await.clone().unwrap();
        assert_eq!(
            form.get("grant_type").map(String::as_str),
            Some("refresh_token")
        );
        assert_eq!(
            form.get("refresh_token").map(String::as_str),
            Some("refresh-old")
        );
        assert_eq!(form.get("client_id").map(String::as_str), Some("client-1"));
        assert_eq!(
            form.get("resource").map(String::as_str),
            Some("https://windshift.example/mcp")
        );
        assert!(!form.contains_key("client_secret"));
    }

    #[tokio::test]
    async fn applies_default_expires_in_when_not_returned() {
        use std::collections::HashMap;
        use std::sync::Arc;

        use axum::{
            extract::{Form, State},
            routing::post,
            Json, Router,
        };
        use tokio::sync::Mutex as TokioMutex;

        type CapturedForm = Arc<TokioMutex<Option<HashMap<String, String>>>>;

        async fn token_endpoint(
            State(captured): State<CapturedForm>,
            Form(form): Form<HashMap<String, String>>,
        ) -> Json<JsonValue> {
            *captured.lock().await = Some(form);
            Json(json!({
                "access_token": "access-new",
                "refresh_token": "refresh-new",
                "token_type": "Bearer",
                // expires_in intentionally absent
            }))
        }

        let captured: CapturedForm = Arc::new(TokioMutex::new(None));
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let app = Router::new()
            .route("/token", post(token_endpoint))
            .with_state(captured.clone());
        let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });

        let mut credential = ServiceCredential {
            id: "cred_1".to_string(),
            source_id: "src_1".to_string(),
            user_id: Some("user_1".to_string()),
            provider: ServiceProvider::RemoteMcp,
            auth_type: AuthType::OAuth,
            principal_email: None,
            credentials: json!({
                "access_token": "access-old",
                "refresh_token": "refresh-old",
                "client_id": "client-1",
                "token_uri": format!("http://{address}/token"),
                "token_endpoint_auth_method": "none",
            }),
            config: json!({}),
            expires_at: Some(OffsetDateTime::now_utc()),
            last_validated_at: None,
            created_at: OffsetDateTime::now_utc(),
            updated_at: OffsetDateTime::now_utc(),
        };

        let p = NativeOAuthParams {
            provider: "test".into(),
            credential_provider: String::new(),
            token_endpoint: format!("http://{address}/token"),
            token_endpoint_auth_method: TokenEndpointAuthMethod::None,
            resource: None,
        };

        let result = do_native_refresh(
            &mut credential,
            &json!({}),
            &p,
            "refresh-old",
            Some(reqwest::Client::new()),
        )
        .await
        .unwrap();

        server.abort();

        // Default expires_in is 3600 seconds.
        let expected = OffsetDateTime::now_utc() + time::Duration::seconds(3590); // 10s skew
        assert!(
            result.expires_at.unwrap() > expected,
            "expires_at should be > now + 3590s (default 3600 with some tolerance)"
        );
    }
}

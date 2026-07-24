use crate::remote_mcp::gateway::{
    pinned_http_client_for_url, read_limited_response_text, validate_endpoint_for_gateway,
};
use reqwest::Client;
use serde::Deserialize;
use serde_json::Value as JsonValue;
use shared::db::repositories::{ConnectorConfigRepository, ServiceCredentialsRepo};
use shared::models::{ServiceCredential, Source};
use shared::DatabasePool;
use std::time::Duration;
use thiserror::Error;
use time::OffsetDateTime;

const REFRESH_SKEW_SECONDS: i64 = 300;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TokenEndpointAuthMethod {
    ClientSecretPost,
    ClientSecretBasic,
    None,
}

impl TokenEndpointAuthMethod {
    fn parse(value: Option<&str>) -> Self {
        match value {
            Some("client_secret_basic") => Self::ClientSecretBasic,
            Some("none") => Self::None,
            _ => Self::ClientSecretPost,
        }
    }
}

#[derive(Debug, Clone)]
pub struct RemoteMcpOAuthConfig {
    pub provider: String,
    pub credential_provider: String,
    pub token_endpoint: String,
    pub token_endpoint_auth_method: TokenEndpointAuthMethod,
    pub resource: Option<String>,
}

#[derive(Debug, Error)]
pub enum OAuthError {
    #[error("missing OAuth field: {0}")]
    MissingField(&'static str),
    #[error("credential requires OAuth reconnect")]
    ReconnectRequired,
    #[error("OAuth refresh failed: {0}")]
    RefreshFailed(String),
    #[error("repository error: {0}")]
    Repository(String),
}

pub fn parse_oauth_config(value: &JsonValue) -> Result<RemoteMcpOAuthConfig, OAuthError> {
    let provider = value
        .get("provider")
        .and_then(|v| v.as_str())
        .ok_or(OAuthError::MissingField("provider"))?
        .to_string();
    let token_endpoint = value
        .get("token_endpoint")
        .and_then(|v| v.as_str())
        .ok_or(OAuthError::MissingField("token_endpoint"))?
        .to_string();
    let credential_provider = value
        .get("credential_provider")
        .and_then(|v| v.as_str())
        .unwrap_or("remote_mcp")
        .to_string();
    let token_endpoint_auth_method = TokenEndpointAuthMethod::parse(
        value
            .get("token_endpoint_auth_method")
            .and_then(|v| v.as_str()),
    );
    let resource = value
        .get("resource")
        .and_then(|v| v.as_str())
        .map(str::to_owned);

    Ok(RemoteMcpOAuthConfig {
        provider,
        credential_provider,
        token_endpoint,
        token_endpoint_auth_method,
        resource,
    })
}

pub fn credential_needs_refresh(credential: &ServiceCredential, now: OffsetDateTime) -> bool {
    match credential.expires_at {
        Some(expires_at) => expires_at <= now + time::Duration::seconds(REFRESH_SKEW_SECONDS),
        None => false,
    }
}

#[derive(Debug, Deserialize)]
struct RefreshResponse {
    access_token: String,
    refresh_token: Option<String>,
    token_type: Option<String>,
    expires_in: Option<i64>,
}

pub async fn usable_oauth_credential(
    db_pool: &DatabasePool,
    _http_client: &Client,
    source: &Source,
    mut credential: ServiceCredential,
    oauth: &RemoteMcpOAuthConfig,
) -> Result<ServiceCredential, OAuthError> {
    if !credential_needs_refresh(&credential, OffsetDateTime::now_utc()) {
        return Ok(credential);
    }

    let refresh_token = credential
        .credentials
        .get("refresh_token")
        .and_then(|v| v.as_str())
        .filter(|v| !v.is_empty())
        .ok_or(OAuthError::ReconnectRequired)?
        .to_string();

    let config_repo = ConnectorConfigRepository::new(db_pool.pool().clone());
    let connector_config = config_repo
        .get_by_provider(&oauth.provider)
        .await
        .map_err(|e| OAuthError::Repository(e.to_string()))?
        .map(|row| row.config)
        .unwrap_or_else(|| serde_json::json!({}));

    let client_id = string_from(&connector_config, "oauth_client_id")
        .or_else(|| string_from(&credential.credentials, "client_id"))
        .ok_or(OAuthError::MissingField("oauth_client_id"))?;
    let client_secret = string_from(&connector_config, "oauth_client_secret")
        .or_else(|| string_from(&credential.credentials, "client_secret"));
    let token_endpoint = string_from(&connector_config, "oauth_token_endpoint")
        .or_else(|| string_from(&credential.credentials, "token_uri"))
        .unwrap_or_else(|| oauth.token_endpoint.clone());
    let token_method = TokenEndpointAuthMethod::parse(
        string_from(&connector_config, "oauth_token_endpoint_auth_method").as_deref(),
    );
    let token_method = if matches!(token_method, TokenEndpointAuthMethod::ClientSecretPost)
        && connector_config
            .get("oauth_token_endpoint_auth_method")
            .is_none()
    {
        oauth.token_endpoint_auth_method.clone()
    } else {
        token_method
    };

    let mut params = vec![
        ("grant_type".to_string(), "refresh_token".to_string()),
        ("refresh_token".to_string(), refresh_token),
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
            let secret = client_secret.ok_or(OAuthError::MissingField("oauth_client_secret"))?;
            basic_auth = Some((client_id, secret));
        }
    }
    if let Some(resource) = &oauth.resource {
        validate_endpoint_for_gateway(resource)
            .await
            .map_err(|e| OAuthError::RefreshFailed(e.to_string()))?;
        params.push(("resource".to_string(), resource.clone()));
    }

    let pinned_client = pinned_http_client_for_url(&token_endpoint)
        .await
        .map_err(|e| OAuthError::RefreshFailed(e.to_string()))?;
    let mut request = pinned_client
        .post(&token_endpoint)
        .timeout(Duration::from_secs(20))
        .header("accept", "application/json");
    if let Some((client_id, client_secret)) = basic_auth {
        request = request.basic_auth(client_id, Some(client_secret));
    }

    let response = request
        .form(&params)
        .send()
        .await
        .map_err(|e| OAuthError::RefreshFailed(e.to_string()))?;
    let status = response.status();
    let body = read_limited_response_text(response)
        .await
        .map_err(|e| OAuthError::RefreshFailed(e.to_string()))?;
    if !status.is_success() {
        if is_reconnect_required_refresh_failure(status.as_u16(), &body) {
            return Err(OAuthError::ReconnectRequired);
        }
        return Err(OAuthError::RefreshFailed(format!(
            "token endpoint returned HTTP {status}"
        )));
    }
    let refreshed: RefreshResponse =
        serde_json::from_str(&body).map_err(|e| OAuthError::RefreshFailed(e.to_string()))?;

    credential.credentials["access_token"] = JsonValue::String(refreshed.access_token);
    if let Some(refresh_token) = refreshed.refresh_token {
        credential.credentials["refresh_token"] = JsonValue::String(refresh_token);
    }
    credential.credentials["token_type"] =
        JsonValue::String(refreshed.token_type.unwrap_or_else(|| "Bearer".to_string()));
    credential.credentials["token_uri"] = JsonValue::String(token_endpoint);
    if let Some(expires_in) = refreshed.expires_in {
        credential.expires_at =
            Some(OffsetDateTime::now_utc() + time::Duration::seconds(expires_in));
    }

    let repo = ServiceCredentialsRepo::new(db_pool.pool().clone())
        .map_err(|e| OAuthError::Repository(e.to_string()))?;
    repo.update_credentials(&credential)
        .await
        .map_err(|e| OAuthError::Repository(e.to_string()))?;

    tracing::debug!(source_id = %source.id, provider = %oauth.provider, credential_provider = %oauth.credential_provider, "refreshed remote MCP OAuth credential");
    Ok(credential)
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
    value.get(key).and_then(|v| v.as_str()).map(str::to_owned)
}

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

    #[test]
    fn parses_normalized_remote_mcp_oauth_config() {
        let cfg = parse_oauth_config(&json!({
            "provider": "remote_mcp:acme",
            "credential_provider": "remote_mcp",
            "token_endpoint": "https://auth.example.com/token",
            "token_endpoint_auth_method": "none",
            "resource": "https://mcp.example.com/mcp"
        }))
        .unwrap();
        assert_eq!(cfg.provider, "remote_mcp:acme");
        assert_eq!(cfg.credential_provider, "remote_mcp");
        assert_eq!(
            cfg.token_endpoint_auth_method,
            TokenEndpointAuthMethod::None
        );
    }

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
}

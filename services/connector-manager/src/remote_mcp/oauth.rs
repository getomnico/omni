use serde_json::Value as JsonValue;
use thiserror::Error;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TokenEndpointAuthMethod {
    ClientSecretPost,
    ClientSecretBasic,
    None,
}

impl TokenEndpointAuthMethod {
    pub(crate) fn parse(value: Option<&str>) -> Self {
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
}

/// Parse `RemoteMcpOAuthConfig` from a JSON value returned by the MCP
/// endpoint's OAuth metadata discovery.
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

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

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
}

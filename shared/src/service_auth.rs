use anyhow::{anyhow, Result};
use async_trait::async_trait;
use base64::{engine::general_purpose, Engine};
use chrono::{Duration, Utc};
use jsonwebtoken::{encode, Algorithm, EncodingKey, Header};
use serde::{Deserialize, Serialize};

use crate::models::{AuthType, ServiceCredentials, ServiceProvider};

/// Trait for service authentication
#[async_trait]
pub trait ServiceAuth: Send + Sync {
    /// Get the authentication header value
    async fn get_auth_header(&self) -> Result<String>;

    /// Check if credentials are expired
    fn is_expired(&self) -> bool;

    /// Validate credentials
    async fn validate(&self) -> Result<bool>;
}

/// Google Service Account authentication
pub struct GoogleServiceAuth {
    service_account_email: String,
    private_key: String,
    scopes: Vec<String>,
    delegated_user: Option<String>,
}

#[derive(Debug, Serialize, Deserialize)]
struct GoogleServiceAccountKey {
    #[serde(rename = "type")]
    key_type: String,
    project_id: String,
    private_key_id: String,
    private_key: String,
    client_email: String,
    client_id: String,
    auth_uri: String,
    token_uri: String,
    auth_provider_x509_cert_url: String,
    client_x509_cert_url: String,
}

#[derive(Debug, Serialize)]
struct GoogleJwtClaims {
    iss: String,         // Issuer (service account email)
    sub: Option<String>, // Subject (delegated user email)
    scope: String,       // Space-separated scopes
    aud: String,         // Audience (token URI)
    exp: i64,            // Expiration time
    iat: i64,            // Issued at time
}

impl GoogleServiceAuth {
    pub fn from_service_account_json(
        json: &str,
        scopes: Vec<String>,
        delegated_user: Option<String>,
    ) -> Result<Self> {
        let key: GoogleServiceAccountKey = serde_json::from_str(json)?;

        if key.key_type != "service_account" {
            return Err(anyhow!(
                "Invalid key type: expected 'service_account', got '{}'",
                key.key_type
            ));
        }

        Ok(Self {
            service_account_email: key.client_email,
            private_key: key.private_key,
            scopes,
            delegated_user,
        })
    }

    pub fn from_credentials(creds: &ServiceCredentials) -> Result<Self> {
        let service_account_json = creds
            .credentials
            .get("service_account_key")
            .and_then(|v| v.as_str())
            .ok_or_else(|| anyhow!("Missing service_account_key in credentials"))?;

        let scopes = creds
            .config
            .get("scopes")
            .and_then(|v| v.as_array())
            .map(|arr| {
                arr.iter()
                    .filter_map(|v| v.as_str().map(String::from))
                    .collect()
            })
            .unwrap_or_else(|| vec!["https://www.googleapis.com/auth/drive.readonly".to_string()]);

        let delegated_user = creds
            .config
            .get("delegated_user")
            .and_then(|v| v.as_str())
            .map(String::from);

        Self::from_service_account_json(service_account_json, scopes, delegated_user)
    }

    async fn get_access_token(&self) -> Result<String> {
        let now = Utc::now();
        let exp = now + Duration::hours(1);

        let claims = GoogleJwtClaims {
            iss: self.service_account_email.clone(),
            sub: self.delegated_user.clone(),
            scope: self.scopes.join(" "),
            aud: "https://oauth2.googleapis.com/token".to_string(),
            exp: exp.timestamp(),
            iat: now.timestamp(),
        };

        let header = Header::new(Algorithm::RS256);
        let key = EncodingKey::from_rsa_pem(self.private_key.as_bytes())?;
        let jwt = encode(&header, &claims, &key)?;

        // Exchange JWT for access token
        let client = reqwest::Client::new();
        let response = client
            .post("https://oauth2.googleapis.com/token")
            .form(&[
                ("grant_type", "urn:ietf:params:oauth:grant-type:jwt-bearer"),
                ("assertion", &jwt),
            ])
            .send()
            .await?;

        if !response.status().is_success() {
            let error_text = response.text().await?;
            return Err(anyhow!("Failed to get access token: {}", error_text));
        }

        #[derive(Deserialize)]
        struct TokenResponse {
            access_token: String,
        }

        let token_response: TokenResponse = response.json().await?;
        Ok(token_response.access_token)
    }
}

#[async_trait]
impl ServiceAuth for GoogleServiceAuth {
    async fn get_auth_header(&self) -> Result<String> {
        let token = self.get_access_token().await?;
        Ok(format!("Bearer {}", token))
    }

    fn is_expired(&self) -> bool {
        // JWT tokens are self-contained and handle their own expiration
        false
    }

    async fn validate(&self) -> Result<bool> {
        // Try to get an access token to validate the credentials
        match self.get_access_token().await {
            Ok(_) => Ok(true),
            Err(_) => Ok(false),
        }
    }
}

/// API Key authentication (for Atlassian, etc.)
pub struct ApiKeyAuth {
    username: String,
    api_key: String,
}

impl ApiKeyAuth {
    pub fn new(username: String, api_key: String) -> Self {
        Self { username, api_key }
    }

    pub fn from_credentials(creds: &ServiceCredentials) -> Result<Self> {
        let username = creds
            .principal_email
            .as_ref()
            .ok_or_else(|| anyhow!("Missing principal_email for API key auth"))?
            .clone();

        let api_key = creds
            .credentials
            .get("api_key")
            .and_then(|v| v.as_str())
            .ok_or_else(|| anyhow!("Missing api_key in credentials"))?
            .to_string();

        Ok(Self::new(username, api_key))
    }
}

#[async_trait]
impl ServiceAuth for ApiKeyAuth {
    async fn get_auth_header(&self) -> Result<String> {
        let auth_string = format!("{}:{}", self.username, self.api_key);
        let encoded = general_purpose::STANDARD.encode(auth_string.as_bytes());
        Ok(format!("Basic {}", encoded))
    }

    fn is_expired(&self) -> bool {
        // API keys typically don't expire
        false
    }

    async fn validate(&self) -> Result<bool> {
        // API key validation would require making a test API call
        // For now, just check that the key is not empty
        Ok(!self.api_key.is_empty())
    }
}

/// Bot Token authentication (for Slack)
pub struct BotTokenAuth {
    bot_token: String,
}

impl BotTokenAuth {
    pub fn new(bot_token: String) -> Self {
        Self { bot_token }
    }

    pub fn from_credentials(creds: &ServiceCredentials) -> Result<Self> {
        let bot_token = creds
            .credentials
            .get("bot_token")
            .and_then(|v| v.as_str())
            .ok_or_else(|| anyhow!("Missing bot_token in credentials"))?
            .to_string();

        Ok(Self::new(bot_token))
    }
}

#[async_trait]
impl ServiceAuth for BotTokenAuth {
    async fn get_auth_header(&self) -> Result<String> {
        Ok(format!("Bearer {}", self.bot_token))
    }

    fn is_expired(&self) -> bool {
        // Bot tokens typically don't expire
        false
    }

    async fn validate(&self) -> Result<bool> {
        // Bot token validation would require making a test API call
        // For now, just check that the token starts with the expected prefix
        Ok(self.bot_token.starts_with("xoxb-"))
    }
}

/// Factory function to create appropriate auth implementation
pub fn create_service_auth(creds: &ServiceCredentials) -> Result<Box<dyn ServiceAuth>> {
    match (creds.provider, creds.auth_type) {
        (ServiceProvider::Google, AuthType::Jwt) => {
            Ok(Box::new(GoogleServiceAuth::from_credentials(creds)?))
        }
        (ServiceProvider::Atlassian, AuthType::ApiKey) => {
            Ok(Box::new(ApiKeyAuth::from_credentials(creds)?))
        }
        (ServiceProvider::Slack, AuthType::BotToken) => {
            Ok(Box::new(BotTokenAuth::from_credentials(creds)?))
        }
        _ => Err(anyhow!(
            "Unsupported auth combination: {:?} with {:?}",
            creds.provider,
            creds.auth_type
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use sqlx::types::time::OffsetDateTime;

    fn make_credentials(
        provider: ServiceProvider,
        auth_type: AuthType,
        principal_email: Option<&str>,
        credentials: serde_json::Value,
        config: serde_json::Value,
    ) -> ServiceCredentials {
        ServiceCredentials {
            id: "test-id".to_string(),
            source_id: "test-source".to_string(),
            user_id: None,
            provider,
            auth_type,
            principal_email: principal_email.map(String::from),
            credentials,
            config,
            expires_at: None,
            last_validated_at: None,
            created_at: OffsetDateTime::now_utc(),
            updated_at: OffsetDateTime::now_utc(),
        }
    }

    #[tokio::test]
    async fn test_api_key_auth_header_format() {
        let auth = ApiKeyAuth::new("user@example.com".to_string(), "my-api-key".to_string());
        let header = auth.get_auth_header().await.unwrap();

        let expected_b64 = general_purpose::STANDARD.encode("user@example.com:my-api-key");
        assert_eq!(header, format!("Basic {}", expected_b64));
    }

    #[tokio::test]
    async fn test_api_key_auth_validate_nonempty() {
        let auth = ApiKeyAuth::new("user".to_string(), "key123".to_string());
        assert!(auth.validate().await.unwrap());
    }

    #[tokio::test]
    async fn test_api_key_auth_validate_empty_key() {
        let auth = ApiKeyAuth::new("user".to_string(), "".to_string());
        assert!(!auth.validate().await.unwrap());
    }

    #[test]
    fn test_api_key_auth_from_credentials_valid() {
        let creds = make_credentials(
            ServiceProvider::Atlassian,
            AuthType::ApiKey,
            Some("admin@corp.com"),
            json!({"api_key": "secret123"}),
            json!({}),
        );
        let auth = ApiKeyAuth::from_credentials(&creds);
        assert!(auth.is_ok());
    }

    #[test]
    fn test_api_key_auth_from_credentials_missing_email() {
        let creds = make_credentials(
            ServiceProvider::Atlassian,
            AuthType::ApiKey,
            None,
            json!({"api_key": "secret123"}),
            json!({}),
        );
        assert!(ApiKeyAuth::from_credentials(&creds).is_err());
    }

    #[test]
    fn test_api_key_auth_from_credentials_missing_key() {
        let creds = make_credentials(
            ServiceProvider::Atlassian,
            AuthType::ApiKey,
            Some("admin@corp.com"),
            json!({}),
            json!({}),
        );
        assert!(ApiKeyAuth::from_credentials(&creds).is_err());
    }

    #[tokio::test]
    async fn test_bot_token_auth_header_format() {
        let auth = BotTokenAuth::new("xoxb-test-token".to_string());
        let header = auth.get_auth_header().await.unwrap();
        assert_eq!(header, "Bearer xoxb-test-token");
    }

    #[tokio::test]
    async fn test_bot_token_validate_valid_prefix() {
        let auth = BotTokenAuth::new("xoxb-123456".to_string());
        assert!(auth.validate().await.unwrap());
    }

    #[tokio::test]
    async fn test_bot_token_validate_invalid_prefix() {
        let auth = BotTokenAuth::new("not-a-bot-token".to_string());
        assert!(!auth.validate().await.unwrap());
    }

    #[test]
    fn test_bot_token_from_credentials_valid() {
        let creds = make_credentials(
            ServiceProvider::Slack,
            AuthType::BotToken,
            None,
            json!({"bot_token": "xoxb-slack-token"}),
            json!({}),
        );
        assert!(BotTokenAuth::from_credentials(&creds).is_ok());
    }

    #[test]
    fn test_bot_token_from_credentials_missing_token() {
        let creds = make_credentials(
            ServiceProvider::Slack,
            AuthType::BotToken,
            None,
            json!({}),
            json!({}),
        );
        assert!(BotTokenAuth::from_credentials(&creds).is_err());
    }

    #[test]
    fn test_create_service_auth_atlassian_api_key() {
        let creds = make_credentials(
            ServiceProvider::Atlassian,
            AuthType::ApiKey,
            Some("admin@corp.com"),
            json!({"api_key": "key123"}),
            json!({}),
        );
        assert!(create_service_auth(&creds).is_ok());
    }

    #[test]
    fn test_create_service_auth_slack_bot_token() {
        let creds = make_credentials(
            ServiceProvider::Slack,
            AuthType::BotToken,
            None,
            json!({"bot_token": "xoxb-token"}),
            json!({}),
        );
        assert!(create_service_auth(&creds).is_ok());
    }

    #[test]
    fn test_create_service_auth_unsupported_combination() {
        let creds = make_credentials(
            ServiceProvider::Slack,
            AuthType::ApiKey,
            None,
            json!({}),
            json!({}),
        );
        assert!(create_service_auth(&creds).is_err());
    }
}

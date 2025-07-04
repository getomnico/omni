use anyhow::{anyhow, Result};
use async_trait::async_trait;
use base64::{engine::general_purpose, Engine};
use chrono::{Duration, Utc};
use jsonwebtoken::{encode, Algorithm, EncodingKey, Header};
use serde::{Deserialize, Serialize};
use serde_json::Value as JsonValue;
use sqlx::PgPool;

use crate::encryption::{EncryptedData, EncryptionService};
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

/// Service credentials repository with encryption support
pub struct ServiceCredentialsRepo {
    pool: PgPool,
    encryption_service: EncryptionService,
}

impl ServiceCredentialsRepo {
    pub fn new(pool: PgPool) -> Result<Self> {
        let encryption_service = EncryptionService::new()?;
        Ok(Self {
            pool,
            encryption_service,
        })
    }

    pub async fn get_by_source_id(&self, source_id: &str) -> Result<Option<ServiceCredentials>> {
        let mut creds = sqlx::query_as::<_, ServiceCredentials>(
            "SELECT * FROM service_credentials WHERE source_id = $1",
        )
        .bind(source_id)
        .fetch_optional(&self.pool)
        .await?;

        if let Some(ref mut creds) = creds {
            self.decrypt_credentials_in_place(creds)?;
        }

        Ok(creds)
    }

    /// Decrypt credentials in place if they are encrypted
    fn decrypt_credentials_in_place(&self, creds: &mut ServiceCredentials) -> Result<()> {
        // Check if credentials are already encrypted (new format)
        if let Some(encrypted_data) = creds.credentials.get("encrypted_data") {
            let encrypted_data: EncryptedData = serde_json::from_value(encrypted_data.clone())?;
            let decrypted_credentials: JsonValue =
                self.encryption_service.decrypt_json(&encrypted_data)?;
            creds.credentials = decrypted_credentials;
        }
        // If no encrypted_data field, credentials are in legacy unencrypted format - leave as is
        Ok(())
    }

    /// Encrypt credentials from application format to database format
    fn encrypt_credentials(&self, creds: &ServiceCredentials) -> Result<JsonValue> {
        let encrypted_data = self.encryption_service.encrypt_json(&creds.credentials)?;
        Ok(serde_json::json!({
            "encrypted_data": encrypted_data,
            "version": 1
        }))
    }

    pub async fn create(&self, creds: ServiceCredentials) -> Result<ServiceCredentials> {
        let encrypted_credentials = self.encrypt_credentials(&creds)?;

        let mut created_creds = sqlx::query_as::<_, ServiceCredentials>(
            r#"
            INSERT INTO service_credentials 
            (id, source_id, provider, auth_type, principal_email, credentials, config, expires_at, last_validated_at)
            VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9)
            RETURNING *
            "#
        )
        .bind(&creds.id)
        .bind(&creds.source_id)
        .bind(&creds.provider)
        .bind(&creds.auth_type)
        .bind(&creds.principal_email)
        .bind(&encrypted_credentials)
        .bind(&creds.config)
        .bind(&creds.expires_at)
        .bind(&creds.last_validated_at)
        .fetch_one(&self.pool)
        .await?;

        // Decrypt the credentials for return (they come back encrypted from the database)
        self.decrypt_credentials_in_place(&mut created_creds)?;
        Ok(created_creds)
    }

    pub async fn update_last_validated(&self, id: &str) -> Result<()> {
        sqlx::query(
            "UPDATE service_credentials SET last_validated_at = CURRENT_TIMESTAMP WHERE id = $1",
        )
        .bind(id)
        .execute(&self.pool)
        .await?;

        Ok(())
    }

    pub async fn delete_by_source_id(&self, source_id: &str) -> Result<()> {
        sqlx::query("DELETE FROM service_credentials WHERE source_id = $1")
            .bind(source_id)
            .execute(&self.pool)
            .await?;

        Ok(())
    }

    /// Update credentials (encrypts the new credentials)
    pub async fn update_credentials(&self, creds: &ServiceCredentials) -> Result<()> {
        let encrypted_credentials = self.encrypt_credentials(creds)?;

        sqlx::query(
            r#"
            UPDATE service_credentials 
            SET credentials = $2, config = $3, expires_at = $4, updated_at = CURRENT_TIMESTAMP
            WHERE id = $1
            "#,
        )
        .bind(&creds.id)
        .bind(&encrypted_credentials)
        .bind(&creds.config)
        .bind(&creds.expires_at)
        .execute(&self.pool)
        .await?;

        Ok(())
    }

    /// Encrypt all existing unencrypted credentials in the database
    pub async fn encrypt_existing_credentials(&self) -> Result<usize> {
        let mut count = 0;

        // Get all credentials that are not encrypted (don't have encrypted_data field)
        let unencrypted_creds = sqlx::query_as::<_, ServiceCredentials>(
            "SELECT * FROM service_credentials WHERE NOT (credentials ? 'encrypted_data')",
        )
        .fetch_all(&self.pool)
        .await?;

        for creds in unencrypted_creds {
            // These credentials are in unencrypted format, encrypt and update them
            self.update_credentials(&creds).await?;
            count += 1;
        }

        Ok(count)
    }
}

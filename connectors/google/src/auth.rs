use anyhow::{anyhow, Result};
use chrono::{Duration, Utc};
use jsonwebtoken::{encode, Algorithm, EncodingKey, Header};
use reqwest::Client;
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use tokio::sync::RwLock;
use tracing::{debug, info};

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct GoogleServiceAccountKey {
    #[serde(rename = "type")]
    pub key_type: String,
    pub project_id: String,
    pub private_key_id: String,
    pub private_key: String,
    pub client_email: String,
    pub client_id: String,
    pub auth_uri: String,
    pub token_uri: String,
    pub auth_provider_x509_cert_url: String,
    pub client_x509_cert_url: String,
}

#[derive(Debug, Serialize)]
struct GoogleJwtClaims {
    iss: String,
    sub: Option<String>,
    scope: String,
    aud: String,
    exp: i64,
    iat: i64,
}

#[derive(Debug, Clone)]
struct CachedToken {
    access_token: String,
    expires_at: i64,
}

pub struct ServiceAccountAuth {
    service_account: GoogleServiceAccountKey,
    scopes: Vec<String>,
    delegated_user: Option<String>,
    client: Client,
    token_cache: Arc<RwLock<Option<CachedToken>>>,
}

impl ServiceAccountAuth {
    pub fn new(
        service_account_json: &str,
        scopes: Vec<String>,
        delegated_user: Option<String>,
    ) -> Result<Self> {
        let service_account: GoogleServiceAccountKey = serde_json::from_str(service_account_json)?;

        if service_account.key_type != "service_account" {
            return Err(anyhow!(
                "Invalid key type: expected 'service_account', got '{}'",
                service_account.key_type
            ));
        }

        Ok(Self {
            service_account,
            scopes,
            delegated_user,
            client: Client::new(),
            token_cache: Arc::new(RwLock::new(None)),
        })
    }

    pub async fn get_access_token(&self) -> Result<String> {
        // Check cache first
        {
            let cache = self.token_cache.read().await;
            if let Some(cached) = &*cache {
                let now = Utc::now().timestamp();
                if cached.expires_at > now + 300 {
                    debug!("Using cached access token");
                    return Ok(cached.access_token.clone());
                }
            }
        }

        info!("Generating new access token for service account");

        let now = Utc::now();
        let exp = now + Duration::hours(1);

        let claims = GoogleJwtClaims {
            iss: self.service_account.client_email.clone(),
            sub: self.delegated_user.clone(),
            scope: self.scopes.join(" "),
            aud: self.service_account.token_uri.clone(),
            exp: exp.timestamp(),
            iat: now.timestamp(),
        };

        let header = Header::new(Algorithm::RS256);
        let key = EncodingKey::from_rsa_pem(self.service_account.private_key.as_bytes())?;
        let jwt = encode(&header, &claims, &key)?;

        // Exchange JWT for access token
        let params = [
            ("grant_type", "urn:ietf:params:oauth:grant-type:jwt-bearer"),
            ("assertion", &jwt),
        ];

        let response = self
            .client
            .post(&self.service_account.token_uri)
            .form(&params)
            .send()
            .await?;

        if !response.status().is_success() {
            let error_text = response.text().await?;
            return Err(anyhow!("Failed to get access token: {}", error_text));
        }

        #[derive(Deserialize)]
        struct TokenResponse {
            access_token: String,
            expires_in: i64,
        }

        let token_response: TokenResponse = response.json().await?;

        // Cache the token
        {
            let mut cache = self.token_cache.write().await;
            *cache = Some(CachedToken {
                access_token: token_response.access_token.clone(),
                expires_at: now.timestamp() + token_response.expires_in,
            });
        }

        Ok(token_response.access_token)
    }

    pub async fn validate(&self) -> Result<()> {
        // Try to get an access token to validate the service account
        self.get_access_token().await?;
        Ok(())
    }
}

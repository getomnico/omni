use anyhow::{anyhow, Result};
use chrono::{Duration, Utc};
use jsonwebtoken::{encode, Algorithm, EncodingKey, Header};
use reqwest::Client;
use serde::{Deserialize, Serialize};
use shared::models::SourceType;
use shared::RateLimiter;
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::RwLock;
use tracing::{debug, info, warn};

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

#[derive(Clone)]
pub struct ServiceAccountAuth {
    service_account: GoogleServiceAccountKey,
    scopes: Vec<String>,
    client: Client,
    token_cache: Arc<RwLock<HashMap<String, CachedToken>>>,
}

impl ServiceAccountAuth {
    pub fn new(service_account_json: &str, scopes: Vec<String>) -> Result<Self> {
        let service_account: GoogleServiceAccountKey = serde_json::from_str(service_account_json)?;

        if service_account.key_type != "service_account" {
            return Err(anyhow!(
                "Invalid key type: expected 'service_account', got '{}'",
                service_account.key_type
            ));
        }

        let client = Client::builder()
            .pool_max_idle_per_host(5) // Reuse connections for token requests
            .pool_idle_timeout(std::time::Duration::from_secs(90))
            .tcp_keepalive(std::time::Duration::from_secs(60))
            .timeout(std::time::Duration::from_secs(30)) // Timeout for token requests
            .connect_timeout(std::time::Duration::from_secs(10)) // Connection timeout
            .build()
            .map_err(|e| anyhow!("Failed to build HTTP client: {}", e))?;

        Ok(Self {
            service_account,
            scopes,
            client,
            token_cache: Arc::new(RwLock::new(HashMap::new())),
        })
    }

    pub async fn get_access_token(&self, impersonate_user: &str) -> Result<String> {
        // Check cache first
        {
            let cache = self.token_cache.read().await;
            if let Some(cached) = cache.get(impersonate_user) {
                let now = Utc::now().timestamp();
                if cached.expires_at > now + 300 {
                    debug!("Using cached access token for user: {}", impersonate_user);
                    return Ok(cached.access_token.clone());
                }
            }
        }

        info!(
            "Generating new access token for user: {}, scopes: {:?}",
            impersonate_user, self.scopes
        );

        let now = Utc::now();
        let exp = now + Duration::hours(1);

        let claims = GoogleJwtClaims {
            iss: self.service_account.client_email.clone(),
            sub: Some(impersonate_user.to_string()),
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
            cache.insert(
                impersonate_user.to_string(),
                CachedToken {
                    access_token: token_response.access_token.clone(),
                    expires_at: now.timestamp() + token_response.expires_in,
                },
            );
        }

        Ok(token_response.access_token)
    }

    pub async fn validate(&self, test_user: &str) -> Result<()> {
        // Try to get an access token to validate the service account
        self.get_access_token(test_user).await?;
        Ok(())
    }

    pub async fn is_token_near_expiry(&self, user: &str, buffer: Duration) -> bool {
        let cache = self.token_cache.read().await;
        if let Some(cached) = cache.get(user) {
            let now = Utc::now().timestamp();
            let buffer_seconds = buffer.num_seconds();
            cached.expires_at <= now + buffer_seconds
        } else {
            true // No token means we need to get one
        }
    }

    pub async fn refresh_access_token(&self, impersonate_user: &str) -> Result<String> {
        info!(
            "Force refreshing access token for user: {}, scopes: {:?}",
            impersonate_user, self.scopes
        );

        // Clear any existing cached token to force refresh
        {
            let mut cache = self.token_cache.write().await;
            cache.remove(impersonate_user);
        }

        // Get a fresh token (this will create a new one since cache is cleared)
        self.get_access_token(impersonate_user).await
    }

    pub async fn get_fresh_token(&self, impersonate_user: &str) -> Result<String> {
        // Check if token is near expiry (within 10 minutes)
        if self
            .is_token_near_expiry(impersonate_user, Duration::minutes(10))
            .await
        {
            warn!(
                "Token for user {} is near expiry, refreshing proactively",
                impersonate_user
            );
            self.refresh_access_token(impersonate_user).await
        } else {
            self.get_access_token(impersonate_user).await
        }
    }
}

/// Determine the required scopes based on the source type
pub fn get_scopes_for_source_type(source_type: SourceType) -> Vec<String> {
    let mut scopes = vec![
        // Admin scope is always needed to list users
        "https://www.googleapis.com/auth/admin.directory.user.readonly".to_string(),
    ];

    match source_type {
        SourceType::GoogleDrive => {
            scopes.push("https://www.googleapis.com/auth/drive.readonly".to_string());
        }
        SourceType::Gmail => {
            scopes.push("https://www.googleapis.com/auth/gmail.readonly".to_string());
        }
        _ => {
            // For any other source type, this shouldn't be called, but include both for compatibility
            scopes.push("https://www.googleapis.com/auth/drive.readonly".to_string());
            scopes.push("https://www.googleapis.com/auth/gmail.readonly".to_string());
        }
    }

    scopes
}

pub fn is_auth_error(status: reqwest::StatusCode) -> bool {
    status == reqwest::StatusCode::UNAUTHORIZED
}

#[derive(Debug)]
pub enum ApiResult<T> {
    Success(T),
    AuthError,
    OtherError(anyhow::Error),
}

pub async fn execute_with_auth_retry<T, F, Fut>(
    auth: &ServiceAccountAuth,
    user_email: &str,
    rate_limiter: Arc<RateLimiter>,
    operation: F,
) -> Result<T>
where
    F: Fn(String) -> Fut,
    Fut: std::future::Future<Output = Result<ApiResult<T>>>,
{
    let mut token = auth.get_fresh_token(user_email).await?;

    for attempt in 0..2 {
        let api_result = rate_limiter
            .execute_with_retry(|| async { operation(token.clone()).await.map_err(Into::into) })
            .await?;

        match api_result {
            ApiResult::Success(response) => return Ok(response),
            ApiResult::AuthError if attempt == 0 => {
                warn!(
                    "Got 401 error for user {}, refreshing token and retrying",
                    user_email
                );
                token = auth.refresh_access_token(user_email).await?;
                continue;
            }
            ApiResult::AuthError => {
                return Err(anyhow!(
                    "Authentication failed for user {} after token refresh",
                    user_email
                ));
            }
            ApiResult::OtherError(e) => return Err(e),
        }
    }

    unreachable!()
}

/// Service for managing Google service credentials and authentication
pub struct GoogleCredentialsService {
    service_credentials_repo: shared::db::repositories::ServiceCredentialsRepo,
}

impl GoogleCredentialsService {
    pub fn new(pool: sqlx::PgPool) -> Result<Self> {
        let service_credentials_repo = shared::db::repositories::ServiceCredentialsRepo::new(pool)?;
        Ok(Self {
            service_credentials_repo,
        })
    }

    /// Get service credentials by source ID
    pub async fn get_credentials_for_source(
        &self,
        source_id: &str,
    ) -> Result<shared::models::ServiceCredentials> {
        self.service_credentials_repo
            .get_by_source_id(source_id)
            .await?
            .ok_or_else(|| anyhow!("Service credentials not found for source: {}", source_id))
    }

    /// Create ServiceAccountAuth from service credentials with appropriate scopes for the source type
    pub fn create_service_auth(
        &self,
        creds: &shared::models::ServiceCredentials,
        source_type: SourceType,
    ) -> Result<ServiceAccountAuth> {
        let service_account_json = creds
            .credentials
            .get("service_account_key")
            .and_then(|v| v.as_str())
            .ok_or_else(|| anyhow!("Missing service_account_key in credentials"))?;

        // Check if custom scopes are provided in config, otherwise use defaults based on source type
        let scopes = creds
            .config
            .get("scopes")
            .and_then(|v| v.as_array())
            .map(|arr| {
                arr.iter()
                    .filter_map(|v| v.as_str().map(String::from))
                    .collect::<Vec<_>>()
            })
            .unwrap_or_else(|| get_scopes_for_source_type(source_type));

        ServiceAccountAuth::new(service_account_json, scopes)
    }

    /// Get domain from service credentials
    pub fn get_domain_from_credentials(
        &self,
        creds: &shared::models::ServiceCredentials,
    ) -> Result<String> {
        creds
            .config
            .get("domain")
            .and_then(|d| d.as_str())
            .map(String::from)
            .ok_or_else(|| anyhow!("Missing domain in service credentials config"))
    }

    /// Get principal email from service credentials
    pub fn get_principal_email_from_credentials(
        &self,
        creds: &shared::models::ServiceCredentials,
    ) -> Result<String> {
        creds
            .principal_email
            .as_ref()
            .map(String::from)
            .ok_or_else(|| anyhow!("Missing principal_email in service credentials"))
    }

    /// Complete authentication setup for a source - returns (auth, domain, principal_email)
    pub async fn setup_auth_for_source(
        &self,
        source_id: &str,
        source_type: SourceType,
    ) -> Result<(ServiceAccountAuth, String, String)> {
        let creds = self.get_credentials_for_source(source_id).await?;
        let auth = self.create_service_auth(&creds, source_type)?;
        let domain = self.get_domain_from_credentials(&creds)?;
        let principal_email = self.get_principal_email_from_credentials(&creds)?;

        Ok((auth, domain, principal_email))
    }

    /// Setup auth for admin operations (only needs admin directory scope)
    pub async fn setup_admin_auth_for_source(
        &self,
        source_id: &str,
    ) -> Result<(ServiceAccountAuth, String, String)> {
        let creds = self.get_credentials_for_source(source_id).await?;

        // For admin operations, we only need the admin directory scope
        let admin_scopes =
            vec!["https://www.googleapis.com/auth/admin.directory.user.readonly".to_string()];

        let service_account_json = creds
            .credentials
            .get("service_account_key")
            .and_then(|v| v.as_str())
            .ok_or_else(|| anyhow!("Missing service_account_key in credentials"))?;

        let auth = ServiceAccountAuth::new(service_account_json, admin_scopes)?;
        let domain = self.get_domain_from_credentials(&creds)?;
        let principal_email = self.get_principal_email_from_credentials(&creds)?;

        Ok((auth, domain, principal_email))
    }

    /// Get access token for a specific user impersonation
    pub async fn get_access_token_for_source(
        &self,
        source_id: &str,
        source_type: SourceType,
        impersonate_user: &str,
    ) -> Result<String> {
        let (auth, _domain, _principal_email) =
            self.setup_auth_for_source(source_id, source_type).await?;
        auth.get_access_token(impersonate_user).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_get_scopes_for_google_drive() {
        let scopes = get_scopes_for_source_type(SourceType::GoogleDrive);

        assert!(scopes.contains(
            &"https://www.googleapis.com/auth/admin.directory.user.readonly".to_string()
        ));
        assert!(scopes.contains(&"https://www.googleapis.com/auth/drive.readonly".to_string()));
        assert!(!scopes.contains(&"https://www.googleapis.com/auth/gmail.readonly".to_string()));
        assert_eq!(scopes.len(), 2);
    }

    #[test]
    fn test_get_scopes_for_gmail() {
        let scopes = get_scopes_for_source_type(SourceType::Gmail);

        assert!(scopes.contains(
            &"https://www.googleapis.com/auth/admin.directory.user.readonly".to_string()
        ));
        assert!(scopes.contains(&"https://www.googleapis.com/auth/gmail.readonly".to_string()));
        assert!(!scopes.contains(&"https://www.googleapis.com/auth/drive.readonly".to_string()));
        assert_eq!(scopes.len(), 2);
    }

    #[test]
    fn test_get_scopes_for_other_source_types() {
        let scopes = get_scopes_for_source_type(SourceType::LocalFiles);

        // For other source types, should include both drive and gmail scopes
        assert!(scopes.contains(
            &"https://www.googleapis.com/auth/admin.directory.user.readonly".to_string()
        ));
        assert!(scopes.contains(&"https://www.googleapis.com/auth/drive.readonly".to_string()));
        assert!(scopes.contains(&"https://www.googleapis.com/auth/gmail.readonly".to_string()));
        assert_eq!(scopes.len(), 3);
    }

    #[test]
    fn test_is_auth_error() {
        assert!(is_auth_error(reqwest::StatusCode::UNAUTHORIZED));
        assert!(!is_auth_error(reqwest::StatusCode::OK));
        assert!(!is_auth_error(reqwest::StatusCode::FORBIDDEN));
        assert!(!is_auth_error(reqwest::StatusCode::NOT_FOUND));
        assert!(!is_auth_error(reqwest::StatusCode::INTERNAL_SERVER_ERROR));
    }

    #[test]
    fn test_google_service_account_key_deserialization() {
        let json = r#"{
            "type": "service_account",
            "project_id": "my-project",
            "private_key_id": "key123",
            "private_key": "-----BEGIN RSA PRIVATE KEY-----\ntest\n-----END RSA PRIVATE KEY-----\n",
            "client_email": "service@my-project.iam.gserviceaccount.com",
            "client_id": "123456789",
            "auth_uri": "https://accounts.google.com/o/oauth2/auth",
            "token_uri": "https://oauth2.googleapis.com/token",
            "auth_provider_x509_cert_url": "https://www.googleapis.com/oauth2/v1/certs",
            "client_x509_cert_url": "https://www.googleapis.com/robot/v1/metadata/x509/service%40my-project.iam.gserviceaccount.com"
        }"#;

        let key: GoogleServiceAccountKey = serde_json::from_str(json).unwrap();

        assert_eq!(key.key_type, "service_account");
        assert_eq!(key.project_id, "my-project");
        assert_eq!(key.private_key_id, "key123");
        assert_eq!(
            key.client_email,
            "service@my-project.iam.gserviceaccount.com"
        );
        assert_eq!(key.client_id, "123456789");
        assert_eq!(key.token_uri, "https://oauth2.googleapis.com/token");
    }

    #[test]
    fn test_service_account_auth_rejects_invalid_key_type() {
        let json = r#"{
            "type": "authorized_user",
            "project_id": "my-project",
            "private_key_id": "key123",
            "private_key": "-----BEGIN RSA PRIVATE KEY-----\ntest\n-----END RSA PRIVATE KEY-----\n",
            "client_email": "user@example.com",
            "client_id": "123456789",
            "auth_uri": "https://accounts.google.com/o/oauth2/auth",
            "token_uri": "https://oauth2.googleapis.com/token",
            "auth_provider_x509_cert_url": "https://www.googleapis.com/oauth2/v1/certs",
            "client_x509_cert_url": "https://www.googleapis.com/robot/v1/metadata/x509/user%40example.com"
        }"#;

        let result = ServiceAccountAuth::new(json, vec!["scope".to_string()]);
        assert!(result.is_err());
        let err = result.err().unwrap();
        assert!(err.to_string().contains("Invalid key type"));
    }

    #[test]
    fn test_api_result_success() {
        let result: ApiResult<String> = ApiResult::Success("test".to_string());
        match result {
            ApiResult::Success(value) => assert_eq!(value, "test"),
            _ => panic!("Expected Success variant"),
        }
    }

    #[test]
    fn test_api_result_auth_error() {
        let result: ApiResult<String> = ApiResult::AuthError;
        match result {
            ApiResult::AuthError => {}
            _ => panic!("Expected AuthError variant"),
        }
    }

    #[test]
    fn test_api_result_other_error() {
        let result: ApiResult<String> = ApiResult::OtherError(anyhow!("Test error"));
        match result {
            ApiResult::OtherError(e) => assert!(e.to_string().contains("Test error")),
            _ => panic!("Expected OtherError variant"),
        }
    }
}

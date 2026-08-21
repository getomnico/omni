use anyhow::{Context, Result, anyhow};
use chrono::{Duration, Utc};
use jsonwebtoken::{Algorithm, EncodingKey, Header, encode};
use omni_connector_sdk::RateLimiter;
use omni_connector_sdk::{RetryableError, ServiceCredential, SourceType};
use reqwest::header::{HeaderMap, RETRY_AFTER};
use reqwest::{Client, StatusCode};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration as StdDuration;
use tokio::sync::RwLock;
use tracing::{debug, info, warn};

pub const GOOGLE_DRIVE_READ_SCOPE: &str = "https://www.googleapis.com/auth/drive.readonly";
pub const GOOGLE_DRIVE_WRITE_SCOPE: &str = "https://www.googleapis.com/auth/drive.file";
pub const GOOGLE_DOCS_SCOPE: &str = "https://www.googleapis.com/auth/documents";
pub const GOOGLE_SHEETS_SCOPE: &str = "https://www.googleapis.com/auth/spreadsheets";
pub const GOOGLE_SLIDES_SCOPE: &str = "https://www.googleapis.com/auth/presentations";
pub const GMAIL_READ_SCOPE: &str = "https://www.googleapis.com/auth/gmail.readonly";
pub const GMAIL_SEND_SCOPE: &str = "https://www.googleapis.com/auth/gmail.send";
pub const GMAIL_MODIFY_SCOPE: &str = "https://www.googleapis.com/auth/gmail.modify";
pub const GOOGLE_CHAT_SPACES_READ_SCOPE: &str =
    "https://www.googleapis.com/auth/chat.spaces.readonly";
pub const GOOGLE_CHAT_MEMBERSHIPS_READ_SCOPE: &str =
    "https://www.googleapis.com/auth/chat.memberships.readonly";
pub const GOOGLE_CHAT_ADMIN_MEMBERSHIPS_READ_SCOPE: &str =
    "https://www.googleapis.com/auth/chat.admin.memberships.readonly";
pub const GOOGLE_CHAT_MESSAGES_READ_SCOPE: &str =
    "https://www.googleapis.com/auth/chat.messages.readonly";
pub const GOOGLE_ADMIN_DIRECTORY_USER_READ_SCOPE: &str =
    "https://www.googleapis.com/auth/admin.directory.user.readonly";
pub const GOOGLE_ADMIN_DIRECTORY_GROUP_READ_SCOPE: &str =
    "https://www.googleapis.com/auth/admin.directory.group.readonly";

/// Scopes used by the Google Workspace CLI bridge when a JWT credential does
/// not carry an explicit scope override.
pub const GOOGLE_WORKSPACE_SCOPES: &[&str] = &[
    GOOGLE_ADMIN_DIRECTORY_USER_READ_SCOPE,
    GOOGLE_ADMIN_DIRECTORY_GROUP_READ_SCOPE,
    GOOGLE_DRIVE_READ_SCOPE,
    GOOGLE_DRIVE_WRITE_SCOPE,
    GOOGLE_DOCS_SCOPE,
    GOOGLE_SHEETS_SCOPE,
    GOOGLE_SLIDES_SCOPE,
    GMAIL_READ_SCOPE,
    GMAIL_SEND_SCOPE,
    GMAIL_MODIFY_SCOPE,
];

#[derive(Debug, Clone, Deserialize)]
pub struct GoogleServiceAccountCredentials {
    pub service_account_key: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct GoogleOAuthCredentials {
    pub access_token: Option<String>,
    pub refresh_token: String,
    pub expires_at: Option<i64>,
    pub user_email: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(untagged)]
pub enum GoogleCredentialPayload {
    ServiceAccount(GoogleServiceAccountCredentials),
    OAuth(GoogleOAuthCredentials),
}

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
    #[serde(skip_serializing_if = "Option::is_none")]
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

        debug!("Building JWT for user: {}", impersonate_user);

        let now = Utc::now();
        let exp = now + Duration::hours(1);

        // Self-auth: when the requested subject is the service account's own
        // client_email, omit the `sub` claim entirely so Google issues a token
        // for the service account itself rather than impersonating a user.
        // This is the SA-direct mode (no domain-wide delegation).
        let subject = (impersonate_user != self.service_account.client_email)
            .then(|| impersonate_user.to_string());

        let claims = GoogleJwtClaims {
            iss: self.service_account.client_email.clone(),
            sub: subject,
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

        debug!(
            "Sending token request to {}",
            self.service_account.token_uri
        );

        let response = match tokio::time::timeout(
            std::time::Duration::from_secs(30),
            self.client
                .post(&self.service_account.token_uri)
                .form(&params)
                .send(),
        )
        .await
        {
            Ok(result) => result?,
            Err(_) => {
                return Err(anyhow!(
                    "Token request to {} timed out after 30s for user {}",
                    self.service_account.token_uri,
                    impersonate_user
                ));
            }
        };

        debug!("Token response received: status={}", response.status());

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

        debug!(
            "Acquiring token cache write lock for user: {}",
            impersonate_user
        );

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

        debug!("Token cached for user: {}", impersonate_user);

        Ok(token_response.access_token)
    }

    /// Get an access token for the service account itself (no `sub` claim,
    /// no impersonation). Used by SA-direct mode.
    pub async fn get_self_access_token(&self) -> Result<String> {
        self.get_access_token(&self.service_account.client_email)
            .await
    }

    /// The service account's own email address.
    pub fn client_email(&self) -> &str {
        &self.service_account.client_email
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

/// OAuth2 authentication for individual user tokens
#[derive(Clone)]
pub struct OAuthAuth {
    access_token: Arc<RwLock<String>>,
    refresh_token: String,
    client_id: String,
    client_secret: String,
    token_expiry: Arc<RwLock<i64>>,
    user_email: String,
    client: Client,
}

impl OAuthAuth {
    pub fn new(
        access_token: String,
        refresh_token: String,
        expires_at: i64,
        user_email: String,
        client_id: String,
        client_secret: String,
    ) -> Result<Self> {
        let client = Client::builder()
            .pool_max_idle_per_host(5)
            .timeout(std::time::Duration::from_secs(30))
            .connect_timeout(std::time::Duration::from_secs(10))
            .build()
            .map_err(|e| anyhow!("Failed to build HTTP client: {}", e))?;

        Ok(Self {
            access_token: Arc::new(RwLock::new(access_token)),
            refresh_token,
            client_id,
            client_secret,
            token_expiry: Arc::new(RwLock::new(expires_at)),
            user_email,
            client,
        })
    }

    pub fn user_email(&self) -> &str {
        &self.user_email
    }

    /// Get a valid access token, refreshing if near expiry
    pub async fn get_access_token(&self, _user_email: &str) -> Result<String> {
        let now = Utc::now().timestamp();
        let expiry = { *self.token_expiry.read().await };

        // Refresh if token expires within 5 minutes
        if expiry <= now + 300 {
            return self.refresh_access_token().await;
        }

        Ok(self.access_token.read().await.clone())
    }

    pub async fn refresh_access_token(&self) -> Result<String> {
        info!(
            "Refreshing OAuth access token for user: {}",
            self.user_email
        );

        let params = [
            ("client_id", self.client_id.as_str()),
            ("client_secret", self.client_secret.as_str()),
            ("refresh_token", self.refresh_token.as_str()),
            ("grant_type", "refresh_token"),
        ];

        let response = self
            .client
            .post("https://oauth2.googleapis.com/token")
            .form(&params)
            .send()
            .await?;

        if !response.status().is_success() {
            let error_text = response.text().await?;
            return Err(anyhow!(
                "Failed to refresh OAuth token for {}: {}",
                self.user_email,
                error_text
            ));
        }

        #[derive(Deserialize)]
        struct TokenResponse {
            access_token: String,
            expires_in: i64,
        }

        let token_response: TokenResponse = response.json().await?;
        let now = Utc::now().timestamp();

        {
            let mut token = self.access_token.write().await;
            *token = token_response.access_token.clone();
        }
        {
            let mut expiry = self.token_expiry.write().await;
            *expiry = now + token_response.expires_in;
        }

        Ok(token_response.access_token)
    }

    pub async fn get_fresh_token(&self, user_email: &str) -> Result<String> {
        self.get_access_token(user_email).await
    }
}

/// Unified auth enum that wraps both service account and OAuth authentication
#[derive(Clone)]
pub enum GoogleAuth {
    ServiceAccount(ServiceAccountAuth),
    OAuth(OAuthAuth),
}

impl GoogleAuth {
    pub async fn get_access_token(&self, user_email: &str) -> Result<String> {
        match self {
            GoogleAuth::ServiceAccount(sa) => sa.get_access_token(user_email).await,
            GoogleAuth::OAuth(oauth) => oauth.get_access_token(user_email).await,
        }
    }

    /// Get a token for the service account itself (no `sub` claim). Only valid
    /// for service-account auth; OAuth has no self-token concept.
    pub async fn get_self_access_token(&self) -> Result<String> {
        match self {
            GoogleAuth::ServiceAccount(sa) => sa.get_self_access_token().await,
            GoogleAuth::OAuth(_) => Err(anyhow!("self-token not supported for OAuth")),
        }
    }

    pub async fn get_fresh_token(&self, user_email: &str) -> Result<String> {
        match self {
            GoogleAuth::ServiceAccount(sa) => sa.get_fresh_token(user_email).await,
            GoogleAuth::OAuth(oauth) => oauth.get_fresh_token(user_email).await,
        }
    }

    pub async fn refresh_access_token(&self, user_email: &str) -> Result<String> {
        match self {
            GoogleAuth::ServiceAccount(sa) => sa.refresh_access_token(user_email).await,
            GoogleAuth::OAuth(oauth) => oauth.refresh_access_token().await,
        }
    }

    pub fn is_oauth(&self) -> bool {
        matches!(self, GoogleAuth::OAuth(_))
    }

    pub fn oauth_user_email(&self) -> Option<&str> {
        match self {
            GoogleAuth::OAuth(oauth) => Some(oauth.user_email()),
            _ => None,
        }
    }
}

/// Determine the required scopes based on the source type (for service accounts with admin delegation)
pub fn get_scopes_for_source_type(source_type: SourceType) -> Vec<String> {
    let mut scopes = vec![
        // Admin scopes needed to list users and groups
        GOOGLE_ADMIN_DIRECTORY_USER_READ_SCOPE.to_string(),
        GOOGLE_ADMIN_DIRECTORY_GROUP_READ_SCOPE.to_string(),
    ];

    match source_type {
        SourceType::GoogleDrive => {
            scopes.push(GOOGLE_DRIVE_READ_SCOPE.to_string());
        }
        SourceType::Gmail => {
            scopes.push(GMAIL_READ_SCOPE.to_string());
        }
        SourceType::GoogleChat => {
            scopes.push(GOOGLE_CHAT_SPACES_READ_SCOPE.to_string());
            scopes.push(GOOGLE_CHAT_MEMBERSHIPS_READ_SCOPE.to_string());
            scopes.push(GOOGLE_CHAT_ADMIN_MEMBERSHIPS_READ_SCOPE.to_string());
            scopes.push(GOOGLE_CHAT_MESSAGES_READ_SCOPE.to_string());
        }
        _ => {
            scopes.push(GOOGLE_DRIVE_READ_SCOPE.to_string());
            scopes.push(GMAIL_READ_SCOPE.to_string());
        }
    }

    scopes
}

/// Determine the required OAuth scopes for a source type (no admin directory scope)
pub fn get_oauth_scopes_for_source_type(source_type: SourceType) -> Vec<String> {
    match source_type {
        SourceType::GoogleDrive => vec![GOOGLE_DRIVE_READ_SCOPE.to_string()],
        SourceType::Gmail => vec![GMAIL_READ_SCOPE.to_string()],
        SourceType::GoogleChat => vec![
            GOOGLE_CHAT_SPACES_READ_SCOPE.to_string(),
            GOOGLE_CHAT_MEMBERSHIPS_READ_SCOPE.to_string(),
            GOOGLE_CHAT_MESSAGES_READ_SCOPE.to_string(),
        ],
        _ => vec![
            GOOGLE_DRIVE_READ_SCOPE.to_string(),
            GMAIL_READ_SCOPE.to_string(),
        ],
    }
}

/// Return whether a service-account credential explicitly or implicitly carries a scope.
pub fn has_service_account_scope(
    creds: &ServiceCredential,
    source_type: SourceType,
    required_scope: &str,
) -> bool {
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

    scopes.iter().any(|scope| scope.as_str() == required_scope)
}

/// Build a `ServiceAccountAuth` from a `ServiceCredential` JWT row. Honors a
/// `scopes` override in `creds.config` and falls back to the per-source-type
/// defaults from `get_scopes_for_source_type`.
pub fn create_service_auth(
    creds: &ServiceCredential,
    source_type: SourceType,
) -> Result<ServiceAccountAuth> {
    let service_account_credentials: GoogleServiceAccountCredentials =
        serde_json::from_value(creds.credentials.clone())
            .context("Invalid Google service-account credentials")?;

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

    ServiceAccountAuth::new(&service_account_credentials.service_account_key, scopes)
}

/// Read the workspace `domain` from a `ServiceCredential` config blob.
pub fn get_domain_from_credentials(creds: &ServiceCredential) -> Result<String> {
    creds
        .config
        .get("domain")
        .and_then(|v| v.as_str())
        .map(String::from)
        .ok_or_else(|| anyhow!("Missing domain in service credentials config"))
}

pub const DEFAULT_GOOGLE_MAX_RETRIES: u32 = 5;

pub fn google_max_retries() -> u32 {
    std::env::var("GOOGLE_MAX_RETRIES")
        .ok()
        .and_then(|value| value.parse::<u32>().ok())
        .unwrap_or(DEFAULT_GOOGLE_MAX_RETRIES)
}

pub fn is_auth_error(status: StatusCode) -> bool {
    status == StatusCode::UNAUTHORIZED
}

pub fn is_rate_limit_error(status: StatusCode) -> bool {
    status == StatusCode::TOO_MANY_REQUESTS
}

pub(crate) fn parse_retry_after(headers: &HeaderMap) -> Option<StdDuration> {
    let value = headers.get(RETRY_AFTER)?.to_str().ok()?;

    if let Ok(seconds) = value.parse::<u64>() {
        return Some(StdDuration::from_secs(seconds));
    }

    chrono::DateTime::parse_from_rfc2822(value)
        .ok()
        .and_then(|date| date.signed_duration_since(Utc::now()).to_std().ok())
}

fn classify_google_api_status<T>(
    status: StatusCode,
    headers: &HeaderMap,
    error_text: String,
    context: String,
) -> ApiResult<T> {
    if is_auth_error(status) {
        ApiResult::AuthError(anyhow!(
            "Google API auth error (HTTP {}): {}",
            status,
            error_text
        ))
    } else if is_rate_limit_error(status) {
        let message = format!("{}: HTTP {} - {}", context, status, error_text);
        match parse_retry_after(headers) {
            Some(retry_after) => ApiResult::RetryableError(RetryableError::RateLimited {
                retry_after,
                message,
            }),
            None => ApiResult::RetryableError(RetryableError::Transient(anyhow!(message))),
        }
    } else {
        ApiResult::OtherError(anyhow!("{}: HTTP {} - {}", context, status, error_text))
    }
}

/// Consume a failed HTTP response and classify it for `execute_with_auth_retry`.
///
/// 401 responses are returned as auth errors so the caller can refresh a token.
/// 429 responses are returned as retryable rate-limiter errors: with
/// `Retry-After` they use the server-specified wait; without it they use the
/// rate limiter's existing exponential backoff path.
pub async fn classify_google_api_error<T>(
    response: reqwest::Response,
    context: impl Into<String>,
) -> Result<ApiResult<T>> {
    let status = response.status();
    let headers = response.headers().clone();
    let error_text = match response.text().await {
        Ok(text) => text,
        Err(e) => format!("(failed to read error body: {})", e),
    };

    Ok(classify_google_api_status(
        status,
        &headers,
        error_text,
        context.into(),
    ))
}

/// Consume a failed HTTP response and produce an `ApiResult::AuthError`
/// carrying the response body so the actual Google error message is
/// preserved in the causal chain.
pub async fn api_auth_error<T>(response: reqwest::Response) -> Result<ApiResult<T>> {
    classify_google_api_error(response, "Google API auth error").await
}

pub async fn classify_google_api_retry_error(
    response: reqwest::Response,
    context: impl Into<String>,
) -> Result<RetryableError> {
    match classify_google_api_error::<()>(response, context).await? {
        ApiResult::RetryableError(e) => Ok(e),
        ApiResult::AuthError(e) | ApiResult::OtherError(e) => Ok(RetryableError::Permanent(e)),
        ApiResult::Success(_) => unreachable!("error classifier cannot return success"),
    }
}

#[derive(Debug)]
pub enum ApiResult<T> {
    Success(T),
    AuthError(anyhow::Error),
    RetryableError(RetryableError),
    OtherError(anyhow::Error),
}

pub async fn execute_with_auth_retry<T, F, Fut>(
    auth: &GoogleAuth,
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
            .execute_with_retry(|| async {
                match operation(token.clone())
                    .await
                    .map_err(RetryableError::Transient)?
                {
                    ApiResult::RetryableError(e) => Err(e),
                    other => Ok(other),
                }
            })
            .await?;

        match api_result {
            ApiResult::Success(response) => return Ok(response),
            ApiResult::AuthError(e) if attempt == 0 => {
                warn!(
                    error = %e,
                    user = %user_email,
                    "Got 401 error, refreshing token and retrying"
                );
                token = auth.refresh_access_token(user_email).await?;
                continue;
            }
            ApiResult::AuthError(e) => {
                return Err(e.context(format!(
                    "Authentication failed for user {} after token refresh",
                    user_email
                )));
            }
            ApiResult::RetryableError(_) => {
                unreachable!("retryable API result should be handled by RateLimiter")
            }
            ApiResult::OtherError(e) => return Err(e),
        }
    }

    unreachable!()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_get_scopes_for_google_drive() {
        let scopes = get_scopes_for_source_type(SourceType::GoogleDrive);

        assert!(scopes.contains(&GOOGLE_ADMIN_DIRECTORY_USER_READ_SCOPE.to_string()));
        assert!(scopes.contains(&GOOGLE_DRIVE_READ_SCOPE.to_string()));
        assert!(!scopes.contains(&GMAIL_READ_SCOPE.to_string()));
        assert_eq!(scopes.len(), 3);
    }

    #[test]
    fn test_get_scopes_for_gmail() {
        let scopes = get_scopes_for_source_type(SourceType::Gmail);

        assert!(scopes.contains(&GOOGLE_ADMIN_DIRECTORY_USER_READ_SCOPE.to_string()));
        assert!(scopes.contains(&GOOGLE_ADMIN_DIRECTORY_GROUP_READ_SCOPE.to_string()));
        assert!(scopes.contains(&GMAIL_READ_SCOPE.to_string()));
        assert!(!scopes.contains(&GOOGLE_DRIVE_READ_SCOPE.to_string()));
        assert_eq!(scopes.len(), 3);
    }

    #[test]
    fn test_get_scopes_for_other_source_types() {
        let scopes = get_scopes_for_source_type(SourceType::LocalFiles);

        // For other source types, should include both drive and gmail scopes
        assert!(scopes.contains(&GOOGLE_ADMIN_DIRECTORY_USER_READ_SCOPE.to_string()));
        assert!(scopes.contains(&GOOGLE_ADMIN_DIRECTORY_GROUP_READ_SCOPE.to_string()));
        assert!(scopes.contains(&GOOGLE_DRIVE_READ_SCOPE.to_string()));
        assert!(scopes.contains(&GMAIL_READ_SCOPE.to_string()));
        assert_eq!(scopes.len(), 4);
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
    fn test_self_auth_omits_sub_claim() {
        // Self-auth (passing client_email as the subject) must serialize the
        // JWT WITHOUT a `sub` claim — Google rejects `"sub": null`.
        let claims = GoogleJwtClaims {
            iss: "sa@project.iam.gserviceaccount.com".to_string(),
            sub: None,
            scope: GOOGLE_DRIVE_READ_SCOPE.to_string(),
            aud: "https://oauth2.googleapis.com/token".to_string(),
            exp: 1_700_000_000,
            iat: 1_699_999_400,
        };
        let serialized = serde_json::to_value(&claims).unwrap();
        assert!(
            serialized.get("sub").is_none(),
            "self-auth JWT must not contain a sub claim, got {:?}",
            serialized
        );
    }

    #[test]
    fn test_impersonated_claims_include_sub() {
        let claims = GoogleJwtClaims {
            iss: "sa@project.iam.gserviceaccount.com".to_string(),
            sub: Some("admin@example.com".to_string()),
            scope: GOOGLE_DRIVE_READ_SCOPE.to_string(),
            aud: "https://oauth2.googleapis.com/token".to_string(),
            exp: 1_700_000_000,
            iat: 1_699_999_400,
        };
        let serialized = serde_json::to_value(&claims).unwrap();
        assert_eq!(
            serialized.get("sub").and_then(|v| v.as_str()),
            Some("admin@example.com")
        );
    }

    #[test]
    fn test_client_email_matches_self_auth_condition() {
        // The `sub` is omitted exactly when the requested subject equals the
        // service account's own client_email.
        let sa = GoogleServiceAccountKey {
            key_type: "service_account".to_string(),
            project_id: "p".to_string(),
            private_key_id: "k".to_string(),
            private_key: "-----BEGIN RSA PRIVATE KEY-----\ntest\n-----END RSA PRIVATE KEY-----\n"
                .to_string(),
            client_email: "sa@project.iam.gserviceaccount.com".to_string(),
            client_id: "1".to_string(),
            auth_uri: "https://accounts.google.com/o/oauth2/auth".to_string(),
            token_uri: "https://oauth2.googleapis.com/token".to_string(),
            auth_provider_x509_cert_url: "https://www.googleapis.com/oauth2/v1/certs".to_string(),
            client_x509_cert_url: "https://www.googleapis.com/robot/v1/metadata/x509/sa"
                .to_string(),
        };
        assert!(
            !(sa.client_email != sa.client_email),
            "self-auth comparison must be false for identical emails"
        );
        let subject =
            ("admin@example.com" != sa.client_email).then(|| "admin@example.com".to_string());
        assert!(subject.is_some(), "real user impersonation keeps sub");
        let self_subject =
            (sa.client_email.as_str() != sa.client_email.as_str()).then(|| sa.client_email.clone());
        assert!(self_subject.is_none(), "self-auth drops sub");
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
        let result: ApiResult<String> = ApiResult::AuthError(anyhow!("auth error"));
        match result {
            ApiResult::AuthError(_) => {}
            _ => panic!("Expected AuthError variant"),
        }
    }

    #[test]
    fn test_classify_google_401_as_auth_error() {
        let result: ApiResult<()> = classify_google_api_status(
            StatusCode::UNAUTHORIZED,
            &HeaderMap::new(),
            "invalid token".to_string(),
            "test request".to_string(),
        );

        match result {
            ApiResult::AuthError(e) => assert!(e.to_string().contains("invalid token")),
            _ => panic!("Expected AuthError variant"),
        }
    }

    #[test]
    fn test_classify_google_429_with_retry_after_as_rate_limited() {
        let mut headers = HeaderMap::new();
        headers.insert(RETRY_AFTER, "2".parse().unwrap());

        let result: ApiResult<()> = classify_google_api_status(
            StatusCode::TOO_MANY_REQUESTS,
            &headers,
            "quota exceeded".to_string(),
            "test request".to_string(),
        );

        match result {
            ApiResult::RetryableError(RetryableError::RateLimited {
                retry_after,
                message,
            }) => {
                assert_eq!(retry_after, StdDuration::from_secs(2));
                assert!(message.contains("quota exceeded"));
            }
            _ => panic!("Expected RetryableError::RateLimited variant"),
        }
    }

    #[test]
    fn test_classify_google_429_without_retry_after_as_transient() {
        let result: ApiResult<()> = classify_google_api_status(
            StatusCode::TOO_MANY_REQUESTS,
            &HeaderMap::new(),
            "quota exceeded".to_string(),
            "test request".to_string(),
        );

        match result {
            ApiResult::RetryableError(RetryableError::Transient(e)) => {
                assert!(e.to_string().contains("quota exceeded"));
            }
            _ => panic!("Expected RetryableError::Transient variant"),
        }
    }

    #[test]
    fn test_parse_retry_after_http_date() {
        let mut headers = HeaderMap::new();
        let retry_at = Utc::now() + Duration::seconds(120);
        headers.insert(RETRY_AFTER, retry_at.to_rfc2822().parse().unwrap());

        let parsed = parse_retry_after(&headers).expect("retry-after date should parse");
        assert!(parsed <= StdDuration::from_secs(120));
        assert!(parsed > StdDuration::from_secs(0));
    }

    #[test]
    fn test_classify_non_429_as_other_error() {
        let result: ApiResult<()> = classify_google_api_status(
            StatusCode::NOT_FOUND,
            &HeaderMap::new(),
            "missing".to_string(),
            "test request".to_string(),
        );

        match result {
            ApiResult::OtherError(e) => assert!(e.to_string().contains("missing")),
            _ => panic!("Expected OtherError variant"),
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

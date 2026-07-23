use anyhow::{anyhow, Context, Result};
use serde::Deserialize;
use serde_json::Value as JsonValue;
use sqlx::PgPool;
use time::{Duration, OffsetDateTime};

use crate::encryption::{EncryptedData, EncryptionService};
use crate::models::{AuthType, ServiceCredential, Source, SourceScope};

const OAUTH_REFRESH_SKEW: Duration = Duration::minutes(1);

#[derive(Deserialize)]
struct OAuthRefreshResponse {
    access_token: String,
    refresh_token: Option<String>,
    token_type: Option<String>,
    expires_in: Option<i64>,
}

fn oauth_needs_refresh(creds: &ServiceCredential) -> bool {
    creds.auth_type == AuthType::OAuth
        && creds
            .expires_at
            .is_some_and(|expires_at| expires_at <= OffsetDateTime::now_utc() + OAUTH_REFRESH_SKEW)
}

fn oauth_is_refreshable(creds: &ServiceCredential) -> bool {
    let Some(values) = creds.credentials.as_object() else {
        return false;
    };
    ["refresh_token", "client_id", "token_uri"]
        .iter()
        .all(|key| {
            values
                .get(*key)
                .and_then(JsonValue::as_str)
                .is_some_and(|value| !value.is_empty())
        })
}

async fn refresh_oauth_tokens(creds: &mut ServiceCredential) -> Result<()> {
    let values = creds
        .credentials
        .as_object_mut()
        .ok_or_else(|| anyhow!("OAuth credentials must be a JSON object"))?;
    let string_value = |key: &str| {
        values
            .get(key)
            .and_then(JsonValue::as_str)
            .filter(|value| !value.is_empty())
            .map(str::to_owned)
    };
    let refresh_token = string_value("refresh_token").context("missing OAuth refresh_token")?;
    let client_id = string_value("client_id").context("missing OAuth client_id")?;
    let token_uri = string_value("token_uri").context("missing OAuth token_uri")?;
    let client_secret = string_value("client_secret");
    let auth_method = string_value("token_endpoint_auth_method").unwrap_or_else(|| {
        if client_secret.is_some() {
            "client_secret_post".to_string()
        } else {
            "none".to_string()
        }
    });

    let mut form = vec![
        ("grant_type", "refresh_token".to_string()),
        ("refresh_token", refresh_token),
    ];
    if auth_method != "client_secret_basic" {
        form.push(("client_id", client_id.clone()));
    }
    if let Some(resource) = string_value("resource") {
        form.push(("resource", resource));
    }
    if auth_method == "client_secret_post" {
        form.push((
            "client_secret",
            client_secret
                .clone()
                .context("missing OAuth client_secret")?,
        ));
    }

    let client = reqwest::Client::new();
    let mut request = client.post(&token_uri).form(&form);
    if auth_method == "client_secret_basic" {
        request = request.basic_auth(
            client_id,
            Some(client_secret.context("missing OAuth client_secret")?),
        );
    } else if auth_method != "none" && auth_method != "client_secret_post" {
        return Err(anyhow!("unsupported OAuth token endpoint auth method"));
    }
    let response = request
        .send()
        .await
        .context("OAuth token refresh request failed")?;
    if !response.status().is_success() {
        return Err(anyhow!(
            "OAuth token refresh failed with status {}",
            response.status()
        ));
    }
    let refreshed: OAuthRefreshResponse = response
        .json()
        .await
        .context("invalid OAuth token refresh response")?;
    let now = OffsetDateTime::now_utc();
    values.insert("access_token".to_string(), refreshed.access_token.into());
    if let Some(refresh_token) = refreshed.refresh_token {
        values.insert("refresh_token".to_string(), refresh_token.into());
    }
    if let Some(token_type) = refreshed.token_type {
        values.insert("token_type".to_string(), token_type.into());
    }
    let expires_in = refreshed
        .expires_in
        .filter(|seconds| *seconds > 0)
        .unwrap_or(3600);
    creds.expires_at = Some(now + Duration::seconds(expires_in));
    creds.last_validated_at = Some(now);
    Ok(())
}

/// Service credentials repository with encryption support.
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

    /// Fetch the org-wide credential row for a source (`user_id IS NULL`).
    pub async fn find_org_credential(&self, source_id: &str) -> Result<Option<ServiceCredential>> {
        let mut creds = sqlx::query_as::<_, ServiceCredential>(
            "SELECT * FROM service_credentials WHERE source_id = $1 AND user_id IS NULL",
        )
        .bind(source_id)
        .fetch_optional(&self.pool)
        .await?;

        if let Some(ref mut creds) = creds {
            self.decrypt_credentials_in_place(creds)?;
        }

        match creds {
            Some(creds) => Ok(Some(self.refresh_oauth_if_needed(creds).await?)),
            None => Ok(None),
        }
    }

    /// Fetch the credential row that "owns" a source — the one used for sync
    /// and any non-user-attributed action. Org sources own a `user_id IS NULL`
    /// row; personal (`scope='user'`) sources own a row keyed on the source
    /// creator's `user_id`. Migration 086 enforces this invariant.
    pub async fn find_owner_credential(
        &self,
        source: &Source,
    ) -> Result<Option<ServiceCredential>> {
        match source.scope {
            SourceScope::Org => self.find_org_credential(&source.id).await,
            SourceScope::User => {
                self.find_user_credential(&source.id, &source.created_by)
                    .await
            }
        }
    }

    /// Fetch the per-user credential row for an org-wide source.
    pub async fn find_user_credential(
        &self,
        source_id: &str,
        user_id: &str,
    ) -> Result<Option<ServiceCredential>> {
        let mut creds = sqlx::query_as::<_, ServiceCredential>(
            "SELECT * FROM service_credentials WHERE source_id = $1 AND user_id = $2",
        )
        .bind(source_id)
        .bind(user_id)
        .fetch_optional(&self.pool)
        .await?;

        if let Some(ref mut creds) = creds {
            self.decrypt_credentials_in_place(creds)?;
        }

        match creds {
            Some(creds) => Ok(Some(self.refresh_oauth_if_needed(creds).await?)),
            None => Ok(None),
        }
    }

    fn decrypt_credentials_in_place(&self, creds: &mut ServiceCredential) -> Result<()> {
        if let Some(encrypted_data) = creds.credentials.get("encrypted_data") {
            let encrypted_data: EncryptedData = serde_json::from_value(encrypted_data.clone())?;
            let decrypted_credentials: JsonValue =
                self.encryption_service.decrypt_json(&encrypted_data)?;
            creds.credentials = decrypted_credentials;
        }
        Ok(())
    }

    fn encrypt_credentials(&self, creds: &ServiceCredential) -> Result<JsonValue> {
        let encrypted_data = self.encryption_service.encrypt_json(&creds.credentials)?;
        Ok(serde_json::json!({
            "encrypted_data": encrypted_data,
            "version": 1
        }))
    }

    async fn refresh_oauth_if_needed(&self, creds: ServiceCredential) -> Result<ServiceCredential> {
        if !oauth_needs_refresh(&creds) || !oauth_is_refreshable(&creds) {
            return Ok(creds);
        }
        self.refresh_oauth_credential(&creds.id).await
    }

    async fn refresh_oauth_credential(&self, credential_id: &str) -> Result<ServiceCredential> {
        let mut tx = self.pool.begin().await?;
        sqlx::query("SELECT pg_advisory_xact_lock(hashtext($1))")
            .bind(credential_id)
            .execute(&mut *tx)
            .await?;

        let mut creds = sqlx::query_as::<_, ServiceCredential>(
            "SELECT * FROM service_credentials WHERE id = $1 FOR UPDATE",
        )
        .bind(credential_id)
        .fetch_one(&mut *tx)
        .await?;
        self.decrypt_credentials_in_place(&mut creds)?;

        // Another request may have refreshed this credential while we waited for
        // the per-row advisory lock. Recheck under the lock before calling the
        // provider so rotating refresh tokens are never replayed concurrently.
        if !oauth_needs_refresh(&creds) || !oauth_is_refreshable(&creds) {
            tx.commit().await?;
            return Ok(creds);
        }

        refresh_oauth_tokens(&mut creds).await?;
        let encrypted_credentials = self.encrypt_credentials(&creds)?;
        sqlx::query(
            "UPDATE service_credentials SET credentials = $2, expires_at = $3, last_validated_at = $4, updated_at = CURRENT_TIMESTAMP WHERE id = $1",
        )
        .bind(&creds.id)
        .bind(encrypted_credentials)
        .bind(creds.expires_at)
        .bind(creds.last_validated_at)
        .execute(&mut *tx)
        .await?;
        tx.commit().await?;
        Ok(creds)
    }

    pub async fn create(&self, creds: ServiceCredential) -> Result<ServiceCredential> {
        let encrypted_credentials = self.encrypt_credentials(&creds)?;

        let mut created_creds = sqlx::query_as::<_, ServiceCredential>(
            r#"
            INSERT INTO service_credentials
            (id, source_id, user_id, provider, auth_type, principal_email, credentials, config, expires_at, last_validated_at)
            VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10)
            RETURNING *
            "#,
        )
        .bind(&creds.id)
        .bind(&creds.source_id)
        .bind(&creds.user_id)
        .bind(creds.provider)
        .bind(creds.auth_type)
        .bind(&creds.principal_email)
        .bind(&encrypted_credentials)
        .bind(&creds.config)
        .bind(creds.expires_at)
        .bind(creds.last_validated_at)
        .fetch_one(&self.pool)
        .await?;

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

    /// Delete all credential rows for a source — used when the source itself is
    /// being removed. Cascades through both org-wide and per-user rows.
    pub async fn delete_by_source_id(&self, source_id: &str) -> Result<()> {
        sqlx::query("DELETE FROM service_credentials WHERE source_id = $1")
            .bind(source_id)
            .execute(&self.pool)
            .await?;

        Ok(())
    }

    /// Delete a per-user credential row. Used by the "disconnect" action in
    /// the user settings UI.
    pub async fn delete_for_user(&self, source_id: &str, user_id: &str) -> Result<()> {
        sqlx::query("DELETE FROM service_credentials WHERE source_id = $1 AND user_id = $2")
            .bind(source_id)
            .bind(user_id)
            .execute(&self.pool)
            .await?;

        Ok(())
    }

    /// Update credentials and refresh-related fields on a credential row.
    pub async fn update_credentials(&self, creds: &ServiceCredential) -> Result<()> {
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
        .bind(creds.expires_at)
        .execute(&self.pool)
        .await?;

        Ok(())
    }

    /// Find any per-user OAuth credential for sources matching the given
    /// source types and provider. Used for recovering a missing MCP catalog
    /// when a connector registers with `mcp_catalog_loaded: false`.
    pub async fn find_any_user_oauth_for_provider(
        &self,
        source_types: &[String],
        provider: &str,
    ) -> Result<Option<(String, String)>> {
        // Returns (source_id, user_id) for the most recently updated
        // per-user OAuth credential matching the criteria.
        // sqlx doesn't support array_agg -> text easily, so we use ANY(..) with
        // a typed PostgreSQL array.
        let result: Option<(String, String)> = sqlx::query_as(
            r#"
            SELECT sc.source_id, sc.user_id
            FROM service_credentials sc
            JOIN sources s ON s.id = sc.source_id AND NOT s.is_deleted
            WHERE sc.user_id IS NOT NULL
              AND sc.auth_type = 'oauth'
              AND sc.provider = $1
              AND s.source_type = ANY($2::text[])
            ORDER BY sc.updated_at DESC
            LIMIT 1
            "#,
        )
        .bind(provider)
        .bind(source_types)
        .fetch_optional(&self.pool)
        .await?;

        Ok(result)
    }

    pub async fn encrypt_existing_credentials(&self) -> Result<usize> {
        let mut count = 0;

        let unencrypted_creds = sqlx::query_as::<_, ServiceCredential>(
            "SELECT * FROM service_credentials WHERE NOT (credentials ? 'encrypted_data')",
        )
        .fetch_all(&self.pool)
        .await?;

        for creds in unencrypted_creds {
            self.update_credentials(&creds).await?;
            count += 1;
        }

        Ok(count)
    }
}

#[cfg(test)]
mod oauth_refresh_tests {
    use std::{collections::HashMap, sync::Arc};

    use axum::{
        extract::{Form, State},
        routing::post,
        Json, Router,
    };
    use serde_json::json;
    use tokio::sync::Mutex;

    use super::*;
    use crate::models::ServiceProvider;

    fn oauth_credential(
        credentials: JsonValue,
        expires_at: Option<OffsetDateTime>,
    ) -> ServiceCredential {
        let now = OffsetDateTime::now_utc();
        ServiceCredential {
            id: "credential-1".to_string(),
            source_id: "source-1".to_string(),
            user_id: None,
            provider: ServiceProvider::Clickup,
            auth_type: AuthType::OAuth,
            principal_email: Some("user@example.com".to_string()),
            credentials,
            config: json!({}),
            expires_at,
            last_validated_at: None,
            created_at: now,
            updated_at: now,
        }
    }

    #[test]
    fn refreshes_only_expiring_oauth_credentials_with_refresh_metadata() {
        let complete = json!({
            "refresh_token": "refresh-old",
            "client_id": "client-1",
            "token_uri": "https://windshift.example/api/oauth/token"
        });
        let expiring = oauth_credential(complete.clone(), Some(OffsetDateTime::now_utc()));
        assert!(oauth_needs_refresh(&expiring));
        assert!(oauth_is_refreshable(&expiring));

        let valid = oauth_credential(
            complete,
            Some(OffsetDateTime::now_utc() + Duration::hours(1)),
        );
        assert!(!oauth_needs_refresh(&valid));

        let incomplete = oauth_credential(json!({ "access_token": "token" }), None);
        assert!(!oauth_is_refreshable(&incomplete));
    }

    #[tokio::test]
    async fn refreshes_public_client_tokens_and_preserves_resource_binding() {
        type CapturedForm = Arc<Mutex<Option<HashMap<String, String>>>>;

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

        let captured: CapturedForm = Arc::new(Mutex::new(None));
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let app = Router::new()
            .route("/token", post(token_endpoint))
            .with_state(captured.clone());
        let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });

        let resource = "https://windshift.example/mcp";
        let mut credential = oauth_credential(
            json!({
                "access_token": "access-old",
                "refresh_token": "refresh-old",
                "client_id": "client-1",
                "token_uri": format!("http://{address}/token"),
                "token_endpoint_auth_method": "none",
                "resource": resource
            }),
            Some(OffsetDateTime::now_utc()),
        );

        refresh_oauth_tokens(&mut credential).await.unwrap();
        server.abort();

        assert_eq!(credential.credentials["access_token"], "access-new");
        assert_eq!(credential.credentials["refresh_token"], "refresh-new");
        assert_eq!(credential.credentials["token_type"], "Bearer");
        assert!(
            credential.expires_at.unwrap() > OffsetDateTime::now_utc() + Duration::seconds(100)
        );

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
        assert_eq!(form.get("resource").map(String::as_str), Some(resource));
        assert!(!form.contains_key("client_secret"));
    }
}

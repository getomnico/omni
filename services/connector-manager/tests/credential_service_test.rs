//! Integration tests for `CredentialService` — OAuth refresh with a real
//! Postgres database and a mock token endpoint.
//!
//! These tests verify that expired OAuth tokens are refreshed correctly
//! (HTTP call outside any DB transaction, updated fields persisted) and that
//! non-refreshable credentials pass through unchanged.
//!
//! # Test Postgres
//! Uses a long-lived Postgres container (`test-pg-test`) started once outside
//! the test harness to avoid testcontainers overhead. Create it with:
//! ```text
//! docker run -d --name test-pg-test \
//!   -e POSTGRES_DB=omni_test \
//!   -e POSTGRES_USER=omni \
//!   -e POSTGRES_PASSWORD=omni_password \
//!   -p 0:5432 \
//!   paradedb/paradedb:0.24.0-pg17
//! ```
//! Then run migrations once:
//! ```text
//! docker run --rm --network host \
//!   -e DATABASE_HOST=localhost \
//!   -e DATABASE_PORT=$(docker port test-pg-test 5432 | head -1 | sed 's/.*://') \
//!   -e DATABASE_USERNAME=omni \
//!   -e DATABASE_PASSWORD=omni_password \
//!   -e DATABASE_NAME=omni_test \
//!   -e DATABASE_SSL=false \
//!   omni-migrator:test
//! ```

use std::collections::HashMap;
use std::sync::{Arc, OnceLock};

use axum::{
    extract::{Form, State},
    routing::post,
    Json, Router,
};
use omni_connector_manager::credential_service::CredentialService;
use shared::db::pool::DatabasePool;
use shared::models::*;
use shared::ServiceCredentialsRepo;
use time::OffsetDateTime;
use tokio::sync::Mutex as TokioMutex;

// ---------------------------------------------------------------------------
// Test Postgres pool — shared across all tests in this binary
// ---------------------------------------------------------------------------

/// Initialise encryption env vars once per process.
static ENV_INIT: OnceLock<()> = OnceLock::new();
fn init_env() {
    ENV_INIT.get_or_init(|| {
        // SAFETY: test-only, single-threaded context.
        unsafe {
            std::env::set_var(
                "ENCRYPTION_KEY",
                "test_master_key_that_is_long_enough_32_chars",
            )
        };
        unsafe { std::env::set_var("ENCRYPTION_SALT", "test_salt_16_chars") };
    });
}

/// Postgres connection URL — override with `TEST_PG_HOST` / `TEST_PG_PORT`.
fn test_pg_url() -> String {
    let host = std::env::var("TEST_PG_HOST").unwrap_or_else(|_| "172.17.0.7".into());
    let port = std::env::var("TEST_PG_PORT").unwrap_or_else(|_| "5432".into());
    format!("postgresql://omni:omni_password@{host}:{port}/omni_test")
}

/// Create a fresh database pool (one per test, avoids cross-test state).
async fn pool() -> DatabasePool {
    init_env();
    DatabasePool::new_with_options(&test_pg_url(), 5, 30)
        .await
        .unwrap()
}

/// Wipe all test data so each test starts clean.
async fn clean_db(pool: &sqlx::PgPool) {
    sqlx::query("DELETE FROM service_credentials")
        .execute(pool)
        .await
        .ok();
    sqlx::query("DELETE FROM sources").execute(pool).await.ok();
    sqlx::query("DELETE FROM users").execute(pool).await.ok();
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn make_oauth_creds(
    id: &str,
    source_id: &str,
    user_id: Option<&str>,
    token_uri: &str,
    refresh_token: Option<&str>,
    client_id: Option<&str>,
    expires_at: Option<OffsetDateTime>,
) -> ServiceCredential {
    let mut creds = serde_json::json!({
        "access_token": "access-old",
        "token_uri": token_uri,
    });
    if let Some(rt) = refresh_token {
        creds["refresh_token"] = serde_json::json!(rt);
    }
    if let Some(cid) = client_id {
        creds["client_id"] = serde_json::json!(cid);
    }
    make_creds(id, source_id, user_id, AuthType::OAuth, creds, expires_at)
}

fn make_jwt_creds(id: &str, source_id: &str, user_id: Option<&str>) -> ServiceCredential {
    make_creds(
        id,
        source_id,
        user_id,
        AuthType::Jwt,
        serde_json::json!({"token": "some-jwt"}),
        None,
    )
}

fn make_creds(
    id: &str,
    source_id: &str,
    user_id: Option<&str>,
    auth_type: AuthType,
    credentials: serde_json::Value,
    expires_at: Option<OffsetDateTime>,
) -> ServiceCredential {
    let now = OffsetDateTime::now_utc();
    ServiceCredential {
        id: id.to_string(),
        source_id: source_id.to_string(),
        user_id: user_id.map(String::from),
        provider: ServiceProvider::Google,
        auth_type,
        principal_email: Some("test@example.com".into()),
        credentials,
        config: serde_json::json!({}),
        expires_at,
        last_validated_at: None,
        created_at: now,
        updated_at: now,
    }
}

/// Generate a 26-character ULID (matching the CHAR(26) column type).
fn new_id() -> &'static str {
    Box::leak(shared::utils::generate_ulid().into_boxed_str())
}

async fn seed_user(pool: &sqlx::PgPool, user_id: &str) {
    sqlx::query(
        r#"INSERT INTO users (id, email, password_hash, created_at, updated_at)
           VALUES ($1, $2, 'hash', NOW(), NOW())
           ON CONFLICT (id) DO NOTHING"#,
    )
    .bind(user_id)
    .bind(format!("{}@test.com", user_id))
    .execute(pool)
    .await
    .unwrap();
}

async fn seed_source(pool: &sqlx::PgPool, source_id: &str, scope: &str, created_by: &str) {
    sqlx::query(
        r#"INSERT INTO sources (id, name, source_type, config, scope, is_active, created_by, created_at, updated_at)
           VALUES ($1, 'Test Source', 'google_drive', '{}', $2, true, $3, NOW(), NOW())
           ON CONFLICT (id) DO NOTHING"#,
    )
    .bind(source_id)
    .bind(scope)
    .bind(created_by)
    .execute(pool)
    .await
    .unwrap();
}

// ── Mock OAuth token endpoint ──────────────────────────────────────────

type CapturedForm = Arc<TokioMutex<Option<HashMap<String, String>>>>;

/// Spawn an axum token endpoint that captures the form and returns canned
/// tokens.  Returns `(listen_address, server_handle, captured_form)`.
async fn spawn_token_endpoint() -> (String, tokio::task::JoinHandle<()>, CapturedForm) {
    let captured: CapturedForm = Arc::new(TokioMutex::new(None));

    async fn handler(
        State(captured): State<CapturedForm>,
        Form(form): Form<HashMap<String, String>>,
    ) -> Json<serde_json::Value> {
        *captured.lock().await = Some(form);
        Json(serde_json::json!({
            "access_token": "access-new",
            "refresh_token": "refresh-new",
            "token_type": "Bearer",
            "expires_in": 3600,
        }))
    }

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let app = Router::new()
        .route("/token", post(handler))
        .with_state(captured.clone());
    let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });

    (format!("http://{address}/token"), server, captured)
}

// ---------------------------------------------------------------------------
// Test: CredentialService — full OAuth refresh flow
// ---------------------------------------------------------------------------

#[tokio::test]
async fn get_user_credential_refreshes_expired_oauth_and_persists() {
    let pool = pool().await;
    clean_db(pool.pool()).await;
    let db = pool.pool();

    let source_id = new_id();
    let user_id = new_id();
    let cred_id = new_id();

    seed_user(db, user_id).await;
    seed_source(db, source_id, "user", user_id).await;

    let (token_url, _server, captured) = spawn_token_endpoint().await;

    let repo = ServiceCredentialsRepo::new(db.clone()).unwrap();
    repo.create(make_oauth_creds(
        cred_id,
        source_id,
        Some(user_id),
        &token_url,
        Some("refresh-old"),
        Some("client-1"),
        Some(OffsetDateTime::now_utc() - time::Duration::seconds(60)), // expired
    ))
    .await
    .unwrap();

    let cs = CredentialService::new(pool.clone());
    let result = cs
        .get_user_credential(source_id, user_id)
        .await
        .unwrap()
        .expect("expected a credential");

    // Token values should be updated.
    assert_eq!(result.credentials["access_token"], "access-new");
    assert_eq!(result.credentials["refresh_token"], "refresh-new");
    assert_eq!(result.credentials["token_type"], "Bearer");
    assert!(result.expires_at.unwrap() > OffsetDateTime::now_utc());
    assert!(result.last_validated_at.is_some());

    // The form captured by the mock endpoint should have the expected fields.
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

    // Re-read from DB to confirm persistence.
    let from_db = repo.find_by_id(cred_id).await.unwrap().unwrap();
    assert_eq!(from_db.credentials["access_token"], "access-new");
}

#[tokio::test]
async fn get_org_credential_refreshes_expired_oauth_and_persists() {
    let pool = pool().await;
    clean_db(pool.pool()).await;
    let db = pool.pool();

    let source_id = new_id();
    let user_id = new_id();
    let cred_id = new_id();

    seed_user(db, user_id).await;
    seed_source(db, source_id, "org", user_id).await;

    let (token_url, _server, captured) = spawn_token_endpoint().await;

    let repo = ServiceCredentialsRepo::new(db.clone()).unwrap();
    repo.create(make_oauth_creds(
        cred_id,
        source_id,
        None, // org-wide
        &token_url,
        Some("refresh-old"),
        Some("client-1"),
        Some(OffsetDateTime::now_utc() - time::Duration::seconds(60)), // expired
    ))
    .await
    .unwrap();

    let cs = CredentialService::new(pool.clone());
    let result = cs
        .get_org_credential(source_id)
        .await
        .unwrap()
        .expect("expected an org credential");

    assert_eq!(result.credentials["access_token"], "access-new");
    assert!(result.expires_at.unwrap() > OffsetDateTime::now_utc());

    let form = captured.lock().await.clone().unwrap();
    assert_eq!(
        form.get("grant_type").map(String::as_str),
        Some("refresh_token")
    );
}

#[tokio::test]
async fn non_oauth_credential_not_refreshed() {
    let pool = pool().await;
    clean_db(pool.pool()).await;
    let db = pool.pool();

    let source_id = new_id();
    let user_id = new_id();
    let cred_id = new_id();

    seed_user(db, user_id).await;
    seed_source(db, source_id, "user", user_id).await;

    let repo = ServiceCredentialsRepo::new(db.clone()).unwrap();
    repo.create(make_jwt_creds(cred_id, source_id, Some(user_id)))
        .await
        .unwrap();

    let cs = CredentialService::new(pool.clone());
    let result = cs
        .get_user_credential(source_id, user_id)
        .await
        .unwrap()
        .expect("expected a credential");

    // Jwt credential should be returned as-is (no refresh attempted).
    assert_eq!(result.credentials["token"], "some-jwt");
    assert_eq!(result.auth_type, AuthType::Jwt);
}

#[tokio::test]
async fn credential_without_refresh_token_not_refreshed() {
    let pool = pool().await;
    clean_db(pool.pool()).await;
    let db = pool.pool();

    let source_id = new_id();
    let user_id = new_id();
    let cred_id = new_id();

    seed_user(db, user_id).await;
    seed_source(db, source_id, "user", user_id).await;

    let (token_url, _server, _captured) = spawn_token_endpoint().await;

    let repo = ServiceCredentialsRepo::new(db.clone()).unwrap();
    repo.create(make_oauth_creds(
        cred_id,
        source_id,
        Some(user_id),
        &token_url,
        None, // no refresh_token!
        Some("client-1"),
        Some(OffsetDateTime::now_utc() - time::Duration::seconds(60)), // expired
    ))
    .await
    .unwrap();

    let cs = CredentialService::new(pool.clone());
    let result = cs
        .get_user_credential(source_id, user_id)
        .await
        .unwrap()
        .expect("expected a credential");

    // Should be returned unchanged (old access token preserved).
    assert_eq!(result.credentials["access_token"], "access-old");
}

#[tokio::test]
async fn credential_without_client_id_not_refreshed() {
    let pool = pool().await;
    clean_db(pool.pool()).await;
    let db = pool.pool();

    let source_id = new_id();
    let user_id = new_id();
    let cred_id = new_id();

    seed_user(db, user_id).await;
    seed_source(db, source_id, "user", user_id).await;

    let (token_url, _server, _captured) = spawn_token_endpoint().await;

    let repo = ServiceCredentialsRepo::new(db.clone()).unwrap();
    repo.create(make_oauth_creds(
        cred_id,
        source_id,
        Some(user_id),
        &token_url,
        Some("refresh-old"),
        None, // no client_id!
        Some(OffsetDateTime::now_utc() - time::Duration::seconds(60)),
    ))
    .await
    .unwrap();

    let cs = CredentialService::new(pool.clone());
    let result = cs
        .get_user_credential(source_id, user_id)
        .await
        .unwrap()
        .expect("expected a credential");

    assert_eq!(result.credentials["access_token"], "access-old");
}

#[tokio::test]
async fn not_expired_credential_not_refreshed() {
    let pool = pool().await;
    clean_db(pool.pool()).await;
    let db = pool.pool();

    let source_id = new_id();
    let user_id = new_id();
    let cred_id = new_id();

    seed_user(db, user_id).await;
    seed_source(db, source_id, "user", user_id).await;

    let (token_url, _server, _captured) = spawn_token_endpoint().await;

    let repo = ServiceCredentialsRepo::new(db.clone()).unwrap();
    repo.create(make_oauth_creds(
        cred_id,
        source_id,
        Some(user_id),
        &token_url,
        Some("refresh-old"),
        Some("client-1"),
        Some(OffsetDateTime::now_utc() + time::Duration::hours(1)), // still valid
    ))
    .await
    .unwrap();

    let cs = CredentialService::new(pool.clone());
    let result = cs
        .get_user_credential(source_id, user_id)
        .await
        .unwrap()
        .expect("expected a credential");

    // Fresh credential should be returned unchanged.
    assert_eq!(result.credentials["access_token"], "access-old");
}

#[tokio::test]
async fn raw_credential_does_not_trigger_refresh() {
    let pool = pool().await;
    clean_db(pool.pool()).await;
    let db = pool.pool();

    let source_id = new_id();
    let user_id = new_id();
    let cred_id = new_id();

    seed_user(db, user_id).await;
    seed_source(db, source_id, "user", user_id).await;

    let (token_url, _server, _captured) = spawn_token_endpoint().await;

    let repo = ServiceCredentialsRepo::new(db.clone()).unwrap();
    repo.create(make_oauth_creds(
        cred_id,
        source_id,
        Some(user_id),
        &token_url,
        Some("refresh-old"),
        Some("client-1"),
        Some(OffsetDateTime::now_utc() - time::Duration::seconds(60)), // expired
    ))
    .await
    .unwrap();

    let cs = CredentialService::new(pool.clone());
    let result = cs
        .raw_user_credential(source_id, user_id)
        .await
        .unwrap()
        .expect("expected a credential");

    // Raw read should NOT trigger refresh; access_token stays old.
    assert_eq!(result.credentials["access_token"], "access-old");
}

// ---------------------------------------------------------------------------
// Test: ServiceCredentialsRepo CRUD (regression-safe for both branches)
// ---------------------------------------------------------------------------

#[tokio::test]
async fn repo_create_and_find_org_credential() {
    let pool = pool().await;
    clean_db(pool.pool()).await;
    let db = pool.pool();

    let source_id = new_id();
    let user_id = new_id();
    let cred_id = new_id();

    seed_user(db, user_id).await;
    seed_source(db, source_id, "org", user_id).await;

    let repo = ServiceCredentialsRepo::new(db.clone()).unwrap();
    repo.create(make_jwt_creds(cred_id, source_id, None))
        .await
        .unwrap();

    let found = repo
        .find_org_credential(source_id)
        .await
        .unwrap()
        .expect("expected org credential");
    assert!(found.user_id.is_none());
    assert_eq!(found.source_id, source_id);
}

#[tokio::test]
async fn repo_create_and_find_user_credential() {
    let pool = pool().await;
    clean_db(pool.pool()).await;
    let db = pool.pool();

    let source_id = new_id();
    let user_id = new_id();
    let cred_id = new_id();

    seed_user(db, user_id).await;
    seed_source(db, source_id, "org", user_id).await;

    let repo = ServiceCredentialsRepo::new(db.clone()).unwrap();
    repo.create(make_jwt_creds(cred_id, source_id, Some(user_id)))
        .await
        .unwrap();

    let found = repo
        .find_user_credential(source_id, user_id)
        .await
        .unwrap()
        .expect("expected user credential");
    assert_eq!(found.user_id.as_deref(), Some(user_id));
}

#[tokio::test]
async fn repo_update_credentials_persists_changes() {
    let pool = pool().await;
    clean_db(pool.pool()).await;
    let db = pool.pool();

    let source_id = new_id();
    let user_id = new_id();
    let cred_id = new_id();

    seed_user(db, user_id).await;
    seed_source(db, source_id, "user", user_id).await;

    let repo = ServiceCredentialsRepo::new(db.clone()).unwrap();
    let cred = make_jwt_creds(cred_id, source_id, Some(user_id));
    repo.create(cred).await.unwrap();

    // Update via the repo.
    let mut updated = repo.find_by_id(cred_id).await.unwrap().unwrap();
    updated.credentials = serde_json::json!({"token": "updated-jwt"});
    repo.update_credentials(&updated).await.unwrap();

    let from_db = repo.find_by_id(cred_id).await.unwrap().unwrap();
    assert_eq!(from_db.credentials["token"], "updated-jwt");
}

#[tokio::test]
async fn repo_delete_for_user_removes_only_user_row() {
    let pool = pool().await;
    clean_db(pool.pool()).await;
    let db = pool.pool();

    let source_id = new_id();
    let user_id = new_id();
    let cred_id_user = new_id();
    let cred_id_org = new_id();

    seed_user(db, user_id).await;
    seed_source(db, source_id, "org", user_id).await;

    let repo = ServiceCredentialsRepo::new(db.clone()).unwrap();
    repo.create(make_jwt_creds(cred_id_org, source_id, None))
        .await
        .unwrap();
    repo.create(make_jwt_creds(cred_id_user, source_id, Some(user_id)))
        .await
        .unwrap();

    repo.delete_for_user(source_id, user_id).await.unwrap();

    assert!(repo
        .find_user_credential(source_id, user_id)
        .await
        .unwrap()
        .is_none());
    assert!(repo.find_org_credential(source_id).await.unwrap().is_some());
}

#[tokio::test]
async fn repo_delete_by_source_id_cascades() {
    let pool = pool().await;
    clean_db(pool.pool()).await;
    let db = pool.pool();

    let source_id = new_id();
    let user1 = new_id();
    let user2 = new_id();

    seed_user(db, user1).await;
    seed_user(db, user2).await;
    seed_source(db, source_id, "org", user1).await;

    let repo = ServiceCredentialsRepo::new(db.clone()).unwrap();
    repo.create(make_jwt_creds("c1", source_id, None))
        .await
        .unwrap();
    repo.create(make_jwt_creds("c2", source_id, Some(user1)))
        .await
        .unwrap();
    repo.create(make_jwt_creds("c3", source_id, Some(user2)))
        .await
        .unwrap();

    repo.delete_by_source_id(source_id).await.unwrap();

    assert!(repo.find_org_credential(source_id).await.unwrap().is_none());
    assert!(repo
        .find_user_credential(source_id, user1)
        .await
        .unwrap()
        .is_none());
    assert!(repo
        .find_user_credential(source_id, user2)
        .await
        .unwrap()
        .is_none());
}

//! Integration tests for `CredentialService` — OAuth refresh with a real
//! Postgres database and a mock token endpoint.
//!
//! These tests verify that expired OAuth tokens are refreshed correctly
//! (HTTP call outside any DB transaction, updated fields persisted) and that
//! non-refreshable credentials pass through unchanged.
//!
//! # Test Postgres
//! Uses a long-lived container (`test-pg-test`). Create it with:
//! ```text
//! docker run -d --name test-pg-test \
//!   -e POSTGRES_DB=omni_test \
//!   -e POSTGRES_USER=omni \
//!   -e POSTGRES_PASSWORD=omni_password \
//!   paradedb/paradedb:0.24.0-pg17
//! ```
//! Run migrations once:
//! ```text
//! docker run --rm --network host \
//!   -e DATABASE_HOST=$(docker inspect test-pg-test --format '{{.NetworkSettings.IPAddress}}') \
//!   -e DATABASE_PORT=5432 \
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
// Setup
// ---------------------------------------------------------------------------

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

fn db_url() -> String {
    let host = std::env::var("TEST_PG_HOST").unwrap_or_else(|_| "172.17.0.7".into());
    let port = std::env::var("TEST_PG_PORT").unwrap_or_else(|_| "5432".into());
    format!("postgresql://omni:omni_password@{host}:{port}/omni_test")
}

async fn fresh_pool() -> DatabasePool {
    init_env();
    let pool = DatabasePool::new_with_options(&db_url(), 5, 30)
        .await
        .unwrap();
    sqlx::query("DELETE FROM service_credentials")
        .execute(pool.pool())
        .await
        .ok();
    sqlx::query("DELETE FROM sources")
        .execute(pool.pool())
        .await
        .ok();
    sqlx::query("DELETE FROM users")
        .execute(pool.pool())
        .await
        .ok();
    pool
}

fn uid() -> &'static str {
    Box::leak(shared::utils::generate_ulid().into_boxed_str())
}

async fn seed_user(pool: &sqlx::PgPool, id: &str) {
    sqlx::query(
        r#"INSERT INTO users (id, email, password_hash, created_at, updated_at)
           VALUES ($1, $2, 'hash', NOW(), NOW())"#,
    )
    .bind(id)
    .bind(format!("{id}@test.com"))
    .execute(pool)
    .await
    .unwrap();
}

async fn seed_source(pool: &sqlx::PgPool, id: &str, scope: &str, created_by: &str) {
    sqlx::query(
        r#"INSERT INTO sources (id, name, source_type, config, scope, is_active, created_by, created_at, updated_at)
           VALUES ($1, 'Test', 'google_drive', '{}', $2, true, $3, NOW(), NOW())"#,
    )
    .bind(id)
    .bind(scope)
    .bind(created_by)
    .execute(pool)
    .await
    .unwrap();
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

fn expired_oauth_creds(
    id: &str,
    source_id: &str,
    user_id: Option<&str>,
    token_uri: &str,
) -> ServiceCredential {
    make_creds(
        id,
        source_id,
        user_id,
        AuthType::OAuth,
        serde_json::json!({
            "access_token": "old-token",
            "refresh_token": "refresh-old",
            "client_id": "client-1",
            "token_uri": token_uri,
        }),
        Some(OffsetDateTime::now_utc() - time::Duration::seconds(60)),
    )
}

fn jwt_creds(id: &str, source_id: &str, user_id: Option<&str>) -> ServiceCredential {
    make_creds(
        id,
        source_id,
        user_id,
        AuthType::Jwt,
        serde_json::json!({"token": "jwt-value"}),
        None,
    )
}

// ── Mock OAuth server ─────────────────────────────────────────────────

type CapturedForm = Arc<TokioMutex<Option<HashMap<String, String>>>>;

async fn spawn_token_server() -> (String, CapturedForm) {
    let captured: CapturedForm = Arc::new(TokioMutex::new(None));

    async fn handler(
        State(captured): State<CapturedForm>,
        Form(form): Form<HashMap<String, String>>,
    ) -> Json<serde_json::Value> {
        *captured.lock().await = Some(form);
        Json(serde_json::json!({
            "access_token": "new-token",
            "refresh_token": "new-refresh",
            "token_type": "Bearer",
            "expires_in": 3600,
        }))
    }

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let app = Router::new()
        .route("/token", post(handler))
        .with_state(captured.clone());
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });

    (format!("http://{addr}/token"), captured)
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[tokio::test]
async fn refresh_flow_end_to_end() {
    // Tests the full OAuth refresh path for both user and org credentials:
    // seed → read (triggers refresh via CredentialService) → verify updated
    // tokens persisted → re-read from DB confirms durability.

    let pool = fresh_pool().await;
    let db = pool.pool();
    let (token_url, captured) = spawn_token_server().await;

    let repo = ServiceCredentialsRepo::new(db.clone()).unwrap();

    // ── Per-user credential ────────────────────────────────────────
    let usr = uid();
    let src_u = uid();
    let cred_u = uid();
    seed_user(db, usr).await;
    seed_source(db, src_u, "user", usr).await;
    repo.create(expired_oauth_creds(cred_u, src_u, Some(usr), &token_url))
        .await
        .unwrap();

    let cs = CredentialService::new(pool.clone());
    let result = cs.get_user_credential(src_u, usr).await.unwrap().unwrap();
    assert_eq!(result.credentials["access_token"], "new-token");
    assert_eq!(result.credentials["refresh_token"], "new-refresh");
    assert_eq!(result.credentials["token_type"], "Bearer");
    assert!(result.expires_at.unwrap() > OffsetDateTime::now_utc());
    assert!(result.last_validated_at.is_some());

    // Confirm persisted in DB.
    let from_db = repo.find_by_id(cred_u).await.unwrap().unwrap();
    assert_eq!(from_db.credentials["access_token"], "new-token");

    // ── Org-wide credential ─────────────────────────────────────────
    let src_o = uid();
    let cred_o = uid();
    seed_source(db, src_o, "org", usr).await;
    repo.create(expired_oauth_creds(cred_o, src_o, None, &token_url))
        .await
        .unwrap();

    let result = cs.get_org_credential(src_o).await.unwrap().unwrap();
    assert_eq!(result.credentials["access_token"], "new-token");

    // ── Verify captured request ─────────────────────────────────────
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
}

#[tokio::test]
async fn raw_read_does_not_trigger_refresh() {
    // `raw_user_credential` must return the stored credential without
    // attempting OAuth refresh.  The old-token proves no HTTP call was made.

    let pool = fresh_pool().await;
    let db = pool.pool();
    let (token_url, _captured) = spawn_token_server().await;

    let usr = uid();
    let src = uid();
    let cred = uid();
    seed_user(db, usr).await;
    seed_source(db, src, "user", usr).await;
    let repo = ServiceCredentialsRepo::new(db.clone()).unwrap();
    repo.create(expired_oauth_creds(cred, src, Some(usr), &token_url))
        .await
        .unwrap();

    let result = CredentialService::new(pool)
        .raw_user_credential(src, usr)
        .await
        .unwrap()
        .unwrap();

    assert_eq!(result.credentials["access_token"], "old-token");
}

#[tokio::test]
async fn repo_crud_round_trip() {
    // Regression: the stripped ServiceCredentialsRepo still performs
    // encrypted create, find, update correctly.

    let pool = fresh_pool().await;
    let db = pool.pool();

    let usr = uid();
    let src = uid();
    let cred = uid();
    seed_user(db, usr).await;
    seed_source(db, src, "user", usr).await;

    let repo = ServiceCredentialsRepo::new(db.clone()).unwrap();

    // Create
    repo.create(jwt_creds(cred, src, Some(usr))).await.unwrap();

    // Find
    let found = repo.find_by_id(cred).await.unwrap().unwrap();
    assert_eq!(found.user_id.as_deref(), Some(usr));
    assert_eq!(found.source_id, src);
    assert_eq!(found.credentials["token"], "jwt-value");

    // Update
    let mut updated = found;
    updated.credentials = serde_json::json!({"token": "updated-value"});
    repo.update_credentials(&updated).await.unwrap();
    let reloaded = repo.find_by_id(cred).await.unwrap().unwrap();
    assert_eq!(reloaded.credentials["token"], "updated-value");
}

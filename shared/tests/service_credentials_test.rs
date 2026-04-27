//! Tests for ServiceCredentialsRepo's per-user credential resolution rules.
//!
//! These map 1:1 onto the four cases in the lookup rule (see plan):
//!   * personal source                              → org row
//!   * org source, read,  per-user row exists       → per-user row
//!   * org source, read,  per-user row absent       → org row (fallback)
//!   * org source, write, per-user row exists       → per-user row
//!   * org source, write, per-user row absent       → NeedsUserAuth

#[cfg(test)]
mod tests {
    use shared::models::{AuthType, ServiceCredentials, ServiceProvider, SourceScope};
    use shared::test_environment::TestEnvironment;
    use shared::{ActionMode, CredentialResolutionError, ServiceCredentialsRepo};
    use sqlx::PgPool;
    use time::OffsetDateTime;

    const SEED_USER_ID: &str = "01JGF7V3E0Y2R1X8P5Q7W9T4N6";
    const SEED_SOURCE_ID: &str = "01JGF7V3E0Y2R1X8P5Q7W9T4N7";
    const OTHER_USER_ID: &str = "01JGF7V3E0Y2R1X8P5Q7W9T4U1";
    const ORG_SOURCE_ID: &str = "01JGF7V3E0Y2R1X8P5Q7W9T4O1";

    fn ensure_encryption_env() {
        std::env::set_var(
            "ENCRYPTION_KEY",
            "test_master_key_that_is_long_enough_32_chars",
        );
        std::env::set_var("ENCRYPTION_SALT", "test_salt_16_chars");
    }

    async fn seed_org_source(pool: &PgPool) {
        sqlx::query(
            r#"
            INSERT INTO users (id, email, password_hash, created_at, updated_at)
            VALUES ($1, 'other@example.com', 'hash', NOW(), NOW())
            ON CONFLICT (id) DO NOTHING
            "#,
        )
        .bind(OTHER_USER_ID)
        .execute(pool)
        .await
        .unwrap();

        sqlx::query(
            r#"
            INSERT INTO sources (id, name, source_type, config, scope, created_by, created_at, updated_at)
            VALUES ($1, 'Org Source', 'google_drive', '{}', 'org', $2, NOW(), NOW())
            ON CONFLICT (id) DO NOTHING
            "#,
        )
        .bind(ORG_SOURCE_ID)
        .bind(SEED_USER_ID)
        .execute(pool)
        .await
        .unwrap();
    }

    fn make_creds(
        id: &str,
        source_id: &str,
        user_id: Option<&str>,
        auth_type: AuthType,
    ) -> ServiceCredentials {
        let now = OffsetDateTime::now_utc();
        ServiceCredentials {
            id: id.to_string(),
            source_id: source_id.to_string(),
            user_id: user_id.map(|s| s.to_string()),
            provider: ServiceProvider::Google,
            auth_type,
            principal_email: Some("acct@example.com".into()),
            credentials: serde_json::json!({"access_token": "tok"}),
            config: serde_json::json!({}),
            expires_at: None,
            last_validated_at: None,
            created_at: now,
            updated_at: now,
        }
    }

    #[tokio::test]
    async fn personal_source_uses_org_row() {
        ensure_encryption_env();
        let env = TestEnvironment::new().await.unwrap();
        let repo = ServiceCredentialsRepo::new(env.db_pool.pool().clone()).unwrap();

        // Personal source already seeded; insert the lone org-row credential.
        repo.create(make_creds(
            "01CRED_PERSONAL_ORG",
            SEED_SOURCE_ID,
            None,
            AuthType::OAuth,
        ))
        .await
        .unwrap();

        let creds = repo
            .get_for_action(
                SEED_SOURCE_ID,
                SEED_USER_ID,
                SourceScope::User,
                ActionMode::Read,
            )
            .await
            .expect("expected the seed (org) row to be returned for a personal source");

        assert!(creds.user_id.is_none());
        assert_eq!(creds.source_id, SEED_SOURCE_ID);
    }

    #[tokio::test]
    async fn org_source_read_falls_back_to_org_row() {
        ensure_encryption_env();
        let env = TestEnvironment::new().await.unwrap();
        seed_org_source(env.db_pool.pool()).await;
        let repo = ServiceCredentialsRepo::new(env.db_pool.pool().clone()).unwrap();

        repo.create(make_creds(
            "01CRED_ORG_ORG",
            ORG_SOURCE_ID,
            None,
            AuthType::Jwt,
        ))
        .await
        .unwrap();

        let creds = repo
            .get_for_action(
                ORG_SOURCE_ID,
                SEED_USER_ID,
                SourceScope::Org,
                ActionMode::Read,
            )
            .await
            .expect("read on org source with no per-user cred should fall back to org row");

        assert!(creds.user_id.is_none());
    }

    #[tokio::test]
    async fn org_source_read_prefers_per_user_row_when_present() {
        ensure_encryption_env();
        let env = TestEnvironment::new().await.unwrap();
        seed_org_source(env.db_pool.pool()).await;
        let repo = ServiceCredentialsRepo::new(env.db_pool.pool().clone()).unwrap();

        repo.create(make_creds(
            "01CRED_ORG_ORG2",
            ORG_SOURCE_ID,
            None,
            AuthType::Jwt,
        ))
        .await
        .unwrap();
        repo.create(make_creds(
            "01CRED_ORG_PER_USER",
            ORG_SOURCE_ID,
            Some(SEED_USER_ID),
            AuthType::OAuth,
        ))
        .await
        .unwrap();

        let creds = repo
            .get_for_action(
                ORG_SOURCE_ID,
                SEED_USER_ID,
                SourceScope::Org,
                ActionMode::Read,
            )
            .await
            .expect("read should pick per-user row when it exists");

        assert_eq!(creds.user_id.as_deref(), Some(SEED_USER_ID));
    }

    #[tokio::test]
    async fn org_source_write_with_per_user_row_succeeds() {
        ensure_encryption_env();
        let env = TestEnvironment::new().await.unwrap();
        seed_org_source(env.db_pool.pool()).await;
        let repo = ServiceCredentialsRepo::new(env.db_pool.pool().clone()).unwrap();

        repo.create(make_creds(
            "01CRED_ORG_ORG3",
            ORG_SOURCE_ID,
            None,
            AuthType::Jwt,
        ))
        .await
        .unwrap();
        repo.create(make_creds(
            "01CRED_ORG_PER_USER2",
            ORG_SOURCE_ID,
            Some(SEED_USER_ID),
            AuthType::OAuth,
        ))
        .await
        .unwrap();

        let creds = repo
            .get_for_action(
                ORG_SOURCE_ID,
                SEED_USER_ID,
                SourceScope::Org,
                ActionMode::Write,
            )
            .await
            .expect("write with per-user row present should succeed");

        assert_eq!(creds.user_id.as_deref(), Some(SEED_USER_ID));
    }

    #[tokio::test]
    async fn org_source_write_without_per_user_row_returns_needs_user_auth() {
        ensure_encryption_env();
        let env = TestEnvironment::new().await.unwrap();
        seed_org_source(env.db_pool.pool()).await;
        let repo = ServiceCredentialsRepo::new(env.db_pool.pool().clone()).unwrap();

        // Org row exists but no per-user row for SEED_USER_ID.
        repo.create(make_creds(
            "01CRED_ORG_ORG4",
            ORG_SOURCE_ID,
            None,
            AuthType::Jwt,
        ))
        .await
        .unwrap();

        let result = repo
            .get_for_action(
                ORG_SOURCE_ID,
                SEED_USER_ID,
                SourceScope::Org,
                ActionMode::Write,
            )
            .await;

        match result {
            Err(CredentialResolutionError::NeedsUserAuth {
                source_id,
                provider,
            }) => {
                assert_eq!(source_id, ORG_SOURCE_ID);
                assert_eq!(provider, ServiceProvider::Google);
            }
            other => panic!("expected NeedsUserAuth, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn org_source_write_without_org_creds_returns_no_credentials() {
        ensure_encryption_env();
        let env = TestEnvironment::new().await.unwrap();
        seed_org_source(env.db_pool.pool()).await;
        let repo = ServiceCredentialsRepo::new(env.db_pool.pool().clone()).unwrap();

        // No credentials at all on the org source.
        let result = repo
            .get_for_action(
                ORG_SOURCE_ID,
                SEED_USER_ID,
                SourceScope::Org,
                ActionMode::Write,
            )
            .await;

        match result {
            Err(CredentialResolutionError::NoCredentials(id)) => {
                assert_eq!(id, ORG_SOURCE_ID);
            }
            other => panic!("expected NoCredentials, got {other:?}"),
        }
    }
}

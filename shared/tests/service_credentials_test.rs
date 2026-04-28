//! Tests for ServiceCredentialsRepo's basic getters. The org/user resolution
//! rules now live in connector-manager (see `resolve_credentials` in
//! `services/connector-manager/src/handlers.rs`); this suite only verifies the
//! repo's two raw getters.

#[cfg(test)]
mod tests {
    use shared::models::{AuthType, ServiceCredentials, ServiceProvider};
    use shared::test_environment::TestEnvironment;
    use shared::ServiceCredentialsRepo;
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
    async fn find_org_credential_returns_org_row() {
        ensure_encryption_env();
        let env = TestEnvironment::new().await.unwrap();
        let repo = ServiceCredentialsRepo::new(env.db_pool.pool().clone()).unwrap();

        repo.create(make_creds(
            "01CRED_PERSONAL_ORG",
            SEED_SOURCE_ID,
            None,
            AuthType::OAuth,
        ))
        .await
        .unwrap();

        let creds = repo
            .find_org_credential(SEED_SOURCE_ID)
            .await
            .unwrap()
            .expect("expected an org credential row");
        assert!(creds.user_id.is_none());
        assert_eq!(creds.source_id, SEED_SOURCE_ID);
    }

    #[tokio::test]
    async fn find_user_credential_returns_per_user_row() {
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
        repo.create(make_creds(
            "01CRED_ORG_PER_USER",
            ORG_SOURCE_ID,
            Some(SEED_USER_ID),
            AuthType::OAuth,
        ))
        .await
        .unwrap();

        let creds = repo
            .find_user_credential(ORG_SOURCE_ID, SEED_USER_ID)
            .await
            .unwrap()
            .expect("expected per-user row");
        assert_eq!(creds.user_id.as_deref(), Some(SEED_USER_ID));
    }

    #[tokio::test]
    async fn find_user_credential_returns_none_when_absent() {
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

        let creds = repo
            .find_user_credential(ORG_SOURCE_ID, SEED_USER_ID)
            .await
            .unwrap();
        assert!(creds.is_none());
    }

    #[tokio::test]
    async fn find_org_credential_returns_none_when_absent() {
        ensure_encryption_env();
        let env = TestEnvironment::new().await.unwrap();
        seed_org_source(env.db_pool.pool()).await;
        let repo = ServiceCredentialsRepo::new(env.db_pool.pool().clone()).unwrap();

        let creds = repo.find_org_credential(ORG_SOURCE_ID).await.unwrap();
        assert!(creds.is_none());
    }
}

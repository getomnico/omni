use anyhow::Result;
use serde_json::json;
use shared::test_environment::TestEnvironment;
use sqlx::Row;
use ulid::Ulid;

#[tokio::test]
async fn document_rls_enforces_permissions_and_resets_pooled_context() -> Result<()> {
    let env = TestEnvironment::new().await?;
    let pool = env.db_pool.pool();
    let source_id = "01JGF7V3E0Y2R1X8P5Q7W9T4N7";

    let alice_id = Ulid::new().to_string();
    let bob_id = Ulid::new().to_string();
    sqlx::query(
        "INSERT INTO users (id, email, password_hash) VALUES ($1, 'alice@example.com', 'hash'), ($2, 'bob@other.test', 'hash')",
    )
    .bind(&alice_id)
    .bind(&bob_id)
    .execute(pool)
    .await?;

    let group_id = Ulid::new().to_string();
    sqlx::query(
        "INSERT INTO groups (id, source_id, email, synced_at) VALUES ($1, $2, 'engineering@example.com', NOW())",
    )
    .bind(&group_id)
    .bind(source_id)
    .execute(pool)
    .await?;
    sqlx::query(
        "INSERT INTO group_memberships (id, group_id, member_email, synced_at) VALUES ($1, $2, 'alice@example.com', NOW())",
    )
    .bind(Ulid::new().to_string())
    .bind(&group_id)
    .execute(pool)
    .await?;

    let cases = [
        ("public", json!({"public": true, "users": [], "groups": []})),
        (
            "direct",
            json!({"public": false, "users": ["alice@example.com"], "groups": []}),
        ),
        (
            "domain",
            json!({"public": false, "users": [], "groups": ["example.com"]}),
        ),
        (
            "group",
            json!({"public": false, "users": [], "groups": ["engineering@example.com"]}),
        ),
        (
            "denied",
            json!({"public": false, "users": ["bob@other.test"], "groups": []}),
        ),
        ("legacy", json!([])),
    ];
    let mut ids = Vec::new();
    for (name, permissions) in cases {
        let id = Ulid::new().to_string();
        sqlx::query(
            "INSERT INTO documents (id, source_id, external_id, title, permissions, metadata, attributes) VALUES ($1, $2, $3, $3, $4, '{}', '{}')",
        )
        .bind(&id)
        .bind(source_id)
        .bind(format!("rls-{name}"))
        .bind(permissions)
        .execute(pool)
        .await?;
        ids.push((name, id));
    }
    let all_ids: Vec<String> = ids.iter().map(|(_, id)| id.clone()).collect();

    let relation = sqlx::query(
        "SELECT relrowsecurity, relforcerowsecurity FROM pg_class WHERE oid = 'documents'::regclass",
    )
    .fetch_one(pool)
    .await?;
    assert!(relation.get::<bool, _>("relrowsecurity"));
    assert!(relation.get::<bool, _>("relforcerowsecurity"));

    let runtime_role = sqlx::query(
        "SELECT rolsuper, rolbypassrls FROM pg_roles WHERE rolname = 'omni_documents_user'",
    )
    .fetch_one(pool)
    .await?;
    assert!(!runtime_role.get::<bool, _>("rolsuper"));
    assert!(!runtime_role.get::<bool, _>("rolbypassrls"));

    let mut alice = env
        .db_pool
        .begin_document_user("alice@example.com", false)
        .await?;
    let alice_visible: Vec<String> = sqlx::query_scalar(
        "SELECT external_id FROM documents WHERE id = ANY($1) ORDER BY external_id",
    )
    .bind(&all_ids)
    .fetch_all(&mut *alice)
    .await?;
    alice.commit().await?;
    assert_eq!(
        alice_visible,
        vec!["rls-direct", "rls-domain", "rls-group", "rls-public"]
    );

    let mut public = env
        .db_pool
        .begin_document_user("ignored@example.com", true)
        .await?;
    let public_visible: Vec<String> =
        sqlx::query_scalar("SELECT external_id FROM documents WHERE id = ANY($1)")
            .bind(&all_ids)
            .fetch_all(&mut *public)
            .await?;
    public.commit().await?;
    assert_eq!(public_visible, vec!["rls-public"]);

    let mut missing = pool.begin().await?;
    sqlx::query("SET LOCAL ROLE omni_documents_user")
        .execute(&mut *missing)
        .await?;
    let missing_count: i64 =
        sqlx::query_scalar("SELECT count(*) FROM documents WHERE id = ANY($1)")
            .bind(&all_ids)
            .fetch_one(&mut *missing)
            .await?;
    missing.commit().await?;
    assert_eq!(missing_count, 0, "missing context must fail closed");

    let mut forged = env
        .db_pool
        .begin_document_user("alice@example.com", false)
        .await?;
    sqlx::query("SELECT set_config('omni.document_access_scope', 'system', true)")
        .execute(&mut *forged)
        .await?;
    let forged_count: i64 = sqlx::query_scalar("SELECT count(*) FROM documents WHERE id = ANY($1)")
        .bind(&all_ids)
        .fetch_one(&mut *forged)
        .await?;
    forged.commit().await?;
    assert_eq!(forged_count, 0, "a user GUC cannot activate system access");

    let mut bob = env
        .db_pool
        .begin_document_user("bob@other.test", false)
        .await?;
    let bob_visible: Vec<String> = sqlx::query_scalar(
        "SELECT external_id FROM documents WHERE id = ANY($1) ORDER BY external_id",
    )
    .bind(&all_ids)
    .fetch_all(&mut *bob)
    .await?;
    bob.commit().await?;
    assert_eq!(bob_visible, vec!["rls-denied", "rls-public"]);

    let mut system = env.db_pool.begin_document_system().await?;
    let system_count: i64 = sqlx::query_scalar("SELECT count(*) FROM documents WHERE id = ANY($1)")
        .bind(&all_ids)
        .fetch_one(&mut *system)
        .await?;
    system.commit().await?;
    assert_eq!(system_count, all_ids.len() as i64);

    Ok(())
}

#[tokio::test]
async fn runtime_login_cannot_activate_system_role_and_related_tables_are_isolated() -> Result<()> {
    let env = TestEnvironment::new().await?;
    let pool = env.db_pool.pool();
    let source_id = "01JGF7V3E0Y2R1X8P5Q7W9T4N7";

    let alice_id = Ulid::new().to_string();
    sqlx::query(
        "INSERT INTO users (id, email, password_hash) VALUES ($1, 'alice@example.com', 'hash')",
    )
    .bind(&alice_id)
    .execute(pool)
    .await?;

    let doc_id = Ulid::new().to_string();
    let content_id = Ulid::new().to_string();
    sqlx::query(
        "INSERT INTO documents (id, source_id, external_id, title, content_id, permissions) VALUES ($1, $2, 'rls-alice', 'alice', $3, '{\"public\":false,\"users\":[\"alice@example.com\"],\"groups\":[]}')",
    )
    .bind(&doc_id)
    .bind(source_id)
    .bind(&content_id)
    .execute(pool)
    .await?;
    sqlx::query(
        "INSERT INTO content_blobs (id, content, size_bytes, storage_backend) VALUES ($1, $2, 5, 'postgres')",
    )
    .bind(&content_id)
    .bind("secret".as_bytes())
    .execute(pool)
    .await?;
    sqlx::query(
        "INSERT INTO embeddings (id, document_id, chunk_index, chunk_start_offset, chunk_end_offset, embedding, model_name, dimensions) VALUES ($1, $2, 0, 0, 5, $3, 'test-model', 2)",
    )
    .bind(Ulid::new().to_string())
    .bind(&doc_id)
    .bind(vec![0.1f32, 0.2f32])
    .execute(pool)
    .await?;

    let runtime = sqlx::query(
        "SELECT 1 FROM pg_roles WHERE rolname = 'omni_runtime' AND NOT EXISTS (
            SELECT 1 FROM pg_auth_members m JOIN pg_roles r ON r.oid = m.roleid
            WHERE m.member = (SELECT oid FROM pg_roles WHERE rolname = 'omni_runtime')
              AND r.rolname = 'omni_documents_system'
        )",
    )
    .fetch_optional(pool)
    .await?;
    assert!(
        runtime.is_some(),
        "omni_runtime must not be granted omni_documents_system"
    );

    let mut user = env
        .db_pool
        .begin_document_user("alice@example.com", false)
        .await?;
    let system_switch = sqlx::query("SET LOCAL ROLE omni_documents_system")
        .execute(&mut *user)
        .await;
    assert!(
        system_switch.is_err(),
        "a user-scoped connection must not be able to assume the system role"
    );
    user.rollback().await?;

    let mut alice = env
        .db_pool
        .begin_document_user("alice@example.com", false)
        .await?;
    let alice_docs: i64 = sqlx::query_scalar("SELECT count(*) FROM documents WHERE id = $1")
        .bind(&doc_id)
        .fetch_one(&mut *alice)
        .await?;
    let alice_blobs: i64 = sqlx::query_scalar("SELECT count(*) FROM content_blobs WHERE id = $1")
        .bind(&content_id)
        .fetch_one(&mut *alice)
        .await?;
    let alice_embeddings: i64 =
        sqlx::query_scalar("SELECT count(*) FROM embeddings WHERE document_id = $1")
            .bind(&doc_id)
            .fetch_one(&mut *alice)
            .await?;
    alice.commit().await?;
    assert_eq!(alice_docs, 1);
    assert_eq!(alice_blobs, 1);
    assert_eq!(alice_embeddings, 1);

    let mut bob = env
        .db_pool
        .begin_document_user("bob@other.test", false)
        .await?;
    let bob_blobs: i64 = sqlx::query_scalar("SELECT count(*) FROM content_blobs WHERE id = $1")
        .bind(&content_id)
        .fetch_one(&mut *bob)
        .await?;
    bob.commit().await?;
    assert_eq!(bob_blobs, 0, "denied documents must not expose their blobs");

    Ok(())
}

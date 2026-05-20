#[cfg(test)]
mod tests {
    use shared::db::repositories::SyncRunRepository;
    use shared::test_environment::TestEnvironment;
    use sqlx::PgPool;
    use time::{Duration, OffsetDateTime};

    const SEED_USER_ID: &str = "01JGF7V3E0Y2R1X8P5Q7W9T4N6";

    async fn insert_source(pool: &PgPool, id: &str, interval_seconds: i32) -> String {
        sqlx::query(
            r#"
            INSERT INTO sources
                (id, name, source_type, config, is_active, is_deleted,
                 sync_interval_seconds, created_by, created_at, updated_at)
            VALUES ($1, $2, 'local_files', '{}', TRUE, FALSE, $3, $4, NOW(), NOW())
            "#,
        )
        .bind(id)
        .bind(format!("source-{}", id))
        .bind(interval_seconds)
        .bind(SEED_USER_ID)
        .execute(pool)
        .await
        .unwrap();
        id.to_string()
    }

    async fn insert_run(
        pool: &PgPool,
        run_id: &str,
        source_id: &str,
        status: &str,
        completed_at: Option<OffsetDateTime>,
    ) {
        sqlx::query(
            r#"
            INSERT INTO sync_runs
                (id, source_id, sync_type, started_at, completed_at, status,
                 created_at, updated_at)
            VALUES ($1, $2, 'incremental', COALESCE($4, NOW()), $4, $3, NOW(), NOW())
            "#,
        )
        .bind(run_id)
        .bind(source_id)
        .bind(status)
        .bind(completed_at)
        .execute(pool)
        .await
        .unwrap();
    }

    fn ulid(seq: u32) -> String {
        format!("01TEST{:020}", seq)
    }

    #[tokio::test]
    async fn sync_runs_are_limited_per_source_newest_first() {
        let env = TestEnvironment::new().await.unwrap();
        let pool = env.db_pool.pool();
        let repo = SyncRunRepository::new(pool);

        let now = OffsetDateTime::now_utc();
        let source_id = insert_source(pool, &ulid(120), 60).await;

        for i in 0..12 {
            insert_run(
                pool,
                &ulid(121 + i),
                &source_id,
                "completed",
                Some(now - Duration::seconds(i as i64)),
            )
            .await;
        }

        let runs = repo.list_runs(&[source_id.clone()], 10).await.unwrap();

        assert_eq!(runs.len(), 10);
        assert!(runs.iter().all(|run| run.source_id == source_id));
        assert_eq!(runs[0].id, ulid(121));
        assert_eq!(runs[9].id, ulid(130));
    }
}

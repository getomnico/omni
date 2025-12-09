use crate::{
    db::error::DatabaseError,
    models::{SyncRun, SyncStatus, SyncType},
    utils::generate_ulid,
};
use sqlx::PgPool;
use time::OffsetDateTime;

#[derive(Clone)]
pub struct SyncRunRepository {
    pool: PgPool,
}

impl SyncRunRepository {
    pub fn new(pool: &PgPool) -> Self {
        Self { pool: pool.clone() }
    }

    pub async fn create(
        &self,
        source_id: &str,
        sync_type: SyncType,
    ) -> Result<SyncRun, DatabaseError> {
        let id = generate_ulid();
        let now = OffsetDateTime::now_utc();

        sqlx::query(
            "INSERT INTO sync_runs (id, source_id, sync_type, status)
             VALUES ($1, $2, $3, $4)",
        )
        .bind(&id)
        .bind(source_id)
        .bind(sync_type)
        .bind(SyncStatus::Running)
        .execute(&self.pool)
        .await?;

        sqlx::query("NOTIFY sync_run_update")
            .execute(&self.pool)
            .await?;

        Ok(SyncRun {
            id,
            source_id: source_id.to_string(),
            sync_type,
            status: SyncStatus::Running,
            created_at: now,
            updated_at: now,
            started_at: Some(now),
            completed_at: None,
            documents_processed: 0,
            documents_updated: 0,
            error_message: None,
        })
    }

    pub async fn find_by_id(&self, id: &str) -> Result<Option<SyncRun>, DatabaseError> {
        let sync_run = sqlx::query_as::<_, SyncRun>(
            r#"
            SELECT id, source_id, sync_type, started_at, completed_at, status,
                   documents_processed, documents_updated, error_message,
                   created_at, updated_at
            FROM sync_runs
            WHERE id = $1
            "#,
        )
        .bind(id)
        .fetch_optional(&self.pool)
        .await?;

        Ok(sync_run)
    }

    pub async fn mark_completed(
        &self,
        id: &str,
        documents_processed: i32,
        documents_updated: i32,
    ) -> Result<(), DatabaseError> {
        sqlx::query(
            "UPDATE sync_runs
             SET status = $1, completed_at = CURRENT_TIMESTAMP,
                 documents_processed = $2, documents_updated = $3,
                 updated_at = CURRENT_TIMESTAMP
             WHERE id = $4",
        )
        .bind(SyncStatus::Completed)
        .bind(documents_processed)
        .bind(documents_updated)
        .bind(id)
        .execute(&self.pool)
        .await?;

        sqlx::query("NOTIFY sync_run_update")
            .execute(&self.pool)
            .await?;

        Ok(())
    }

    pub async fn mark_failed(&self, id: &str, error: &str) -> Result<(), DatabaseError> {
        sqlx::query(
            "UPDATE sync_runs
             SET status = $1, completed_at = CURRENT_TIMESTAMP,
                 error_message = $2, updated_at = CURRENT_TIMESTAMP
             WHERE id = $3",
        )
        .bind(SyncStatus::Failed)
        .bind(error)
        .bind(id)
        .execute(&self.pool)
        .await?;

        sqlx::query("NOTIFY sync_run_update")
            .execute(&self.pool)
            .await?;

        Ok(())
    }

    pub async fn increment_progress(&self, id: &str) -> Result<(), DatabaseError> {
        sqlx::query(
            "UPDATE sync_runs
             SET documents_processed = documents_processed + 1,
                 updated_at = CURRENT_TIMESTAMP
             WHERE id = $1",
        )
        .bind(id)
        .execute(&self.pool)
        .await?;

        sqlx::query("NOTIFY sync_run_update")
            .execute(&self.pool)
            .await?;

        Ok(())
    }

    pub async fn get_last_completed_for_source(
        &self,
        source_id: &str,
        sync_type: SyncType,
    ) -> Result<Option<SyncRun>, DatabaseError> {
        let sync_run = sqlx::query_as::<_, SyncRun>(
            r#"
            SELECT id, source_id, sync_type, started_at, completed_at, status,
                   documents_processed, documents_updated, error_message,
                   created_at, updated_at
            FROM sync_runs
            WHERE source_id = $1 AND sync_type = $2 AND status = $3
            ORDER BY completed_at DESC
            LIMIT 1
            "#,
        )
        .bind(source_id)
        .bind(sync_type)
        .bind(SyncStatus::Completed)
        .fetch_optional(&self.pool)
        .await?;

        Ok(sync_run)
    }

    pub async fn get_running_for_source(
        &self,
        source_id: &str,
    ) -> Result<Option<SyncRun>, DatabaseError> {
        let sync_run = sqlx::query_as::<_, SyncRun>(
            r#"
            SELECT id, source_id, sync_type, started_at, completed_at, status,
                   documents_processed, documents_updated, error_message,
                   created_at, updated_at
            FROM sync_runs
            WHERE source_id = $1 AND status = $2
            ORDER BY created_at DESC
            LIMIT 1
            "#,
        )
        .bind(source_id)
        .bind(SyncStatus::Running)
        .fetch_optional(&self.pool)
        .await?;

        Ok(sync_run)
    }

    pub async fn find_all_running(&self) -> Result<Vec<SyncRun>, DatabaseError> {
        let sync_runs = sqlx::query_as::<_, SyncRun>(
            r#"
            SELECT id, source_id, sync_type, started_at, completed_at, status,
                   documents_processed, documents_updated, error_message,
                   created_at, updated_at
            FROM sync_runs
            WHERE status = $1
            ORDER BY created_at DESC
            "#,
        )
        .bind(SyncStatus::Running)
        .fetch_all(&self.pool)
        .await?;

        Ok(sync_runs)
    }
}

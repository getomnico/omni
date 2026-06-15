use crate::{
    db::error::DatabaseError,
    models::{SyncRun, SyncStatus, SyncType},
    utils::generate_ulid,
};
use sqlx::{Error as SqlxError, PgPool};
use time::OffsetDateTime;

const RUNNING_SYNC_SLOT_INDEX: &str = "idx_sync_runs_one_running_per_source_slot";

fn map_sqlx_error(error: SqlxError) -> DatabaseError {
    if let SqlxError::Database(db_error) = &error {
        if db_error.constraint() == Some(RUNNING_SYNC_SLOT_INDEX) {
            return DatabaseError::RunningSyncSlotConflict;
        }
    }
    DatabaseError::Connection(error)
}

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
        trigger_type: &str,
    ) -> Result<SyncRun, DatabaseError> {
        let id = generate_ulid();
        let now = OffsetDateTime::now_utc();

        sqlx::query(
            "INSERT INTO sync_runs (id, source_id, sync_type, status, trigger_type, queued_at, started_at, last_activity_at)
             VALUES ($1, $2, $3, $4, $5, $6, $6, $6)",
        )
        .bind(&id)
        .bind(source_id)
        .bind(sync_type)
        .bind(SyncStatus::Running)
        .bind(trigger_type)
        .bind(now)
        .execute(&self.pool)
        .await
        .map_err(map_sqlx_error)?;

        Ok(SyncRun {
            id,
            source_id: source_id.to_string(),
            sync_type,
            status: SyncStatus::Running,
            trigger_type: trigger_type.to_string(),
            created_at: now,
            updated_at: now,
            started_at: Some(now),
            completed_at: None,
            documents_scanned: 0,
            documents_processed: 0,
            documents_updated: 0,
            error_message: None,
            checkpoint: None,
        })
    }

    pub async fn find_by_id(&self, id: &str) -> Result<Option<SyncRun>, DatabaseError> {
        let sync_run = sqlx::query_as::<_, SyncRun>(
            r#"
            SELECT id, source_id, sync_type, started_at, completed_at, status, trigger_type,
                   documents_scanned, documents_processed, documents_updated, error_message,
                   checkpoint, created_at, updated_at
            FROM sync_runs
            WHERE id = $1
            "#,
        )
        .bind(id)
        .fetch_optional(&self.pool)
        .await?;

        Ok(sync_run)
    }

    /// Flip status to `Completed`. Counters are maintained by
    /// `increment_scanned` / `increment_updated`, so this only touches
    /// status fields. Prefer [`complete_and_publish_checkpoint`] for normal
    /// SDK completion so the successful checkpoint is atomically promoted to
    /// the source.
    pub async fn mark_completed(&self, id: &str) -> Result<bool, DatabaseError> {
        let result = sqlx::query(
            "UPDATE sync_runs
             SET status = $1, completed_at = CURRENT_TIMESTAMP,
                 updated_at = CURRENT_TIMESTAMP
             WHERE id = $2 AND status = $3",
        )
        .bind(SyncStatus::Completed)
        .bind(id)
        .bind(SyncStatus::Running)
        .execute(&self.pool)
        .await?;

        Ok(result.rows_affected() > 0)
    }

    pub async fn update_checkpoint(
        &self,
        id: &str,
        checkpoint: serde_json::Value,
    ) -> Result<bool, DatabaseError> {
        let result = sqlx::query(
            "UPDATE sync_runs
             SET checkpoint = $1,
                 last_activity_at = CURRENT_TIMESTAMP,
                 updated_at = CURRENT_TIMESTAMP
             WHERE id = $2 AND status = $3",
        )
        .bind(&checkpoint)
        .bind(id)
        .bind(SyncStatus::Running)
        .execute(&self.pool)
        .await?;

        Ok(result.rows_affected() > 0)
    }

    pub async fn complete_and_publish_checkpoint(&self, id: &str) -> Result<bool, DatabaseError> {
        let mut tx = self.pool.begin().await?;

        let row: Option<(String, Option<serde_json::Value>)> = sqlx::query_as(
            "SELECT source_id, checkpoint
             FROM sync_runs
             WHERE id = $1 AND status = $2
             FOR UPDATE",
        )
        .bind(id)
        .bind(SyncStatus::Running)
        .fetch_optional(&mut *tx)
        .await?;

        let Some((source_id, checkpoint)) = row else {
            tx.rollback().await?;
            return Ok(false);
        };

        sqlx::query(
            "UPDATE sync_runs
             SET status = $1, completed_at = CURRENT_TIMESTAMP,
                 updated_at = CURRENT_TIMESTAMP
             WHERE id = $2",
        )
        .bind(SyncStatus::Completed)
        .bind(id)
        .execute(&mut *tx)
        .await?;

        sqlx::query(
            "UPDATE sources
             SET checkpoint = $1,
                 updated_at = CURRENT_TIMESTAMP
             WHERE id = $2",
        )
        .bind(&checkpoint)
        .bind(&source_id)
        .execute(&mut *tx)
        .await?;

        tx.commit().await?;
        Ok(true)
    }

    pub async fn mark_failed(&self, id: &str, error: &str) -> Result<bool, DatabaseError> {
        let result = sqlx::query(
            "UPDATE sync_runs
             SET status = $1, completed_at = CURRENT_TIMESTAMP,
                 error_message = $2, updated_at = CURRENT_TIMESTAMP
             WHERE id = $3 AND status = $4",
        )
        .bind(SyncStatus::Failed)
        .bind(error)
        .bind(id)
        .bind(SyncStatus::Running)
        .execute(&self.pool)
        .await?;

        Ok(result.rows_affected() > 0)
    }

    pub async fn increment_scanned(&self, id: &str, count: i32) -> Result<bool, DatabaseError> {
        let result = sqlx::query(
            "UPDATE sync_runs
             SET documents_scanned = documents_scanned + $1,
                 last_activity_at = CURRENT_TIMESTAMP,
                 updated_at = CURRENT_TIMESTAMP
             WHERE id = $2 AND status = $3",
        )
        .bind(count)
        .bind(id)
        .bind(SyncStatus::Running)
        .execute(&self.pool)
        .await?;

        Ok(result.rows_affected() > 0)
    }

    pub async fn increment_updated(&self, id: &str, count: i32) -> Result<bool, DatabaseError> {
        let result = sqlx::query(
            "UPDATE sync_runs
             SET documents_updated = documents_updated + $1,
                 last_activity_at = CURRENT_TIMESTAMP,
                 updated_at = CURRENT_TIMESTAMP
             WHERE id = $2 AND status = $3",
        )
        .bind(count)
        .bind(id)
        .bind(SyncStatus::Running)
        .execute(&self.pool)
        .await?;

        Ok(result.rows_affected() > 0)
    }

    pub async fn increment_progress(&self, id: &str) -> Result<(), DatabaseError> {
        self.increment_progress_by(id, 1).await
    }

    pub async fn increment_progress_by(&self, id: &str, count: i32) -> Result<(), DatabaseError> {
        sqlx::query(
            "UPDATE sync_runs
             SET documents_processed = documents_processed + $1,
                 last_activity_at = CURRENT_TIMESTAMP,
                 updated_at = CURRENT_TIMESTAMP
             WHERE id = $2 AND status = $3",
        )
        .bind(count)
        .bind(id)
        .bind(SyncStatus::Running)
        .execute(&self.pool)
        .await?;

        Ok(())
    }

    pub async fn get_last_completed_for_source(
        &self,
        source_id: &str,
        sync_type: Option<SyncType>,
    ) -> Result<Option<SyncRun>, DatabaseError> {
        let sync_run = match sync_type {
            Some(st) => {
                sqlx::query_as::<_, SyncRun>(
                    r#"
                    SELECT id, source_id, sync_type, started_at, completed_at, status, trigger_type,
                           documents_scanned, documents_processed, documents_updated, error_message,
                   checkpoint, created_at, updated_at
                    FROM sync_runs
                    WHERE source_id = $1 AND sync_type = $2 AND status = $3
                    ORDER BY completed_at DESC
                    LIMIT 1
                    "#,
                )
                .bind(source_id)
                .bind(st)
                .bind(SyncStatus::Completed)
                .fetch_optional(&self.pool)
                .await?
            }
            None => {
                sqlx::query_as::<_, SyncRun>(
                    r#"
                    SELECT id, source_id, sync_type, started_at, completed_at, status, trigger_type,
                           documents_scanned, documents_processed, documents_updated, error_message,
                   checkpoint, created_at, updated_at
                    FROM sync_runs
                    WHERE source_id = $1 AND status = $2
                    ORDER BY completed_at DESC
                    LIMIT 1
                    "#,
                )
                .bind(source_id)
                .bind(SyncStatus::Completed)
                .fetch_optional(&self.pool)
                .await?
            }
        };

        Ok(sync_run)
    }

    pub async fn get_running_for_source(
        &self,
        source_id: &str,
    ) -> Result<Option<SyncRun>, DatabaseError> {
        let sync_run = sqlx::query_as::<_, SyncRun>(
            r#"
            SELECT id, source_id, sync_type, started_at, completed_at, status, trigger_type,
                   documents_scanned, documents_processed, documents_updated, error_message,
                   checkpoint, created_at, updated_at
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

    /// Most recent running sync for a source whose `sync_type` is in the
    /// supplied set. Used to ask "is a scheduled (Full/Incremental) sync
    /// running?" or "is realtime running?" independently, since one of each
    /// can run concurrently for the same source.
    pub async fn get_running_for_source_in_types(
        &self,
        source_id: &str,
        sync_types: &[SyncType],
    ) -> Result<Option<SyncRun>, DatabaseError> {
        if sync_types.is_empty() {
            return Ok(None);
        }
        let type_strs: Vec<String> = sync_types.iter().map(|t| t.to_string()).collect();
        let sync_run = sqlx::query_as::<_, SyncRun>(
            r#"
            SELECT id, source_id, sync_type, started_at, completed_at, status, trigger_type,
                   documents_scanned, documents_processed, documents_updated, error_message,
                   checkpoint, created_at, updated_at
            FROM sync_runs
            WHERE source_id = $1 AND status = $2 AND sync_type::text = ANY($3)
            ORDER BY created_at DESC
            LIMIT 1
            "#,
        )
        .bind(source_id)
        .bind(SyncStatus::Running)
        .bind(&type_strs)
        .fetch_optional(&self.pool)
        .await?;

        Ok(sync_run)
    }

    pub async fn find_all_running(&self) -> Result<Vec<SyncRun>, DatabaseError> {
        let sync_runs = sqlx::query_as::<_, SyncRun>(
            r#"
            SELECT id, source_id, sync_type, started_at, completed_at, status, trigger_type,
                   documents_scanned, documents_processed, documents_updated, error_message,
                   checkpoint, created_at, updated_at
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

    /// Fetch all running sync runs whose source_id is in the supplied list.
    pub async fn find_running_for_sources(
        &self,
        source_ids: &[String],
    ) -> Result<Vec<SyncRun>, DatabaseError> {
        if source_ids.is_empty() {
            return Ok(vec![]);
        }
        let sync_runs = sqlx::query_as::<_, SyncRun>(
            r#"
            SELECT id, source_id, sync_type, started_at, completed_at, status, trigger_type,
                   documents_scanned, documents_processed, documents_updated, error_message,
                   checkpoint, created_at, updated_at
            FROM sync_runs
            WHERE status = $1 AND source_id = ANY($2)
            ORDER BY created_at DESC
            "#,
        )
        .bind(SyncStatus::Running)
        .bind(source_ids)
        .fetch_all(&self.pool)
        .await?;

        Ok(sync_runs)
    }

    /// Mark a batch of sync runs as cancelled, returning the IDs that were
    /// actually transitioned (i.e., were still in `running` state).
    pub async fn mark_cancelled_many(
        &self,
        ids: &[String],
        message: &str,
    ) -> Result<Vec<String>, DatabaseError> {
        if ids.is_empty() {
            return Ok(vec![]);
        }
        let rows: Vec<(String,)> = sqlx::query_as(
            "UPDATE sync_runs
             SET status = $1, completed_at = CURRENT_TIMESTAMP,
                 error_message = $2, updated_at = CURRENT_TIMESTAMP
             WHERE id = ANY($3) AND status = $4
             RETURNING id",
        )
        .bind(SyncStatus::Cancelled)
        .bind(message)
        .bind(ids)
        .bind(SyncStatus::Running)
        .fetch_all(&self.pool)
        .await?;

        Ok(rows.into_iter().map(|(id,)| id).collect())
    }

    pub async fn count_running_in_slot_class(
        &self,
        slot_class: crate::models::SyncSlotClass,
    ) -> Result<i64, DatabaseError> {
        let sync_types: &[SyncType] = match slot_class {
            crate::models::SyncSlotClass::Realtime => &[SyncType::Realtime],
            crate::models::SyncSlotClass::Scheduled => &[SyncType::Full, SyncType::Incremental],
        };
        let type_strs: Vec<String> = sync_types.iter().map(|t| t.to_string()).collect();
        let count = sqlx::query_scalar::<_, i64>(
            "SELECT COUNT(*) FROM sync_runs WHERE status = $1 AND sync_type::text = ANY($2)",
        )
        .bind(SyncStatus::Running)
        .bind(&type_strs)
        .fetch_one(&self.pool)
        .await?;

        Ok(count)
    }

    pub async fn mark_cancelled(&self, id: &str) -> Result<bool, DatabaseError> {
        self.mark_cancelled_with_message(id, "Cancelled by user")
            .await
    }

    pub async fn mark_cancelled_with_message(
        &self,
        id: &str,
        message: &str,
    ) -> Result<bool, DatabaseError> {
        let result = sqlx::query(
            "UPDATE sync_runs
             SET status = $1, completed_at = CURRENT_TIMESTAMP,
                 error_message = $2, updated_at = CURRENT_TIMESTAMP
             WHERE id = $3 AND status = $4",
        )
        .bind(SyncStatus::Cancelled)
        .bind(message)
        .bind(id)
        .bind(SyncStatus::Running)
        .execute(&self.pool)
        .await?;

        Ok(result.rows_affected() > 0)
    }

    pub async fn update_activity(&self, id: &str) -> Result<bool, DatabaseError> {
        let result = sqlx::query(
            "UPDATE sync_runs
             SET last_activity_at = CURRENT_TIMESTAMP, updated_at = CURRENT_TIMESTAMP
             WHERE id = $1 AND status = $2",
        )
        .bind(id)
        .bind(SyncStatus::Running)
        .execute(&self.pool)
        .await?;

        Ok(result.rows_affected() > 0)
    }

    pub async fn find_latest_for_sources(
        &self,
        source_ids: &[String],
    ) -> Result<Vec<SyncRun>, DatabaseError> {
        let sync_runs = sqlx::query_as::<_, SyncRun>(
            r#"
            SELECT DISTINCT ON (source_id)
                   id, source_id, sync_type, started_at, completed_at, status, trigger_type,
                   documents_scanned, documents_processed, documents_updated, error_message,
                   checkpoint, created_at, updated_at
            FROM sync_runs
            WHERE source_id = ANY($1)
            ORDER BY source_id, started_at DESC
            "#,
        )
        .bind(source_ids)
        .fetch_all(&self.pool)
        .await?;

        Ok(sync_runs)
    }

    pub async fn list_runs(
        &self,
        source_ids: &[String],
        limit_per_source: i64,
    ) -> Result<Vec<SyncRun>, DatabaseError> {
        self.list_runs_for_sync_types(source_ids, &[], limit_per_source)
            .await
    }

    pub async fn list_runs_for_sync_types(
        &self,
        source_ids: &[String],
        sync_types: &[SyncType],
        limit_per_source: i64,
    ) -> Result<Vec<SyncRun>, DatabaseError> {
        if source_ids.is_empty() {
            return Ok(Vec::new());
        }

        let type_strs: Vec<String> = sync_types.iter().map(|t| t.to_string()).collect();
        let sync_runs = sqlx::query_as::<_, SyncRun>(
            r#"
            SELECT id, source_id, sync_type, started_at, completed_at, status, trigger_type,
                   documents_scanned, documents_processed, documents_updated, error_message,
                   NULL::jsonb AS checkpoint, created_at, updated_at
            FROM (
                SELECT sr.id, sr.source_id, sr.sync_type, sr.started_at, sr.completed_at,
                       sr.status, sr.trigger_type, sr.documents_scanned, sr.documents_processed,
                       sr.documents_updated, sr.error_message, sr.created_at, sr.updated_at,
                       ROW_NUMBER() OVER (
                           PARTITION BY source_id
                           ORDER BY started_at DESC, created_at DESC
                       ) AS rn
                FROM sync_runs sr
                WHERE source_id = ANY($1)
                  AND (cardinality($2::text[]) = 0 OR sync_type::text = ANY($2))
            ) ranked
            WHERE rn <= $3
            ORDER BY source_id, started_at DESC, created_at DESC
            "#,
        )
        .bind(source_ids)
        .bind(&type_strs)
        .bind(limit_per_source)
        .fetch_all(&self.pool)
        .await?;

        Ok(sync_runs)
    }
}

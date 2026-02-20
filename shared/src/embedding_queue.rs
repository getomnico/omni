use anyhow::Result;
use serde::{Deserialize, Serialize};
use sqlx::{PgPool, Postgres, Row, Transaction};
use ulid::Ulid;

use crate::db::repositories::EmbeddingProviderRepository;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum EmbeddingQueueStatus {
    Pending,
    Processing,
    Completed,
    Failed,
}

impl std::fmt::Display for EmbeddingQueueStatus {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            EmbeddingQueueStatus::Pending => write!(f, "pending"),
            EmbeddingQueueStatus::Processing => write!(f, "processing"),
            EmbeddingQueueStatus::Completed => write!(f, "completed"),
            EmbeddingQueueStatus::Failed => write!(f, "failed"),
        }
    }
}

impl std::str::FromStr for EmbeddingQueueStatus {
    type Err = anyhow::Error;

    fn from_str(s: &str) -> Result<Self> {
        match s {
            "pending" => Ok(EmbeddingQueueStatus::Pending),
            "processing" => Ok(EmbeddingQueueStatus::Processing),
            "completed" => Ok(EmbeddingQueueStatus::Completed),
            "failed" => Ok(EmbeddingQueueStatus::Failed),
            _ => Err(anyhow::anyhow!("Invalid embedding queue status: {}", s)),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EmbeddingQueueItem {
    pub id: String,
    pub document_id: String,
    pub status: EmbeddingQueueStatus,
    pub retry_count: i32,
    pub error_message: Option<String>,
    pub created_at: sqlx::types::time::OffsetDateTime,
    pub updated_at: sqlx::types::time::OffsetDateTime,
    pub processed_at: Option<sqlx::types::time::OffsetDateTime>,
}

impl sqlx::FromRow<'_, sqlx::postgres::PgRow> for EmbeddingQueueItem {
    fn from_row(row: &sqlx::postgres::PgRow) -> Result<Self, sqlx::Error> {
        use sqlx::Row;

        let status_str: String = row.try_get("status")?;
        let status =
            status_str
                .parse::<EmbeddingQueueStatus>()
                .map_err(|e| sqlx::Error::ColumnDecode {
                    index: "status".to_string(),
                    source: e.into(),
                })?;

        Ok(EmbeddingQueueItem {
            id: row.try_get("id")?,
            document_id: row.try_get("document_id")?,
            status,
            retry_count: row.try_get("retry_count")?,
            error_message: row.try_get("error_message")?,
            created_at: row.try_get("created_at")?,
            updated_at: row.try_get("updated_at")?,
            processed_at: row.try_get("processed_at")?,
        })
    }
}

#[derive(Clone)]
pub struct EmbeddingQueue {
    pool: PgPool,
    provider_repo: EmbeddingProviderRepository,
}

impl EmbeddingQueue {
    pub fn new(pool: PgPool) -> Self {
        let provider_repo = EmbeddingProviderRepository::new(&pool);
        Self {
            pool,
            provider_repo,
        }
    }

    pub async fn enqueue(&self, document_id: String) -> Result<Option<String>> {
        if !self.provider_repo.has_active_provider().await? {
            return Ok(None);
        }

        let id = Ulid::new().to_string();

        let result = sqlx::query(
            r#"
            INSERT INTO embedding_queue (id, document_id)
            SELECT $1, $2
            WHERE NOT EXISTS (
                SELECT 1 FROM embedding_queue
                WHERE document_id = $2 AND status IN ('pending', 'processing')
            )
            "#,
        )
        .bind(&id)
        .bind(&document_id)
        .execute(&self.pool)
        .await?;

        if result.rows_affected() > 0 {
            Ok(Some(id))
        } else {
            Ok(None)
        }
    }

    pub async fn enqueue_batch(&self, document_ids: Vec<String>) -> Result<Vec<String>> {
        if !self.provider_repo.has_active_provider().await? {
            return Ok(vec![]);
        }

        let mut tx = self.pool.begin().await?;
        let mut ids = Vec::new();

        for document_id in document_ids {
            let id = Ulid::new().to_string();

            let result = sqlx::query(
                r#"
                INSERT INTO embedding_queue (id, document_id)
                SELECT $1, $2
                WHERE NOT EXISTS (
                    SELECT 1 FROM embedding_queue
                    WHERE document_id = $2 AND status IN ('pending', 'processing')
                )
                "#,
            )
            .bind(&id)
            .bind(&document_id)
            .execute(&mut *tx)
            .await?;

            if result.rows_affected() > 0 {
                ids.push(id);
            }
        }

        tx.commit().await?;
        Ok(ids)
    }

    pub async fn dequeue_batch(&self, batch_size: i32) -> Result<Vec<EmbeddingQueueItem>> {
        let items = sqlx::query_as::<_, EmbeddingQueueItem>(
            r#"
            UPDATE embedding_queue
            SET status = $2,
                updated_at = CURRENT_TIMESTAMP,
                processing_started_at = CURRENT_TIMESTAMP
            WHERE id IN (
                SELECT id
                FROM embedding_queue
                WHERE status = $3
                   OR (status = $4 AND retry_count < 3)
                ORDER BY created_at
                LIMIT $1
                FOR UPDATE SKIP LOCKED
            )
            RETURNING *
            "#,
        )
        .bind(batch_size)
        .bind(EmbeddingQueueStatus::Processing.to_string())
        .bind(EmbeddingQueueStatus::Pending.to_string())
        .bind(EmbeddingQueueStatus::Failed.to_string())
        .fetch_all(&self.pool)
        .await?;

        Ok(items)
    }

    pub async fn mark_completed(&self, ids: &[String]) -> Result<()> {
        sqlx::query(
            r#"
            UPDATE embedding_queue
            SET status = $2,
                processed_at = CURRENT_TIMESTAMP,
                updated_at = CURRENT_TIMESTAMP
            WHERE id = ANY($1)
            "#,
        )
        .bind(ids)
        .bind(EmbeddingQueueStatus::Completed.to_string())
        .execute(&self.pool)
        .await?;

        Ok(())
    }

    pub async fn mark_failed(&self, id: &str, error: &str) -> Result<()> {
        sqlx::query(
            r#"
            UPDATE embedding_queue
            SET status = $3,
                error_message = $2,
                retry_count = retry_count + 1,
                updated_at = CURRENT_TIMESTAMP
            WHERE id = $1
            "#,
        )
        .bind(id)
        .bind(error)
        .bind(EmbeddingQueueStatus::Failed.to_string())
        .execute(&self.pool)
        .await?;

        Ok(())
    }

    pub async fn mark_failed_batch(&self, ids: &[String], error: &str) -> Result<()> {
        sqlx::query(
            r#"
            UPDATE embedding_queue
            SET status = $3,
                error_message = $2,
                retry_count = retry_count + 1,
                updated_at = CURRENT_TIMESTAMP
            WHERE id = ANY($1)
            "#,
        )
        .bind(ids)
        .bind(error)
        .bind(EmbeddingQueueStatus::Failed.to_string())
        .execute(&self.pool)
        .await?;

        Ok(())
    }

    pub async fn recover_stale_processing_items(&self, timeout_seconds: i32) -> Result<i64> {
        let result = sqlx::query(
            r#"
            UPDATE embedding_queue
            SET status = $2,
                processing_started_at = NULL,
                updated_at = CURRENT_TIMESTAMP
            WHERE status = $3
            AND processing_started_at < CURRENT_TIMESTAMP - INTERVAL '1 second' * $1
            "#,
        )
        .bind(timeout_seconds)
        .bind(EmbeddingQueueStatus::Pending.to_string())
        .bind(EmbeddingQueueStatus::Processing.to_string())
        .execute(&self.pool)
        .await?;

        let recovered_count = result.rows_affected() as i64;
        if recovered_count > 0 {
            tracing::info!(
                "Recovered {} stale embedding processing items (timeout: {}s)",
                recovered_count,
                timeout_seconds
            );
        }

        Ok(recovered_count)
    }

    pub async fn cleanup_completed(&self, days_old: i32) -> Result<i64> {
        let result = sqlx::query(
            r#"
            DELETE FROM embedding_queue
            WHERE status = $2
              AND processed_at < CURRENT_TIMESTAMP - INTERVAL '1 day' * $1
            "#,
        )
        .bind(days_old)
        .bind(EmbeddingQueueStatus::Completed.to_string())
        .execute(&self.pool)
        .await?;

        Ok(result.rows_affected() as i64)
    }

    pub async fn cleanup_failed(&self, days_old: i32) -> Result<i64> {
        let result = sqlx::query(
            r#"
            DELETE FROM embedding_queue
            WHERE status = $2
              AND retry_count >= 3
              AND updated_at < CURRENT_TIMESTAMP - INTERVAL '1 day' * $1
            "#,
        )
        .bind(days_old)
        .bind(EmbeddingQueueStatus::Failed.to_string())
        .execute(&self.pool)
        .await?;

        Ok(result.rows_affected() as i64)
    }

    pub async fn get_queue_stats(&self) -> Result<QueueStats> {
        let row = sqlx::query(
            r#"
            SELECT 
                COUNT(*) FILTER (WHERE status = $1) as pending,
                COUNT(*) FILTER (WHERE status = $2) as processing,
                COUNT(*) FILTER (WHERE status = $3) as completed,
                COUNT(*) FILTER (WHERE status = $4) as failed
            FROM embedding_queue
            "#,
        )
        .bind(EmbeddingQueueStatus::Pending.to_string())
        .bind(EmbeddingQueueStatus::Processing.to_string())
        .bind(EmbeddingQueueStatus::Completed.to_string())
        .bind(EmbeddingQueueStatus::Failed.to_string())
        .fetch_one(&self.pool)
        .await?;

        Ok(QueueStats {
            pending: row.try_get::<i64, _>("pending").unwrap_or(0),
            processing: row.try_get::<i64, _>("processing").unwrap_or(0),
            completed: row.try_get::<i64, _>("completed").unwrap_or(0),
            failed: row.try_get::<i64, _>("failed").unwrap_or(0),
        })
    }
}

#[derive(Debug, Serialize)]
pub struct QueueStats {
    pub pending: i64,
    pub processing: i64,
    pub completed: i64,
    pub failed: i64,
}

pub async fn update_document_embedding_status(
    tx: &mut Transaction<'_, Postgres>,
    document_id: &str,
    status: &str,
) -> Result<()> {
    sqlx::query(
        r#"
        UPDATE documents
        SET embedding_status = $2,
            updated_at = CURRENT_TIMESTAMP
        WHERE id = $1
        "#,
    )
    .bind(document_id)
    .bind(status)
    .execute(&mut **tx)
    .await?;

    Ok(())
}

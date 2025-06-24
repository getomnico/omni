use anyhow::Result;
use serde::{Deserialize, Serialize};
use sqlx::{PgPool, Postgres, Row, Transaction};
use ulid::Ulid;

#[derive(Debug, Clone, Serialize, Deserialize, sqlx::FromRow)]
pub struct EmbeddingQueueItem {
    pub id: String,
    pub document_id: String,
    pub content: String,
    pub status: String,
    pub retry_count: i32,
    pub error_message: Option<String>,
    pub created_at: sqlx::types::time::OffsetDateTime,
    pub updated_at: sqlx::types::time::OffsetDateTime,
    pub processed_at: Option<sqlx::types::time::OffsetDateTime>,
}

#[derive(Clone)]
pub struct EmbeddingQueue {
    pool: PgPool,
}

impl EmbeddingQueue {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }

    pub async fn enqueue(&self, document_id: String, content: String) -> Result<String> {
        let id = Ulid::new().to_string();

        sqlx::query("INSERT INTO embedding_queue (id, document_id, content) VALUES ($1, $2, $3)")
            .bind(&id)
            .bind(&document_id)
            .bind(&content)
            .execute(&self.pool)
            .await?;

        Ok(id)
    }

    pub async fn enqueue_batch(&self, items: Vec<(String, String)>) -> Result<Vec<String>> {
        let mut tx = self.pool.begin().await?;
        let mut ids = Vec::new();

        for (document_id, content) in items {
            let id = Ulid::new().to_string();

            sqlx::query(
                "INSERT INTO embedding_queue (id, document_id, content) VALUES ($1, $2, $3)",
            )
            .bind(&id)
            .bind(&document_id)
            .bind(&content)
            .execute(&mut *tx)
            .await?;

            ids.push(id);
        }

        tx.commit().await?;
        Ok(ids)
    }

    pub async fn dequeue_batch(&self, batch_size: i32) -> Result<Vec<EmbeddingQueueItem>> {
        let items = sqlx::query_as::<_, EmbeddingQueueItem>(
            r#"
            UPDATE embedding_queue
            SET status = 'processing',
                updated_at = CURRENT_TIMESTAMP
            WHERE id IN (
                SELECT id
                FROM embedding_queue
                WHERE status = 'pending'
                   OR (status = 'failed' AND retry_count < 3)
                ORDER BY created_at
                LIMIT $1
                FOR UPDATE SKIP LOCKED
            )
            RETURNING *
            "#,
        )
        .bind(batch_size)
        .fetch_all(&self.pool)
        .await?;

        Ok(items)
    }

    pub async fn mark_completed(&self, ids: &[String]) -> Result<()> {
        sqlx::query(
            r#"
            UPDATE embedding_queue
            SET status = 'completed',
                processed_at = CURRENT_TIMESTAMP,
                updated_at = CURRENT_TIMESTAMP
            WHERE id = ANY($1)
            "#,
        )
        .bind(ids)
        .execute(&self.pool)
        .await?;

        Ok(())
    }

    pub async fn mark_failed(&self, id: &str, error: &str) -> Result<()> {
        sqlx::query(
            r#"
            UPDATE embedding_queue
            SET status = 'failed',
                error_message = $2,
                retry_count = retry_count + 1,
                updated_at = CURRENT_TIMESTAMP
            WHERE id = $1
            "#,
        )
        .bind(id)
        .bind(error)
        .execute(&self.pool)
        .await?;

        Ok(())
    }

    pub async fn mark_failed_batch(&self, ids: &[String], error: &str) -> Result<()> {
        sqlx::query(
            r#"
            UPDATE embedding_queue
            SET status = 'failed',
                error_message = $2,
                retry_count = retry_count + 1,
                updated_at = CURRENT_TIMESTAMP
            WHERE id = ANY($1)
            "#,
        )
        .bind(ids)
        .bind(error)
        .execute(&self.pool)
        .await?;

        Ok(())
    }

    pub async fn cleanup_completed(&self, days_old: i32) -> Result<i64> {
        let result = sqlx::query(
            r#"
            DELETE FROM embedding_queue
            WHERE status = 'completed'
              AND processed_at < CURRENT_TIMESTAMP - INTERVAL '1 day' * $1
            "#,
        )
        .bind(days_old)
        .execute(&self.pool)
        .await?;

        Ok(result.rows_affected() as i64)
    }

    pub async fn get_queue_stats(&self) -> Result<QueueStats> {
        let row = sqlx::query(
            r#"
            SELECT 
                COUNT(*) FILTER (WHERE status = 'pending') as pending,
                COUNT(*) FILTER (WHERE status = 'processing') as processing,
                COUNT(*) FILTER (WHERE status = 'completed') as completed,
                COUNT(*) FILTER (WHERE status = 'failed') as failed
            FROM embedding_queue
            "#,
        )
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

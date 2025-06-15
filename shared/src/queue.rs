use anyhow::Result;
use sqlx::{PgPool, Row};
use ulid::Ulid;

use crate::models::{ConnectorEvent, ConnectorEventQueueItem};

pub struct EventQueue {
    pool: PgPool,
}

impl EventQueue {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }

    pub async fn enqueue(&self, source_id: &str, event: &ConnectorEvent) -> Result<String> {
        let id = Ulid::new().to_string();
        let event_type = match event {
            ConnectorEvent::DocumentCreated { .. } => "document_created",
            ConnectorEvent::DocumentUpdated { .. } => "document_updated",
            ConnectorEvent::DocumentDeleted { .. } => "document_deleted",
        };

        let mut tx = self.pool.begin().await?;

        sqlx::query(
            r#"
            INSERT INTO connector_events_queue (id, source_id, event_type, payload)
            VALUES ($1, $2, $3, $4)
            "#,
        )
        .bind(&id)
        .bind(source_id)
        .bind(event_type)
        .bind(serde_json::to_value(event)?)
        .execute(&mut *tx)
        .await?;

        sqlx::query("NOTIFY indexer_queue")
            .execute(&mut *tx)
            .await?;

        tx.commit().await?;

        Ok(id)
    }

    pub async fn dequeue_batch(&self, batch_size: i32) -> Result<Vec<ConnectorEventQueueItem>> {
        let rows = sqlx::query(
            r#"
            WITH batch AS (
                SELECT id
                FROM connector_events_queue
                WHERE status = 'pending'
                ORDER BY created_at
                LIMIT $1
                FOR UPDATE SKIP LOCKED
            )
            UPDATE connector_events_queue q
            SET status = 'processing'
            FROM batch
            WHERE q.id = batch.id
            RETURNING 
                q.id,
                q.source_id,
                q.event_type,
                q.payload,
                q.status,
                q.retry_count,
                q.max_retries,
                q.created_at,
                q.processed_at,
                q.error_message
            "#,
        )
        .bind(batch_size)
        .fetch_all(&self.pool)
        .await?;

        let mut events = Vec::new();
        for row in rows {
            let status_str: String = row.get("status");
            let status = match status_str.as_str() {
                "pending" => crate::models::EventStatus::Pending,
                "processing" => crate::models::EventStatus::Processing,
                "completed" => crate::models::EventStatus::Completed,
                "failed" => crate::models::EventStatus::Failed,
                "dead_letter" => crate::models::EventStatus::DeadLetter,
                _ => crate::models::EventStatus::Pending,
            };

            events.push(ConnectorEventQueueItem {
                id: row.get("id"),
                source_id: row.get("source_id"),
                event_type: row.get("event_type"),
                payload: row.get("payload"),
                status,
                retry_count: row.get("retry_count"),
                max_retries: row.get("max_retries"),
                created_at: row.get("created_at"),
                processed_at: row.get("processed_at"),
                error_message: row.get("error_message"),
            });
        }

        Ok(events)
    }

    pub async fn mark_completed(&self, event_id: &str) -> Result<()> {
        sqlx::query(
            r#"
            UPDATE connector_events_queue
            SET status = 'completed', processed_at = NOW()
            WHERE id = $1
            "#,
        )
        .bind(event_id)
        .execute(&self.pool)
        .await?;

        Ok(())
    }

    pub async fn mark_failed(&self, event_id: &str, error: &str) -> Result<()> {
        let result = sqlx::query(
            r#"
            UPDATE connector_events_queue
            SET 
                retry_count = retry_count + 1,
                error_message = $2,
                status = CASE 
                    WHEN retry_count + 1 >= max_retries THEN 'dead_letter'
                    ELSE 'failed'
                END
            WHERE id = $1
            RETURNING retry_count, max_retries
            "#,
        )
        .bind(event_id)
        .bind(error)
        .fetch_one(&self.pool)
        .await?;

        let retry_count: i32 = result.get("retry_count");
        let max_retries: i32 = result.get("max_retries");

        if retry_count >= max_retries {
            tracing::error!(
                "Event {} moved to dead letter queue after {} retries",
                event_id,
                retry_count
            );
        }

        Ok(())
    }

    pub async fn retry_failed_events(&self) -> Result<i64> {
        let result = sqlx::query(
            r#"
            UPDATE connector_events_queue
            SET status = 'pending'
            WHERE status = 'failed'
            AND retry_count < max_retries
            AND created_at > NOW() - INTERVAL '24 hours'
            "#,
        )
        .execute(&self.pool)
        .await?;

        Ok(result.rows_affected() as i64)
    }

    pub async fn get_queue_stats(&self) -> Result<QueueStats> {
        let rows = sqlx::query(
            r#"
            SELECT 
                status,
                COUNT(*) as count
            FROM connector_events_queue
            WHERE created_at > NOW() - INTERVAL '24 hours'
            GROUP BY status
            "#,
        )
        .fetch_all(&self.pool)
        .await?;

        let mut pending = 0;
        let mut processing = 0;
        let mut completed = 0;
        let mut failed = 0;
        let mut dead_letter = 0;

        for row in rows {
            let status: String = row.get("status");
            let count: i64 = row.get("count");

            match status.as_str() {
                "pending" => pending = count,
                "processing" => processing = count,
                "completed" => completed = count,
                "failed" => failed = count,
                "dead_letter" => dead_letter = count,
                _ => {}
            }
        }

        Ok(QueueStats {
            pending,
            processing,
            completed,
            failed,
            dead_letter,
        })
    }

    pub async fn cleanup_old_events(&self, retention_days: i32) -> Result<CleanupResult> {
        let mut tx = self.pool.begin().await?;

        // Delete completed events older than retention period
        let completed_result = sqlx::query(
            r#"
            DELETE FROM connector_events_queue
            WHERE status = 'completed'
            AND processed_at < NOW() - INTERVAL '1 day' * $1
            "#,
        )
        .bind(retention_days)
        .execute(&mut *tx)
        .await?;

        // Delete dead letter events older than retention period (they're unlikely to be retried)
        let dead_letter_result = sqlx::query(
            r#"
            DELETE FROM connector_events_queue
            WHERE status = 'dead_letter'
            AND created_at < NOW() - INTERVAL '1 day' * $1
            "#,
        )
        .bind(retention_days)
        .execute(&mut *tx)
        .await?;

        // Run VACUUM to reclaim space (this will run after the transaction commits)
        tx.commit().await?;

        // VACUUM cannot run inside a transaction, so we run it separately
        sqlx::query("VACUUM ANALYZE connector_events_queue")
            .execute(&self.pool)
            .await?;

        Ok(CleanupResult {
            completed_deleted: completed_result.rows_affected(),
            dead_letter_deleted: dead_letter_result.rows_affected(),
        })
    }
}

#[derive(Debug, serde::Serialize)]
pub struct QueueStats {
    pub pending: i64,
    pub processing: i64,
    pub completed: i64,
    pub failed: i64,
    pub dead_letter: i64,
}

#[derive(Debug, serde::Serialize)]
pub struct CleanupResult {
    pub completed_deleted: u64,
    pub dead_letter_deleted: u64,
}

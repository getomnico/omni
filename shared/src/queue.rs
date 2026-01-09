use anyhow::Result;
use sqlx::{PgPool, Row};
use ulid::Ulid;

use crate::models::{ConnectorEvent, ConnectorEventQueueItem};

#[derive(Clone)]
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
            INSERT INTO connector_events_queue (id, sync_run_id, source_id, event_type, payload)
            VALUES ($1, $2, $3, $4, $5)
            "#,
        )
        .bind(&id)
        .bind(event.sync_run_id())
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
        // Dequeue events from a single sync_run (the one with most pending events)
        // This ensures each batch contains events from only one sync_run
        let rows = sqlx::query(
            r#"
            WITH target_sync_run AS (
                SELECT sync_run_id
                FROM connector_events_queue
                WHERE status = 'pending'
                GROUP BY sync_run_id
                ORDER BY COUNT(*) DESC
                LIMIT 1
            ),
            batch AS (
                SELECT id
                FROM connector_events_queue
                WHERE status = 'pending'
                  AND sync_run_id = (SELECT sync_run_id FROM target_sync_run)
                ORDER BY created_at
                LIMIT $1
                FOR UPDATE SKIP LOCKED
            )
            UPDATE connector_events_queue q
            SET status = 'processing',
                processing_started_at = NOW()
            FROM batch
            WHERE q.id = batch.id
            RETURNING
                q.id,
                q.sync_run_id,
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
                sync_run_id: row.get("sync_run_id"),
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

    pub async fn recover_stale_processing_items(&self, timeout_seconds: i32) -> Result<i64> {
        let result = sqlx::query(
            r#"
            UPDATE connector_events_queue
            SET status = 'pending',
                processing_started_at = NULL
            WHERE status = 'processing'
            AND processing_started_at < NOW() - INTERVAL '1 second' * $1
            "#,
        )
        .bind(timeout_seconds)
        .execute(&self.pool)
        .await?;

        let recovered_count = result.rows_affected() as i64;
        if recovered_count > 0 {
            tracing::info!(
                "Recovered {} stale processing items (timeout: {}s)",
                recovered_count,
                timeout_seconds
            );
        }

        Ok(recovered_count)
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

    pub async fn get_pending_count(&self) -> Result<i64> {
        let row: (i64,) =
            sqlx::query_as("SELECT COUNT(*) FROM connector_events_queue WHERE status = 'pending'")
                .fetch_one(&self.pool)
                .await?;
        Ok(row.0)
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

    // Batch operations for improved performance
    pub async fn mark_events_completed_batch(&self, event_ids: Vec<String>) -> Result<i64> {
        if event_ids.is_empty() {
            return Ok(0);
        }

        let result = sqlx::query(
            r#"
            UPDATE connector_events_queue
            SET status = 'completed', processed_at = NOW()
            WHERE id = ANY($1)
            "#,
        )
        .bind(&event_ids)
        .execute(&self.pool)
        .await?;

        Ok(result.rows_affected() as i64)
    }

    pub async fn mark_events_failed_batch(
        &self,
        event_ids_with_errors: Vec<(String, String)>,
    ) -> Result<i64> {
        if event_ids_with_errors.is_empty() {
            return Ok(0);
        }

        let event_ids: Vec<String> = event_ids_with_errors
            .iter()
            .map(|(id, _)| id.clone())
            .collect();
        let error_messages: Vec<String> = event_ids_with_errors
            .iter()
            .map(|(_, err)| err.clone())
            .collect();

        let result = sqlx::query(
            r#"
            UPDATE connector_events_queue
            SET status = 'failed',
                retry_count = retry_count + 1,
                error_message = data_table.error_message,
                processed_at = NOW()
            FROM (
                SELECT * FROM UNNEST($1::text[], $2::text[]) AS t(id, error_message)
            ) AS data_table
            WHERE connector_events_queue.id = data_table.id
            "#,
        )
        .bind(&event_ids)
        .bind(&error_messages)
        .execute(&self.pool)
        .await?;

        Ok(result.rows_affected() as i64)
    }

    pub async fn mark_events_dead_letter_batch(
        &self,
        event_ids_with_errors: Vec<(String, String)>,
    ) -> Result<i64> {
        if event_ids_with_errors.is_empty() {
            return Ok(0);
        }

        let event_ids: Vec<String> = event_ids_with_errors
            .iter()
            .map(|(id, _)| id.clone())
            .collect();
        let error_messages: Vec<String> = event_ids_with_errors
            .iter()
            .map(|(_, err)| err.clone())
            .collect();

        let result = sqlx::query(
            r#"
            UPDATE connector_events_queue
            SET status = CASE 
                    WHEN retry_count >= max_retries THEN 'dead_letter'
                    ELSE 'failed'
                END,
                retry_count = retry_count + 1,
                error_message = data_table.error_message,
                processed_at = NOW()
            FROM (
                SELECT * FROM UNNEST($1::text[], $2::text[]) AS t(id, error_message)
            ) AS data_table
            WHERE connector_events_queue.id = data_table.id
            "#,
        )
        .bind(&event_ids)
        .bind(&error_messages)
        .execute(&self.pool)
        .await?;

        Ok(result.rows_affected() as i64)
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

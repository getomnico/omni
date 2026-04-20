use crate::db::error::DatabaseError;
use serde::Serialize;
use sqlx::{FromRow, PgPool, Row};

/// Represents an orphan blob ready for deletion
#[derive(Debug, FromRow)]
pub struct OrphanBlob {
    pub id: String,
    pub size_bytes: i64,
}

/// Statistics about orphaned content blobs
#[derive(Debug, Serialize)]
pub struct OrphanStats {
    /// Orphans not yet marked (new this cycle)
    pub unmarked_orphans: i64,
    /// Orphans marked but not yet expired
    pub pending_orphans: i64,
    /// Orphans ready for deletion (past retention period)
    pub expired_orphans: i64,
    /// Total size of orphaned blobs in bytes
    pub orphan_size_bytes: i64,
}

pub struct ContentBlobRepository {
    pool: PgPool,
}

impl ContentBlobRepository {
    pub fn new(pool: &PgPool) -> Self {
        Self { pool: pool.clone() }
    }

    /// Mark blobs as orphaned if they are not referenced by any document
    /// or any pending/processing queue event.
    /// Returns the number of blobs marked.
    ///
    /// Bounded to MARK_ORPHANS_BATCH rows per call: the previous unbounded
    /// `NOT IN` anti-joins against the full `content_blobs` table materialized
    /// hash tables over every row and took 30+ hours to complete on production
    /// data (5M+ blobs). NOT EXISTS plus an explicit row cap lets the planner
    /// use an indexed anti-join and keeps each GC pass predictable — the
    /// remainder gets picked up on the next scheduled tick.
    pub async fn mark_orphans(&self) -> Result<i64, DatabaseError> {
        const MARK_ORPHANS_BATCH: i64 = 100_000;

        let result = sqlx::query(
            r#"
            WITH candidates AS (
                SELECT cb.id
                FROM content_blobs cb
                WHERE cb.orphaned_at IS NULL
                  AND NOT EXISTS (
                      SELECT 1 FROM documents d WHERE d.content_id = cb.id
                  )
                  AND NOT EXISTS (
                      SELECT 1 FROM connector_events_queue q
                      WHERE q.status IN ('pending', 'processing')
                        AND q.payload->>'content_id' = cb.id::text
                  )
                LIMIT $1
            )
            UPDATE content_blobs cb
            SET orphaned_at = CURRENT_TIMESTAMP
            FROM candidates
            WHERE cb.id = candidates.id
            "#,
        )
        .bind(MARK_ORPHANS_BATCH)
        .execute(&self.pool)
        .await?;

        Ok(result.rows_affected() as i64)
    }

    /// Unmark blobs that are no longer orphaned (got re-referenced).
    /// Returns the number of blobs unmarked.
    pub async fn unmark_non_orphans(&self) -> Result<i64, DatabaseError> {
        let result = sqlx::query(
            r#"
            UPDATE content_blobs cb
            SET orphaned_at = NULL
            WHERE cb.orphaned_at IS NOT NULL
              AND (
                  EXISTS (
                      SELECT 1 FROM documents d WHERE d.content_id = cb.id
                  )
                  OR EXISTS (
                      SELECT 1 FROM connector_events_queue q
                      WHERE q.status IN ('pending', 'processing')
                        AND q.payload->>'content_id' = cb.id::text
                  )
              )
            "#,
        )
        .execute(&self.pool)
        .await?;

        Ok(result.rows_affected() as i64)
    }

    /// Fetch a batch of expired orphans ready for deletion.
    /// Uses FOR UPDATE SKIP LOCKED to avoid blocking concurrent operations.
    pub async fn fetch_expired_orphans(
        &self,
        retention_days: i32,
        batch_size: i32,
    ) -> Result<Vec<OrphanBlob>, DatabaseError> {
        let rows = sqlx::query_as::<_, OrphanBlob>(
            r#"
            SELECT id, size_bytes
            FROM content_blobs
            WHERE orphaned_at IS NOT NULL
            AND orphaned_at < CURRENT_TIMESTAMP - INTERVAL '1 day' * $1
            ORDER BY orphaned_at
            LIMIT $2
            FOR UPDATE SKIP LOCKED
            "#,
        )
        .bind(retention_days)
        .bind(batch_size)
        .fetch_all(&self.pool)
        .await?;

        Ok(rows)
    }

    /// Get statistics about orphaned content blobs.
    pub async fn get_orphan_stats(
        &self,
        retention_days: i32,
    ) -> Result<OrphanStats, DatabaseError> {
        let row = sqlx::query(
            r#"
            SELECT
                COUNT(*) FILTER (
                    WHERE orphaned_at IS NULL
                    AND id NOT IN (
                        SELECT DISTINCT content_id FROM documents WHERE content_id IS NOT NULL
                    )
                    AND id NOT IN (
                        SELECT DISTINCT payload->>'content_id'
                        FROM connector_events_queue
                        WHERE status IN ('pending', 'processing')
                        AND payload->>'content_id' IS NOT NULL
                    )
                ) as unmarked_orphans,
                COUNT(*) FILTER (
                    WHERE orphaned_at IS NOT NULL
                    AND orphaned_at >= CURRENT_TIMESTAMP - INTERVAL '1 day' * $1
                ) as pending_orphans,
                COUNT(*) FILTER (
                    WHERE orphaned_at IS NOT NULL
                    AND orphaned_at < CURRENT_TIMESTAMP - INTERVAL '1 day' * $1
                ) as expired_orphans,
                COALESCE(SUM(size_bytes) FILTER (WHERE orphaned_at IS NOT NULL), 0) as orphan_size_bytes
            FROM content_blobs
            "#,
        )
        .bind(retention_days)
        .fetch_one(&self.pool)
        .await?;

        Ok(OrphanStats {
            unmarked_orphans: row.get("unmarked_orphans"),
            pending_orphans: row.get("pending_orphans"),
            expired_orphans: row.get("expired_orphans"),
            orphan_size_bytes: row.get("orphan_size_bytes"),
        })
    }
}

use anyhow::Result;
use shared::connector_event_queue::EventQueue;
use shared::db::error::DatabaseError;
use shared::db::repositories::{SyncRunRepository, person::SOURCE_MUTATION_LOCK_NAMESPACE};
use shared::models::{ConnectorEvent, SyncType};
use shared::task_queue::TaskQueue;
use sqlx::PgPool;
use tracing::{debug, error, info};

const BATCH_SIZE: i64 = 500;

pub struct SourceCleanup;

impl SourceCleanup {
    pub async fn cleanup_deleted_sources(pool: &PgPool) {
        let deleted_sources: Vec<(String,)> =
            match sqlx::query_as("SELECT id FROM sources WHERE is_deleted = true")
                .fetch_all(pool)
                .await
            {
                Ok(sources) => sources,
                Err(e) => {
                    error!("Failed to query deleted sources: {}", e);
                    return;
                }
            };

        if deleted_sources.is_empty() {
            return;
        }

        debug!(
            "Found {} deleted sources to clean up",
            deleted_sources.len()
        );

        for (source_id,) in &deleted_sources {
            if let Err(e) = cleanup_source(pool, source_id).await {
                error!("Failed to clean up source {}: {}", source_id, e);
            }
        }
    }
}

async fn cleanup_source(pool: &PgPool, source_id: &str) -> Result<()> {
    // Stop connector-owned runs immediately. Any in-flight SDK emission will
    // subsequently fail trusted-context validation; cleanup runs are internal
    // queue producers and do not use the SDK handlers.
    sqlx::query(
        r#"
        UPDATE sync_runs
        SET status = 'cancelled', completed_at = NOW(), updated_at = NOW(),
            error_message = 'Source was deleted'
        WHERE source_id = $1 AND status = 'running'
          AND trigger_type <> 'source_cleanup'
        "#,
    )
    .bind(source_id)
    .execute(pool)
    .await?;

    // Coordinate the pending/source-data decision and physical deletion with
    // every event mutation. SDK emission holds the same advisory lock for the
    // whole enqueue transaction, so while this transaction runs no new events
    // can be admitted for the source. Do not hold this transaction while
    // creating a cleanup run through another pooled connection.
    let mut tx = pool.begin().await?;
    sqlx::query("SELECT pg_advisory_xact_lock($1, hashtext($2))")
        .bind(SOURCE_MUTATION_LOCK_NAMESPACE)
        .bind(source_id)
        .execute(&mut *tx)
        .await?;

    // Retry-pending events must never be retried after source deletion. Fresh
    // pending emissions remain eligible so accepted events settle before the
    // source and its documents are removed.
    let retry_pending_ids: Vec<String> = sqlx::query_scalar(
        r#"
        SELECT id
        FROM tasks
        WHERE task_type = 'connector_event'
          AND payload ->> 'source_id' = $1
          AND status = 'pending'
          AND attempt_count > 0
        ORDER BY id
        "#,
    )
    .bind(source_id)
    .fetch_all(&mut *tx)
    .await?;
    if !retry_pending_ids.is_empty() {
        TaskQueue::new(pool.clone())
            .dead_letter_pending_with_executor(
                &mut *tx,
                &retry_pending_ids,
                "Source deleted before event retry",
            )
            .await?;
    }

    // Quiesce every admitted event type before touching documents or the
    // source. A running event can fail after this pass and become retry-pending;
    // the next cleanup pass cancels it before deletion.

    let event_unresolved: bool = sqlx::query_scalar(
        r#"
        SELECT EXISTS (
            SELECT 1
            FROM tasks q
            WHERE q.task_type = 'connector_event'
              AND q.payload ->> 'source_id' = $1
              AND q.status IN ('pending', 'running')
        )
        "#,
    )
    .bind(source_id)
    .fetch_one(&mut *tx)
    .await?;
    if event_unresolved {
        // Commit (not rollback) so the dead-lettered failed events stay
        // dead-lettered while the remaining events settle.
        tx.commit().await?;
        return Ok(());
    }

    // No document/group/person events are in flight, so their writes are
    // settled. Delete documents now — completed events already wrote them and
    // cannot reappear. Do not write searcher-owned agent_capabilities from
    // connector-manager; stale source-scoped capabilities are pruned by the
    // existing AI/searcher capability sync path.
    let result = sqlx::query(
        r#"
        WITH batch AS (
            SELECT id FROM documents WHERE source_id = $1 LIMIT $2
        )
        DELETE FROM documents WHERE id IN (SELECT id FROM batch)
        "#,
    )
    .bind(source_id)
    .bind(BATCH_SIZE)
    .execute(&mut *tx)
    .await?;

    if result.rows_affected() > 0 {
        tx.commit().await?;
        info!(
            "Cleaned up {} documents for deleted source {}",
            result.rows_affected(),
            source_id
        );
        return Ok(());
    }

    let has_source_people: bool =
        sqlx::query_scalar("SELECT EXISTS (SELECT 1 FROM people WHERE source_data ? $1)")
            .bind(source_id)
            .fetch_one(&mut *tx)
            .await?;
    if !has_source_people {
        // The indexer removed this source's embedded provenance before source
        // deletion. The shared lock prevents a person mutation racing this check.
        sqlx::query("DELETE FROM sources WHERE id = $1")
            .bind(source_id)
            .execute(&mut *tx)
            .await?;
        tx.commit().await?;
        info!("Deleted source row for fully cleaned source {}", source_id);
        return Ok(());
    }
    tx.rollback().await?;

    let sync_runs = SyncRunRepository::new(pool);
    let run = match sync_runs
        .create(source_id, SyncType::Full, "source_cleanup")
        .await
    {
        Ok(run) => run,
        Err(DatabaseError::RunningSyncSlotConflict) => return Ok(()),
        Err(error) => return Err(error.into()),
    };
    let emails: Vec<String> = sqlx::query_scalar(
        "SELECT email FROM people WHERE source_data ? $1 ORDER BY email LIMIT $2",
    )
    .bind(source_id)
    .bind(BATCH_SIZE)
    .fetch_all(pool)
    .await?;
    let events: Vec<ConnectorEvent> = emails
        .into_iter()
        .map(|email| ConnectorEvent::PersonDeleted {
            sync_run_id: run.id.clone(),
            source_id: source_id.to_string(),
            email,
        })
        .collect();
    if let Err(error) = EventQueue::new(pool.clone())
        .enqueue_batch(source_id, &events)
        .await
    {
        let _ = sync_runs.mark_failed(&run.id, &error.to_string()).await;
        return Err(error);
    }
    sync_runs.mark_completed(&run.id).await?;
    info!(
        "Queued person deletions before deleting source {}",
        source_id
    );
    Ok(())
}

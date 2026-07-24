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

async fn cleanup_source(pool: &PgPool, source_id: &str) -> Result<(), sqlx::Error> {
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
    .execute(pool)
    .await?;

    if result.rows_affected() > 0 {
        info!(
            "Cleaned up {} documents for deleted source {}",
            result.rows_affected(),
            source_id
        );
        return Ok(());
    }

    // No documents left. Do not write searcher-owned agent_capabilities from
    // connector-manager; stale source-scoped capabilities are pruned by the
    // existing AI/searcher capability sync path.

    // Safe to delete the source row (cascades to sync_runs, credentials, etc.)
    sqlx::query("DELETE FROM sources WHERE id = $1")
        .bind(source_id)
        .execute(pool)
        .await?;

    info!("Deleted source row for fully cleaned source {}", source_id);
    Ok(())
}

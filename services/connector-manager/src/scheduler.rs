use crate::config::ConnectorManagerConfig;
use crate::models::TriggerType;
use crate::sync_manager::{SyncError, SyncManager};
use sqlx::PgPool;
use std::sync::Arc;
use time::OffsetDateTime;
use tokio::time::{interval, Duration};
use tracing::{debug, error, info, warn};

pub struct Scheduler {
    pool: PgPool,
    config: ConnectorManagerConfig,
    sync_manager: Arc<SyncManager>,
}

impl Scheduler {
    pub fn new(
        pool: PgPool,
        config: ConnectorManagerConfig,
        sync_manager: Arc<SyncManager>,
    ) -> Self {
        Self {
            pool,
            config,
            sync_manager,
        }
    }

    pub async fn run(&self) {
        let mut scheduler_interval =
            interval(Duration::from_secs(self.config.scheduler_interval_seconds));

        info!(
            "Scheduler started, checking every {} seconds",
            self.config.scheduler_interval_seconds
        );

        loop {
            scheduler_interval.tick().await;
            self.tick().await;
        }
    }

    async fn tick(&self) {
        debug!("Scheduler tick");

        // Check for sources due for sync
        if let Err(e) = self.process_due_sources().await {
            error!("Error processing due sources: {}", e);
        }

        // Detect and handle stale syncs
        match self.sync_manager.detect_stale_syncs().await {
            Ok(stale) => {
                if !stale.is_empty() {
                    info!("Marked {} stale syncs as failed", stale.len());
                }
            }
            Err(e) => {
                error!("Error detecting stale syncs: {}", e);
            }
        }
    }

    async fn process_due_sources(&self) -> Result<(), SchedulerError> {
        let now = OffsetDateTime::now_utc();

        // Find sources where next_sync_at is due
        let due_sources: Vec<DueSource> = sqlx::query_as(
            r#"
            SELECT id, name, source_type::text as source_type
            FROM sources
            WHERE is_active = true
              AND is_deleted = false
              AND next_sync_at IS NOT NULL
              AND next_sync_at <= $1
            ORDER BY next_sync_at ASC
            LIMIT 10
            "#,
        )
        .bind(now)
        .fetch_all(&self.pool)
        .await
        .map_err(|e| SchedulerError::DatabaseError(e.to_string()))?;

        if due_sources.is_empty() {
            debug!("No sources due for sync");
            return Ok(());
        }

        info!("Found {} sources due for sync", due_sources.len());

        for source in due_sources {
            // Skip if already syncing
            if self.sync_manager.is_sync_running(&source.id) {
                debug!("Source {} is already syncing, skipping", source.id);
                continue;
            }

            // Try to trigger sync
            match self
                .sync_manager
                .trigger_sync(
                    &source.id,
                    Some("incremental".to_string()),
                    TriggerType::Scheduled,
                )
                .await
            {
                Ok(sync_run_id) => {
                    info!(
                        "Scheduled sync {} triggered for source {} ({})",
                        sync_run_id, source.name, source.source_type
                    );

                    // Update next_sync_at
                    if let Err(e) = self.update_next_sync_at(&source.id).await {
                        error!(
                            "Failed to update next_sync_at for source {}: {}",
                            source.id, e
                        );
                    }
                }
                Err(SyncError::ConcurrencyLimitReached) => {
                    debug!("Concurrency limit reached, will retry on next tick");
                    break;
                }
                Err(e) => {
                    warn!(
                        "Failed to trigger scheduled sync for source {}: {}",
                        source.id, e
                    );
                }
            }
        }

        Ok(())
    }

    async fn update_next_sync_at(&self, source_id: &str) -> Result<(), SchedulerError> {
        // Calculate next sync time based on interval
        sqlx::query(
            r#"
            UPDATE sources
            SET next_sync_at = CURRENT_TIMESTAMP + (sync_interval_seconds || ' seconds')::interval,
                updated_at = CURRENT_TIMESTAMP
            WHERE id = $1 AND sync_interval_seconds IS NOT NULL
            "#,
        )
        .bind(source_id)
        .execute(&self.pool)
        .await
        .map_err(|e| SchedulerError::DatabaseError(e.to_string()))?;

        Ok(())
    }
}

#[derive(Debug, sqlx::FromRow)]
struct DueSource {
    id: String,
    name: String,
    source_type: String,
}

#[derive(Debug, thiserror::Error)]
pub enum SchedulerError {
    #[error("Database error: {0}")]
    DatabaseError(String),
}

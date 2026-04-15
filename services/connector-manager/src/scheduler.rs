use crate::config::ConnectorManagerConfig;
use crate::models::TriggerType;
use crate::source_cleanup::SourceCleanup;
use crate::sync_manager::{SyncError, SyncManager};
use shared::db::repositories::SourceRepository;
use shared::models::SyncType;
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

        // Probe in-flight syncs and reconcile any the connector has lost
        if let Err(e) = self.sync_manager.monitor_running_syncs().await {
            error!("Error monitoring running syncs: {}", e);
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

        // Clean up soft-deleted sources
        SourceCleanup::cleanup_deleted_sources(&self.pool).await;
    }

    async fn process_due_sources(&self) -> Result<(), SchedulerError> {
        let now = OffsetDateTime::now_utc();
        let source_repo = SourceRepository::new(&self.pool);

        let due_sources = source_repo
            .find_due_for_sync(now)
            .await
            .map_err(|e| SchedulerError::DatabaseError(e.to_string()))?;

        if due_sources.is_empty() {
            debug!("No sources due for sync");
            return Ok(());
        }

        info!("Found {} sources due for sync", due_sources.len());

        for source in due_sources {
            if self
                .sync_manager
                .is_sync_running(&source.id)
                .await
                .unwrap_or(false)
            {
                debug!("Source {} is already syncing, skipping", source.id);
                continue;
            }

            match self
                .sync_manager
                .trigger_sync(&source.id, SyncType::Incremental, TriggerType::Scheduled)
                .await
            {
                Ok(sync_run_id) => {
                    info!(
                        "Scheduled sync {} triggered for source {} ({:?})",
                        sync_run_id, source.name, source.source_type
                    );
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
}

#[derive(Debug, thiserror::Error)]
pub enum SchedulerError {
    #[error("Database error: {0}")]
    DatabaseError(String),
}

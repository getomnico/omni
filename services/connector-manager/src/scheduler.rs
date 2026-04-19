use crate::config::ConnectorManagerConfig;
use crate::handlers::get_sync_modes_for_source;
use crate::models::TriggerType;
use crate::source_cleanup::SourceCleanup;
use crate::sync_manager::{SyncError, SyncManager};
use redis::Client as RedisClient;
use shared::db::repositories::SourceRepository;
use shared::models::SyncType;
use sqlx::PgPool;
use std::sync::Arc;
use time::OffsetDateTime;
use tokio::time::{interval, Duration};
use tracing::{debug, error, info, warn};

pub struct Scheduler {
    pool: PgPool,
    redis_client: RedisClient,
    config: ConnectorManagerConfig,
    sync_manager: Arc<SyncManager>,
}

impl Scheduler {
    pub fn new(
        pool: PgPool,
        redis_client: RedisClient,
        config: ConnectorManagerConfig,
        sync_manager: Arc<SyncManager>,
    ) -> Self {
        Self {
            pool,
            redis_client,
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

            let sync_type = pick_scheduled_sync_type(
                &get_sync_modes_for_source(&self.redis_client, source.source_type).await,
            );

            match self
                .sync_manager
                .trigger_sync(&source.id, sync_type, TriggerType::Scheduled)
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

/// Pick the sync type a scheduled tick should request for a source, based on
/// the sync modes the connector declared in its manifest. Realtime wins when
/// available (the SDK's 409 guard keeps exactly one watcher alive; subsequent
/// ticks are no-ops until the watcher exits). Else prefer Incremental, falling
/// back to Full for connectors that only do full scans.
fn pick_scheduled_sync_type(sync_modes: &[String]) -> SyncType {
    if sync_modes.iter().any(|m| m == "realtime") {
        SyncType::Realtime
    } else if sync_modes.iter().any(|m| m == "incremental") {
        SyncType::Incremental
    } else {
        SyncType::Full
    }
}

#[derive(Debug, thiserror::Error)]
pub enum SchedulerError {
    #[error("Database error: {0}")]
    DatabaseError(String),
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn picks_realtime_when_declared() {
        let modes = vec!["full".to_string(), "realtime".to_string()];
        assert_eq!(pick_scheduled_sync_type(&modes), SyncType::Realtime);
    }

    #[test]
    fn falls_back_to_incremental() {
        let modes = vec!["full".to_string(), "incremental".to_string()];
        assert_eq!(pick_scheduled_sync_type(&modes), SyncType::Incremental);
    }

    #[test]
    fn falls_back_to_full_when_only_full() {
        let modes = vec!["full".to_string()];
        assert_eq!(pick_scheduled_sync_type(&modes), SyncType::Full);
    }

    #[test]
    fn falls_back_to_full_when_empty() {
        assert_eq!(pick_scheduled_sync_type(&[]), SyncType::Full);
    }
}

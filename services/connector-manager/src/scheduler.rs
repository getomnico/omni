use crate::config::ConnectorManagerConfig;
use crate::handlers::get_sync_modes_for_source;
use crate::models::TriggerType;
use crate::source_cleanup::SourceCleanup;
use crate::sync_manager::{SyncError, SyncManager};
use redis::Client as RedisClient;
use shared::db::repositories::SourceRepository;
use shared::models::{SyncSlotClass, SyncType};
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

        // Ensure realtime watchers are running for sources that declared the
        // capability. Independent of the scheduled-sync slot, so realtime
        // never starves Full / Incremental and vice versa.
        if let Err(e) = self.ensure_realtime_running().await {
            error!("Error ensuring realtime syncs: {}", e);
        }

        // Check for sources due for scheduled (Full / Incremental) sync.
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

    /// Ensure each active source whose connector advertised `Realtime` has a
    /// running realtime sync. Realtime watchers are long-lived (they heartbeat
    /// rather than complete), so this acts as a "supervisor": start a watcher
    /// if one isn't already in flight, otherwise no-op. Runs in parallel with
    /// scheduled Full / Incremental syncs because they occupy a separate slot.
    async fn ensure_realtime_running(&self) -> Result<(), SchedulerError> {
        let source_repo = SourceRepository::new(&self.pool);

        let active_sources = source_repo
            .find_active_sources()
            .await
            .map_err(|e| SchedulerError::DatabaseError(e.to_string()))?;

        for source in active_sources {
            let modes = get_sync_modes_for_source(&self.redis_client, source.source_type).await;
            if !modes.contains(&SyncType::Realtime) {
                continue;
            }

            match self
                .sync_manager
                .is_sync_class_running(&source.id, SyncSlotClass::Realtime)
                .await
            {
                Ok(true) => continue,
                Ok(false) => {}
                Err(e) => {
                    warn!(
                        "Realtime running check failed for source {}: {}",
                        source.id, e
                    );
                    continue;
                }
            }

            match self
                .sync_manager
                .trigger_sync(&source.id, SyncType::Realtime, TriggerType::Scheduled)
                .await
            {
                Ok(sync_run_id) => {
                    info!(
                        "Realtime sync {} started for source {} ({:?})",
                        sync_run_id, source.name, source.source_type
                    );
                }
                Err(SyncError::ConcurrencyLimitReached) => {
                    debug!("Concurrency limit reached, will retry on next tick");
                    break;
                }
                Err(SyncError::SyncAlreadyRunning(_)) => continue,
                Err(e) => {
                    warn!(
                        "Failed to start realtime sync for source {}: {}",
                        source.id, e
                    );
                }
            }
        }

        Ok(())
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
                .is_sync_class_running(&source.id, SyncSlotClass::Scheduled)
                .await
                .unwrap_or(false)
            {
                debug!(
                    "Source {} already has a scheduled sync running, skipping",
                    source.id
                );
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

/// Pick the sync type a scheduled tick should request for a source. Realtime
/// is intentionally excluded — realtime watchers occupy a separate slot and are
/// supervised by [`Scheduler::ensure_realtime_running`]. Prefer Incremental
/// when declared, falling back to Full for connectors that only do full scans.
fn pick_scheduled_sync_type(sync_modes: &[SyncType]) -> SyncType {
    if sync_modes.contains(&SyncType::Incremental) {
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
    fn skips_realtime_when_declared() {
        // Realtime is supervised separately; scheduled ticks must never pick it.
        let modes = vec![SyncType::Full, SyncType::Incremental, SyncType::Realtime];
        assert_eq!(pick_scheduled_sync_type(&modes), SyncType::Incremental);
    }

    #[test]
    fn prefers_incremental_over_full() {
        let modes = vec![SyncType::Full, SyncType::Incremental];
        assert_eq!(pick_scheduled_sync_type(&modes), SyncType::Incremental);
    }

    #[test]
    fn falls_back_to_full_when_only_full() {
        let modes = vec![SyncType::Full];
        assert_eq!(pick_scheduled_sync_type(&modes), SyncType::Full);
    }

    #[test]
    fn falls_back_to_full_when_only_realtime() {
        // A connector that only declares Realtime has nothing for the
        // scheduled tick to do; default to Full so a manual trigger or
        // bootstrap path can backfill if it ever runs.
        let modes = vec![SyncType::Realtime];
        assert_eq!(pick_scheduled_sync_type(&modes), SyncType::Full);
    }

    #[test]
    fn falls_back_to_full_when_empty() {
        assert_eq!(pick_scheduled_sync_type(&[]), SyncType::Full);
    }
}

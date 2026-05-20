use crate::config::ConnectorManagerConfig;
use crate::handlers::get_sync_modes_for_source;
use crate::models::TriggerType;
use crate::source_cleanup::SourceCleanup;
use crate::sync_manager::{SyncError, SyncManager};
use redis::Client as RedisClient;
use shared::db::repositories::{SourceRepository, SyncRunRepository};
use shared::models::{Source, SyncRun, SyncSlotClass, SyncStatus, SyncType};
use sqlx::PgPool;
use std::collections::HashMap;
use std::sync::Arc;
use time::{Duration as TimeDuration, OffsetDateTime};
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
        let sync_run_repo = SyncRunRepository::new(&self.pool);

        let sources = source_repo
            .find_active_sources()
            .await
            .map_err(|e| SchedulerError::DatabaseError(e.to_string()))?;
        let source_ids: Vec<String> = sources.iter().map(|source| source.id.clone()).collect();
        let sync_runs = sync_run_repo
            .list_runs(
                &source_ids,
                i64::from(self.config.sync_max_consecutive_failures.max(1)),
            )
            .await
            .map_err(|e| SchedulerError::DatabaseError(e.to_string()))?;
        let due_sources = sources_due_for_sync(
            sources,
            sync_runs,
            now,
            self.config.sync_max_consecutive_failures,
            self.config.sync_backoff_base_seconds,
            self.config.sync_backoff_max_seconds,
        );

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

fn sources_due_for_sync(
    sources: Vec<Source>,
    sync_runs: Vec<SyncRun>,
    now: OffsetDateTime,
    max_consecutive_failures: i32,
    backoff_base_seconds: i64,
    backoff_max_seconds: i64,
) -> Vec<Source> {
    let mut runs_by_source = HashMap::<String, Vec<SyncRun>>::new();
    for run in sync_runs {
        runs_by_source
            .entry(run.source_id.clone())
            .or_default()
            .push(run);
    }

    let source_count = sources.len();
    let mut due_sources: Vec<(Source, Option<OffsetDateTime>)> = sources
        .into_iter()
        .filter_map(|source| {
            let sync_interval_seconds = match source.sync_interval_seconds {
                Some(interval) => interval,
                None => {
                    info!(
                        "Skipping scheduled sync for source {} ({:?}): no sync interval configured",
                        source.id, source.source_type
                    );
                    return None;
                }
            };
            let sync_runs = runs_by_source.remove(&source.id).unwrap_or_default();
            let latest_success_at = sync_runs
                .iter()
                .find(|run| run.status == SyncStatus::Completed)
                .and_then(|run| run.completed_at);
            let failed_runs: Vec<&SyncRun> = sync_runs
                .iter()
                .take_while(|run| run.status == SyncStatus::Failed)
                .collect();

            // Circuit breaker: stop scheduled retries once the visible
            // consecutive failure streak reaches the configured threshold.
            if max_consecutive_failures <= failed_runs.len() as i32 {
                info!(
                    "Skipping scheduled sync for source {} ({:?}): circuit breaker open after {} consecutive failures (threshold {})",
                    source.id,
                    source.source_type,
                    failed_runs.len(),
                    max_consecutive_failures
                );
                return None;
            }

            let last_failed_at = failed_runs.first().and_then(|run| run.completed_at);
            if let Some(last_failed_at) = last_failed_at {
                let backoff =
                    backoff_seconds(failed_runs.len(), backoff_base_seconds, backoff_max_seconds);
                if last_failed_at + TimeDuration::seconds(backoff) > now {
                    info!(
                        "Skipping scheduled sync for source {} ({:?}): failure backoff still active after {} consecutive failures; last failure at {}, backoff {}s",
                        source.id,
                        source.source_type,
                        failed_runs.len(),
                        last_failed_at,
                        backoff
                    );
                    return None;
                }
            }

            // With a bounded recent-run window, missing success means either
            // the source never succeeded or the latest success is older than
            // the fetched failure window. In both cases the interval is not
            // the blocking condition; failure threshold/backoff above decides.
            let Some(latest_success_at) = latest_success_at else {
                info!(
                    "Source {} ({:?}) is due for scheduled sync: no recent successful sync found, {} consecutive failures below threshold",
                    source.id,
                    source.source_type,
                    failed_runs.len()
                );
                return Some((source, last_failed_at));
            };

            let next_sync_at =
                latest_success_at + TimeDuration::seconds(sync_interval_seconds as i64);
            if next_sync_at > now {
                info!(
                    "Skipping scheduled sync for source {} ({:?}): next sync due at {}, latest success at {}, interval {}s",
                    source.id,
                    source.source_type,
                    next_sync_at,
                    latest_success_at,
                    sync_interval_seconds
                );
                return None;
            }

            info!(
                "Source {} ({:?}) is due for scheduled sync: latest success at {}, interval {}s, {} consecutive failures",
                source.id,
                source.source_type,
                latest_success_at,
                sync_interval_seconds,
                failed_runs.len()
            );
            Some((source, last_failed_at.or(Some(latest_success_at))))
        })
        .collect();

    due_sources.sort_by_key(|(_, last_sync_at)| *last_sync_at);
    let due_count = due_sources.len();
    info!(
        "Scheduled sync due-source evaluation complete: {} of {} sources due",
        due_count, source_count
    );

    due_sources
        .into_iter()
        .take(10)
        .map(|(source, _)| source)
        .collect()
}

fn backoff_seconds(
    failure_count: usize,
    backoff_base_seconds: i64,
    backoff_max_seconds: i64,
) -> i64 {
    if failure_count == 0 {
        return 0;
    }

    let multiplier = 1_i64 << failure_count.saturating_sub(1).min(20);
    backoff_max_seconds.min(backoff_base_seconds.saturating_mul(multiplier))
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
    use serde_json::json;
    use shared::models::{SourceScope, SourceType, UserFilterMode};

    fn source(id: &str, interval_seconds: Option<i32>) -> Source {
        let now = OffsetDateTime::now_utc();
        Source {
            id: id.to_string(),
            name: format!("source-{id}"),
            source_type: SourceType::LocalFiles,
            config: json!({}),
            is_active: true,
            is_deleted: false,
            scope: SourceScope::Org,
            user_filter_mode: UserFilterMode::All,
            user_whitelist: None,
            user_blacklist: None,
            connector_state: None,
            sync_interval_seconds: interval_seconds,
            created_at: now,
            updated_at: now,
            created_by: "01JGF7V3E0Y2R1X8P5Q7W9T4N6".to_string(),
        }
    }

    fn sync_run(
        id: &str,
        source_id: &str,
        status: SyncStatus,
        completed_at: Option<OffsetDateTime>,
    ) -> SyncRun {
        let now = OffsetDateTime::now_utc();
        SyncRun {
            id: id.to_string(),
            source_id: source_id.to_string(),
            sync_type: SyncType::Incremental,
            started_at: completed_at,
            completed_at,
            status,
            documents_scanned: 0,
            documents_processed: 0,
            documents_updated: 0,
            error_message: None,
            created_at: now,
            updated_at: now,
        }
    }

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

    #[test]
    fn due_sources_include_elapsed_successes() {
        let now = OffsetDateTime::now_utc();
        let due = sources_due_for_sync(
            vec![source("source-1", Some(60))],
            vec![sync_run(
                "run-1",
                "source-1",
                SyncStatus::Completed,
                Some(now - TimeDuration::seconds(120)),
            )],
            now,
            10,
            30,
            3600,
        );

        assert_eq!(due[0].id, "source-1");
    }

    #[test]
    fn due_sources_skip_recent_successes() {
        let now = OffsetDateTime::now_utc();
        let due = sources_due_for_sync(
            vec![source("source-1", Some(60))],
            vec![sync_run(
                "run-1",
                "source-1",
                SyncStatus::Completed,
                Some(now - TimeDuration::seconds(10)),
            )],
            now,
            10,
            30,
            3600,
        );

        assert!(due.is_empty());
    }

    #[test]
    fn due_sources_apply_failure_backoff_and_threshold() {
        let now = OffsetDateTime::now_utc();
        let due = sources_due_for_sync(
            vec![
                source("inside-backoff", Some(60)),
                source("after-backoff", Some(60)),
                source("threshold-hit", Some(60)),
            ],
            vec![
                sync_run(
                    "run-1",
                    "inside-backoff",
                    SyncStatus::Failed,
                    Some(now - TimeDuration::seconds(10)),
                ),
                sync_run(
                    "run-2",
                    "after-backoff",
                    SyncStatus::Failed,
                    Some(now - TimeDuration::seconds(120)),
                ),
                sync_run(
                    "run-3",
                    "threshold-hit",
                    SyncStatus::Failed,
                    Some(now - TimeDuration::seconds(120)),
                ),
                sync_run(
                    "run-4",
                    "threshold-hit",
                    SyncStatus::Failed,
                    Some(now - TimeDuration::seconds(180)),
                ),
            ],
            now,
            2,
            30,
            3600,
        );

        assert_eq!(due.len(), 1);
        assert_eq!(due[0].id, "after-backoff");
    }

    #[test]
    fn completed_run_resets_failure_streak() {
        let now = OffsetDateTime::now_utc();
        let due = sources_due_for_sync(
            vec![source("source-1", Some(60))],
            vec![
                sync_run(
                    "run-1",
                    "source-1",
                    SyncStatus::Completed,
                    Some(now - TimeDuration::seconds(120)),
                ),
                sync_run(
                    "run-2",
                    "source-1",
                    SyncStatus::Failed,
                    Some(now - TimeDuration::seconds(600)),
                ),
            ],
            now,
            1,
            30,
            3600,
        );

        assert_eq!(due[0].id, "source-1");
    }

    #[test]
    fn source_with_old_success_outside_recent_window_is_due_after_failure_backoff() {
        let now = OffsetDateTime::now_utc();
        let due = sources_due_for_sync(
            vec![source("source-1", Some(60))],
            vec![sync_run(
                "run-1",
                "source-1",
                SyncStatus::Failed,
                Some(now - TimeDuration::seconds(120)),
            )],
            now,
            10,
            30,
            3600,
        );

        assert_eq!(due[0].id, "source-1");
    }
}

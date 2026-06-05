use crate::config::ConnectorManagerConfig;
use crate::handlers::get_sync_modes_for_source;
use crate::models::TriggerType;
use crate::source_cleanup::SourceCleanup;
use crate::sync_circuit_breaker::current_unsuccessful_streak;
use crate::sync_manager::{SyncError, SyncManager};
use futures::FutureExt;
use redis::Client as RedisClient;
use shared::db::repositories::{SourceRepository, SyncRunRepository};
use shared::models::{Source, SyncRun, SyncSlotClass, SyncStatus, SyncType};
use sqlx::PgPool;
use std::collections::HashMap;
use std::fmt::Display;
use std::future::Future;
use std::panic::AssertUnwindSafe;
use std::sync::{Arc, Mutex};
use std::time::Instant;
use time::{Duration as TimeDuration, OffsetDateTime};
use tokio::time::{interval, sleep, timeout, Duration};
use tracing::{debug, error, info, warn};

const SCHEDULER_PHASE_TIMEOUT: Duration = Duration::from_secs(300);
const SCHEDULER_RESTART_DELAY: Duration = Duration::from_secs(5);

pub struct Scheduler {
    pool: PgPool,
    redis_client: RedisClient,
    config: ConnectorManagerConfig,
    sync_manager: Arc<SyncManager>,
    slot_health: Arc<Mutex<HashMap<SlotHealthKey, SlotHealth>>>,
}

impl Scheduler {
    pub async fn run(
        pool: PgPool,
        redis_client: RedisClient,
        config: ConnectorManagerConfig,
        sync_manager: Arc<SyncManager>,
    ) {
        loop {
            let scheduler = Self::new(
                pool.clone(),
                redis_client.clone(),
                config.clone(),
                sync_manager.clone(),
            );

            match AssertUnwindSafe(scheduler.run_internal())
                .catch_unwind()
                .await
            {
                Ok(()) => {
                    error!("Scheduler exited unexpectedly; restarting");
                }
                Err(_) => {
                    error!("Scheduler panicked; restarting");
                }
            }

            sleep(SCHEDULER_RESTART_DELAY).await;
        }
    }

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
            slot_health: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    async fn run_internal(&self) {
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

        self.run_phase("ensure_realtime_running", self.ensure_realtime_running())
            .await;

        self.run_phase("process_due_sources", self.process_due_sources())
            .await;

        self.run_phase(
            "cancel_syncs_for_inactive_sources",
            self.sync_manager.cancel_syncs_for_inactive_sources(),
        )
        .await
        .inspect(|cancelled| {
            if !cancelled.is_empty() {
                info!(
                    "Cancelled {} sync(s) for inactive/deleted sources",
                    cancelled.len()
                );
            }
        });

        self.run_phase(
            "monitor_running_syncs",
            self.sync_manager.monitor_running_syncs(),
        )
        .await;

        if let Some(stale) = self
            .run_phase("detect_stale_syncs", self.sync_manager.detect_stale_syncs())
            .await
        {
            if !stale.is_empty() {
                info!("Marked {} stale syncs as failed", stale.len());
            }
        }

        self.run_phase("cleanup_deleted_sources", async {
            SourceCleanup::cleanup_deleted_sources(&self.pool).await;
            Ok::<(), SchedulerError>(())
        })
        .await;
    }

    async fn run_phase<T, E, F>(&self, phase: &'static str, future: F) -> Option<T>
    where
        E: Display,
        F: Future<Output = Result<T, E>>,
    {
        let started = Instant::now();
        debug!(
            phase,
            timeout_secs = SCHEDULER_PHASE_TIMEOUT.as_secs(),
            "Scheduler phase started"
        );

        match timeout(SCHEDULER_PHASE_TIMEOUT, future).await {
            Ok(Ok(value)) => {
                debug!(
                    phase,
                    elapsed_ms = started.elapsed().as_millis(),
                    "Scheduler phase completed"
                );
                Some(value)
            }
            Ok(Err(e)) => {
                error!(
                    phase,
                    elapsed_ms = started.elapsed().as_millis(),
                    error = %e,
                    "Scheduler phase failed"
                );
                None
            }
            Err(_) => {
                error!(
                    phase,
                    timeout_secs = SCHEDULER_PHASE_TIMEOUT.as_secs(),
                    "Scheduler phase timed out; continuing with next phase"
                );
                None
            }
        }
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
        let now = OffsetDateTime::now_utc();

        for source in active_sources {
            let modes = get_sync_modes_for_source(&self.redis_client, source.source_type).await;
            if !modes.contains(&SyncType::Realtime) {
                continue;
            }
            if self.slot_backoff_active(&source, SyncSlotClass::Realtime, now) {
                continue;
            }

            match self
                .sync_manager
                .is_sync_class_running(&source.id, SyncSlotClass::Realtime)
                .await
            {
                Ok(true) => {
                    debug!(
                        source_id = %source.id,
                        source_type = ?source.source_type,
                        "Realtime sync already running"
                    );
                    continue;
                }
                Ok(false) => {}
                Err(e) => {
                    warn!(
                        "Realtime running check failed for source {}: {}",
                        source.id, e
                    );
                    continue;
                }
            }

            debug!(
                source_id = %source.id,
                source_type = ?source.source_type,
                source_name = %source.name,
                "Starting realtime sync"
            );
            match self
                .sync_manager
                .trigger_sync(&source.id, SyncType::Realtime, TriggerType::Scheduled)
                .await
            {
                Ok(sync_run_id) => {
                    self.mark_slot_healthy(&source.id, SyncSlotClass::Realtime);
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
                Err(SyncError::SyncModeUnavailable { .. }) => {
                    self.mark_slot_unhealthy(
                        &source.id,
                        SyncSlotClass::Realtime,
                        OffsetDateTime::now_utc(),
                    );
                    info!(
                        "Realtime sync is not available for source {} ({:?})",
                        source.id, source.source_type
                    );
                    continue;
                }
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

    fn mark_slot_healthy(&self, source_id: &str, slot_class: SyncSlotClass) {
        let mut slot_health = self.slot_health.lock().expect("slot health lock poisoned");
        slot_health.remove(&SlotHealthKey::new(source_id, slot_class));
    }

    fn mark_slot_unhealthy(
        &self,
        source_id: &str,
        slot_class: SyncSlotClass,
        failed_at: OffsetDateTime,
    ) {
        let mut slot_health = self.slot_health.lock().expect("slot health lock poisoned");
        let health = slot_health
            .entry(SlotHealthKey::new(source_id, slot_class))
            .or_default();
        health.consecutive_failures += 1;
        health.last_failure_at = Some(failed_at);
    }

    fn slot_backoff_active(
        &self,
        source: &Source,
        slot_class: SyncSlotClass,
        now: OffsetDateTime,
    ) -> bool {
        let health = {
            let slot_health = self.slot_health.lock().expect("slot health lock poisoned");
            slot_health
                .get(&SlotHealthKey::new(&source.id, slot_class))
                .copied()
        };

        let Some((consecutive_failures, last_failure_at)) = active_backoff(
            &health,
            now,
            self.config.sync_backoff_base_seconds,
            self.config.sync_backoff_max_seconds,
        ) else {
            return false;
        };

        info!(
            "Skipping {} sync for source {} ({:?}): slot backoff still active after {} failures; last failure at {}, backoff {}s",
            slot_class,
            source.id,
            source.source_type,
            consecutive_failures,
            last_failure_at,
            backoff_seconds(consecutive_failures, self.config.sync_backoff_base_seconds, self.config.sync_backoff_max_seconds)
        );
        true
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
        let recent_run_limit = self
            .config
            .sync_max_consecutive_failures
            .max(1)
            .saturating_mul(3);
        let sync_runs = sync_run_repo
            .list_runs_for_sync_types(
                &source_ids,
                &[SyncType::Full, SyncType::Incremental],
                i64::from(recent_run_limit),
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
            if self.slot_backoff_active(&source, SyncSlotClass::Scheduled, now) {
                continue;
            }

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
                    self.mark_slot_healthy(&source.id, SyncSlotClass::Scheduled);
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
                    self.mark_slot_unhealthy(
                        &source.id,
                        SyncSlotClass::Scheduled,
                        OffsetDateTime::now_utc(),
                    );
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
            let unsuccessful_runs = current_unsuccessful_streak(&sync_runs);

            // Circuit breaker: stop scheduled retries once the visible streak
            // of terminal failed/cancelled runs reaches the configured threshold.
            if max_consecutive_failures <= unsuccessful_runs.len() as i32 {
                info!(
                    "Skipping scheduled sync for source {} ({:?}): circuit breaker open after {} consecutive failed/cancelled runs (threshold {})",
                    source.id,
                    source.source_type,
                    unsuccessful_runs.len(),
                    max_consecutive_failures
                );
                return None;
            }

            let last_failed_at = unsuccessful_runs.first().and_then(|run| run.completed_at);
            if let Some(last_failed_at) = last_failed_at {
                let backoff = backoff_seconds(
                    unsuccessful_runs.len(),
                    backoff_base_seconds,
                    backoff_max_seconds,
                );
                if last_failed_at + TimeDuration::seconds(backoff) > now {
                    info!(
                        "Skipping scheduled sync for source {} ({:?}): failure backoff still active after {} consecutive failed/cancelled runs; last failure at {}, backoff {}s",
                        source.id,
                        source.source_type,
                        unsuccessful_runs.len(),
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
                    "Source {} ({:?}) is due for scheduled sync: no recent successful sync found, {} consecutive failed/cancelled runs below threshold",
                    source.id,
                    source.source_type,
                    unsuccessful_runs.len()
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
                "Source {} ({:?}) is due for scheduled sync: latest success at {}, interval {}s, {} consecutive unsuccessful runs",
                source.id,
                source.source_type,
                latest_success_at,
                sync_interval_seconds,
                unsuccessful_runs.len()
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

fn active_backoff(
    health: &Option<SlotHealth>,
    now: OffsetDateTime,
    backoff_base_seconds: i64,
    backoff_max_seconds: i64,
) -> Option<(usize, OffsetDateTime)> {
    let health = health.as_ref()?;
    let last_failure_at = health.last_failure_at?;
    let backoff = backoff_seconds(
        health.consecutive_failures,
        backoff_base_seconds,
        backoff_max_seconds,
    );

    if last_failure_at + TimeDuration::seconds(backoff) <= now {
        None
    } else {
        Some((health.consecutive_failures, last_failure_at))
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

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
struct SlotHealthKey {
    source_id: String,
    slot_class: SyncSlotClass,
}

impl SlotHealthKey {
    fn new(source_id: &str, slot_class: SyncSlotClass) -> Self {
        Self {
            source_id: source_id.to_string(),
            slot_class,
        }
    }
}

#[derive(Debug, Clone, Copy, Default)]
struct SlotHealth {
    consecutive_failures: usize,
    last_failure_at: Option<OffsetDateTime>,
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
        sync_run_with_type(id, source_id, SyncType::Incremental, status, completed_at)
    }

    fn sync_run_with_type(
        id: &str,
        source_id: &str,
        sync_type: SyncType,
        status: SyncStatus,
        completed_at: Option<OffsetDateTime>,
    ) -> SyncRun {
        let now = OffsetDateTime::now_utc();
        SyncRun {
            id: id.to_string(),
            source_id: source_id.to_string(),
            sync_type,
            started_at: completed_at,
            completed_at,
            status,
            trigger_type: TriggerType::Scheduled.to_string(),
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
    fn slot_health_backoff_uses_in_memory_state() {
        let now = OffsetDateTime::now_utc();
        let health = Some(SlotHealth {
            consecutive_failures: 2,
            last_failure_at: Some(now - TimeDuration::seconds(50)),
        });

        assert_eq!(
            active_backoff(&health, now, 30, 3600),
            health.map(|health| (health.consecutive_failures, health.last_failure_at.unwrap()))
        );
    }

    #[test]
    fn slot_health_backoff_expires() {
        let now = OffsetDateTime::now_utc();
        let health = Some(SlotHealth {
            consecutive_failures: 1,
            last_failure_at: Some(now - TimeDuration::seconds(31)),
        });

        assert!(active_backoff(&health, now, 30, 3600).is_none());
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
    fn manual_failure_does_not_count_toward_failure_streak() {
        let now = OffsetDateTime::now_utc();
        let mut manual_run = sync_run(
            "run-1",
            "source-1",
            SyncStatus::Failed,
            Some(now - TimeDuration::seconds(120)),
        );
        manual_run.trigger_type = TriggerType::Manual.to_string();

        let due = sources_due_for_sync(
            vec![source("source-1", Some(60))],
            vec![
                manual_run,
                sync_run(
                    "run-2",
                    "source-1",
                    SyncStatus::Failed,
                    Some(now - TimeDuration::seconds(180)),
                ),
            ],
            now,
            2,
            30,
            3600,
        );

        assert_eq!(due[0].id, "source-1");
    }

    #[test]
    fn manual_success_breaks_failure_streak() {
        let now = OffsetDateTime::now_utc();
        let mut manual_run = sync_run(
            "run-1",
            "source-1",
            SyncStatus::Completed,
            Some(now - TimeDuration::seconds(120)),
        );
        manual_run.trigger_type = TriggerType::Manual.to_string();

        let due = sources_due_for_sync(
            vec![source("source-1", Some(60))],
            vec![
                manual_run,
                sync_run(
                    "run-2",
                    "source-1",
                    SyncStatus::Failed,
                    Some(now - TimeDuration::seconds(180)),
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
    fn running_run_does_not_count_toward_failure_streak() {
        let now = OffsetDateTime::now_utc();
        let due = sources_due_for_sync(
            vec![source("source-1", Some(60))],
            vec![
                sync_run("run-1", "source-1", SyncStatus::Running, None),
                sync_run(
                    "run-2",
                    "source-1",
                    SyncStatus::Failed,
                    Some(now - TimeDuration::seconds(180)),
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
    fn cancelled_run_counts_toward_failure_streak() {
        let now = OffsetDateTime::now_utc();
        let due = sources_due_for_sync(
            vec![source("source-1", Some(60))],
            vec![
                sync_run(
                    "run-1",
                    "source-1",
                    SyncStatus::Cancelled,
                    Some(now - TimeDuration::seconds(120)),
                ),
                sync_run(
                    "run-2",
                    "source-1",
                    SyncStatus::Failed,
                    Some(now - TimeDuration::seconds(180)),
                ),
            ],
            now,
            2,
            30,
            3600,
        );

        assert!(due.is_empty());
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

    #[test]
    fn realtime_failures_do_not_block_scheduled_syncs() {
        let now = OffsetDateTime::now_utc();
        let due = sources_due_for_sync(
            vec![source("source-1", Some(60))],
            vec![sync_run_with_type(
                "run-1",
                "source-1",
                SyncType::Realtime,
                SyncStatus::Failed,
                Some(now - TimeDuration::seconds(10)),
            )],
            now,
            1,
            30,
            3600,
        );

        assert_eq!(due[0].id, "source-1");
    }
}

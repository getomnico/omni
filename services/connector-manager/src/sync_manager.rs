use crate::config::ConnectorManagerConfig;
use crate::connector_client::{ClientError, ConnectorClient};
use crate::handlers::get_connector_url_for_source;
use crate::models::{SyncRequest, TriggerType};
use dashmap::DashMap;
use redis::Client as RedisClient;
use shared::db::repositories::SyncRunRepository;
use shared::models::{SourceType, SyncStatus, SyncType};
use shared::{DatabasePool, Repository, SourceRepository};
use sqlx::PgPool;
use std::sync::Arc;
use time::format_description::well_known::Rfc3339;
use time::OffsetDateTime;
use tracing::{error, info, warn};

const MAX_RESUME_ATTEMPTS: usize = 3;

#[derive(Clone)]
pub struct SyncManager {
    pool: PgPool,
    config: ConnectorManagerConfig,
    redis_client: RedisClient,
    connector_client: ConnectorClient,
    sync_run_repo: SyncRunRepository,
    /// In-memory tally of resume attempts per sync_run_id. Cleared when a
    /// sync transitions out of `running`. Bounds resume churn for chronically
    /// crashing connectors without needing schema changes.
    resume_attempts: Arc<DashMap<String, usize>>,
}

impl SyncManager {
    pub fn new(
        db_pool: &DatabasePool,
        config: ConnectorManagerConfig,
        redis_client: RedisClient,
    ) -> Self {
        Self {
            pool: db_pool.pool().clone(),
            config,
            redis_client,
            connector_client: ConnectorClient::new(),
            sync_run_repo: SyncRunRepository::new(db_pool.pool()),
            resume_attempts: Arc::new(DashMap::new()),
        }
    }

    pub async fn trigger_sync(
        &self,
        source_id: &str,
        sync_type: SyncType,
        trigger_type: TriggerType,
    ) -> Result<String, SyncError> {
        if self.is_sync_running(source_id).await? {
            return Err(SyncError::SyncAlreadyRunning(source_id.to_string()));
        }

        if self.active_sync_count().await? >= self.config.max_concurrent_syncs {
            return Err(SyncError::ConcurrencyLimitReached);
        }

        // Get source details
        let source_repo = SourceRepository::new(&self.pool);
        let source = source_repo
            .find_by_id(source_id.to_string())
            .await
            .map_err(|e| SyncError::DatabaseError(e.to_string()))?
            .ok_or_else(|| SyncError::SourceNotFound(source_id.to_string()))?;

        if !source.is_active {
            return Err(SyncError::SourceInactive(source_id.to_string()));
        }

        // Get connector URL from registry
        let connector_url = get_connector_url_for_source(&self.redis_client, source.source_type)
            .await
            .ok_or_else(|| {
                SyncError::ConnectorNotConfigured(format!("{:?}", source.source_type))
            })?;

        // Check last completed sync to determine effective sync type and last_sync_at
        let last_completed = self
            .sync_run_repo
            .get_last_completed_for_source(source_id, None)
            .await
            .map_err(|e| SyncError::DatabaseError(e.to_string()))?;

        let effective_sync_type = match sync_type {
            SyncType::Incremental if last_completed.is_none() => {
                info!(
                    "No prior completed sync for source {}; upgrading to full sync",
                    source_id
                );
                SyncType::Full
            }
            other => other,
        };

        let last_sync_at = if effective_sync_type == SyncType::Incremental {
            last_completed
                .as_ref()
                .and_then(|run| run.completed_at)
                .and_then(|ts| ts.format(&Rfc3339).ok())
        } else {
            None
        };

        let sync_run = self
            .sync_run_repo
            .create(source_id, effective_sync_type, &trigger_type.to_string())
            .await
            .map_err(|e| SyncError::DatabaseError(e.to_string()))?;

        let sync_request = SyncRequest {
            sync_run_id: sync_run.id.clone(),
            source_id: source_id.to_string(),
            sync_mode: match effective_sync_type {
                SyncType::Full => "full",
                SyncType::Incremental => "incremental",
                SyncType::Realtime => "realtime",
            }
            .to_string(),
            last_sync_at,
        };

        // Trigger sync (non-blocking call to connector)
        match self
            .connector_client
            .trigger_sync(&connector_url, &sync_request)
            .await
        {
            Ok(response) => {
                info!(
                    "Sync triggered for source {}: {:?}",
                    source_id, response.status
                );
                Ok(sync_run.id)
            }
            Err(e) => {
                self.mark_sync_failed(&sync_run.id, &e.to_string()).await?;
                Err(SyncError::ConnectorError(e))
            }
        }
    }

    pub async fn cancel_sync(&self, sync_run_id: &str) -> Result<(), SyncError> {
        let sync_run = self
            .sync_run_repo
            .find_by_id(sync_run_id)
            .await
            .map_err(|e| SyncError::DatabaseError(e.to_string()))?
            .ok_or_else(|| SyncError::SyncRunNotFound(sync_run_id.to_string()))?;

        if sync_run.status != SyncStatus::Running {
            return Err(SyncError::SyncNotRunning(sync_run_id.to_string()));
        }

        let source_repo = SourceRepository::new(&self.pool);
        let source = Repository::find_by_id(&source_repo, sync_run.source_id.clone())
            .await
            .map_err(|e| SyncError::DatabaseError(e.to_string()))?
            .ok_or_else(|| SyncError::SourceNotFound(sync_run.source_id.clone()))?;

        let connector_url = get_connector_url_for_source(&self.redis_client, source.source_type)
            .await
            .ok_or_else(|| {
                SyncError::ConnectorNotConfigured(format!("{:?}", source.source_type))
            })?;

        if let Err(e) = self
            .connector_client
            .cancel_sync(&connector_url, sync_run_id)
            .await
        {
            warn!("Failed to send cancel request to connector: {}", e);
        }

        self.sync_run_repo
            .mark_cancelled(sync_run_id)
            .await
            .map_err(|e| SyncError::DatabaseError(e.to_string()))?;

        self.resume_attempts.remove(sync_run_id);
        info!("Sync {} cancelled", sync_run_id);
        Ok(())
    }

    pub async fn is_sync_running(&self, source_id: &str) -> Result<bool, SyncError> {
        self.sync_run_repo
            .get_running_for_source(source_id)
            .await
            .map(|r| r.is_some())
            .map_err(|e| SyncError::DatabaseError(e.to_string()))
    }

    pub async fn active_sync_count(&self) -> Result<usize, SyncError> {
        self.sync_run_repo
            .find_all_running()
            .await
            .map(|r| r.len())
            .map_err(|e| SyncError::DatabaseError(e.to_string()))
    }

    /// Probe each running sync's connector to confirm it is actually running.
    ///
    /// For every `sync_runs` row with status='running', ask the connector via
    /// `GET /sync/{sync_run_id}` whether it still has the sync in flight. If the
    /// connector reports `running: false` (typically because it restarted and
    /// lost its in-memory state), mark the row as failed and immediately
    /// trigger a fresh run that resumes from the last persisted
    /// `connector_state`. Tolerates probe errors / 404s / unregistered
    /// connectors as no-ops; existing stale-detection is the fallback.
    pub async fn monitor_running_syncs(&self) -> Result<(), SyncError> {
        let running = self
            .sync_run_repo
            .find_all_running()
            .await
            .map_err(|e| SyncError::DatabaseError(e.to_string()))?;

        if running.is_empty() {
            return Ok(());
        }

        let source_repo = SourceRepository::new(&self.pool);

        for sync_run in running {
            let source = match source_repo
                .find_by_id(sync_run.source_id.clone())
                .await
                .map_err(|e| SyncError::DatabaseError(e.to_string()))?
            {
                Some(s) => s,
                None => continue,
            };

            let connector_url =
                match get_connector_url_for_source(&self.redis_client, source.source_type).await {
                    Some(url) => url,
                    None => {
                        // Connector isn't registered in Redis — either it's
                        // down long enough for the 90s TTL to have expired,
                        // or it never came up. Treat as a lost sync so we
                        // count attempts and eventually mark the row failed
                        // (instead of waiting for the 60-min stale timeout).
                        warn!(
                        "No registered connector for sync {} (source_type={:?}); treating as lost",
                        sync_run.id, source.source_type
                    );
                        self.handle_lost_sync(&sync_run.id, &sync_run.source_id)
                            .await;
                        continue;
                    }
                };

            match self
                .connector_client
                .get_sync_status(&connector_url, &sync_run.id)
                .await
            {
                Ok(status) if status.running => {
                    // Healthy observation — clear any prior lost-signal count
                    // so the attempt cap tracks consecutive failures.
                    self.resume_attempts.remove(&sync_run.id);
                }
                Ok(_) => {
                    warn!(
                        "Connector reports sync {} no longer running; reconciling",
                        sync_run.id
                    );
                    self.handle_lost_sync(&sync_run.id, &sync_run.source_id)
                        .await;
                }
                Err(ClientError::ConnectorError { status: 404, .. }) => {
                    // Connector hasn't implemented the status endpoint
                    // (e.g., Rust connectors prior to follow-up PR).
                    // Fall through to existing stale-detection.
                }
                Err(e) => {
                    // Connector reachable-via-Redis but not responding
                    // (connection refused, timeout, 5xx). Treat same as
                    // "lost" — attempt counter absorbs transient blips.
                    warn!(
                        "Sync status probe failed for {} ({}): {}; treating as lost",
                        sync_run.id, connector_url, e
                    );
                    self.handle_lost_sync(&sync_run.id, &sync_run.source_id)
                        .await;
                }
            }
        }

        Ok(())
    }

    /// Re-trigger `/sync` on the connector for an existing `running` sync_run
    /// whose connector has lost track of it (typically due to a restart). The
    /// row stays in `running`; the connector will resume from the
    /// incrementally-persisted `connector_state`. If we exceed
    /// MAX_RESUME_ATTEMPTS or the connector keeps refusing, mark the row
    /// failed so we stop churning.
    async fn handle_lost_sync(&self, sync_run_id: &str, source_id: &str) {
        let attempts = {
            let mut entry = self
                .resume_attempts
                .entry(sync_run_id.to_string())
                .or_insert(0);
            *entry += 1;
            *entry
        };

        if attempts > MAX_RESUME_ATTEMPTS {
            warn!(
                "Sync {} exceeded {} resume attempts; marking failed",
                sync_run_id, MAX_RESUME_ATTEMPTS
            );
            if let Err(e) = self
                .mark_sync_failed(
                    sync_run_id,
                    "Connector repeatedly lost sync; auto-resume gave up",
                )
                .await
            {
                error!("Failed to mark sync {} as failed: {}", sync_run_id, e);
            }
            self.resume_attempts.remove(sync_run_id);
            return;
        }

        let source = match SourceRepository::new(&self.pool)
            .find_by_id(source_id.to_string())
            .await
        {
            Ok(Some(s)) => s,
            Ok(None) => return,
            Err(e) => {
                error!("Failed to load source {}: {}", source_id, e);
                return;
            }
        };

        let connector_url =
            match get_connector_url_for_source(&self.redis_client, source.source_type).await {
                Some(url) => url,
                None => {
                    // Connector is still unregistered. Attempt was counted
                    // above; after MAX_RESUME_ATTEMPTS the cap above will
                    // mark the row failed.
                    warn!(
                        "Cannot resume sync {} — connector for {:?} not registered (attempt {}/{})",
                        sync_run_id, source.source_type, attempts, MAX_RESUME_ATTEMPTS
                    );
                    return;
                }
            };

        // Look up the existing run to recover sync_type and the right
        // last_sync_at to send.
        let sync_run = match self.sync_run_repo.find_by_id(sync_run_id).await {
            Ok(Some(r)) => r,
            Ok(None) => return,
            Err(e) => {
                error!("Failed to load sync_run {}: {}", sync_run_id, e);
                return;
            }
        };

        let last_sync_at = if sync_run.sync_type == SyncType::Incremental {
            match self
                .sync_run_repo
                .get_last_completed_for_source(source_id, None)
                .await
            {
                Ok(Some(r)) => r.completed_at.and_then(|ts| ts.format(&Rfc3339).ok()),
                _ => None,
            }
        } else {
            None
        };

        let sync_request = SyncRequest {
            sync_run_id: sync_run_id.to_string(),
            source_id: source_id.to_string(),
            sync_mode: match sync_run.sync_type {
                SyncType::Full => "full",
                SyncType::Incremental => "incremental",
                SyncType::Realtime => "realtime",
            }
            .to_string(),
            last_sync_at,
        };

        match self
            .connector_client
            .trigger_sync(&connector_url, &sync_request)
            .await
        {
            Ok(_) => {
                info!(
                    "Auto-resumed sync {} on connector (attempt {}/{})",
                    sync_run_id, attempts, MAX_RESUME_ATTEMPTS
                );
                // Reset staleness clock so detect_stale_syncs doesn't fire
                // before the resumed sync starts emitting.
                if let Err(e) = self.sync_run_repo.update_activity(sync_run_id).await {
                    warn!(
                        "Failed to bump activity for resumed sync {}: {}",
                        sync_run_id, e
                    );
                }
            }
            Err(e) => {
                warn!(
                    "Failed to re-trigger sync {} on connector (attempt {}/{}): {}",
                    sync_run_id, attempts, MAX_RESUME_ATTEMPTS, e
                );
            }
        }
    }

    pub async fn detect_stale_syncs(&self) -> Result<Vec<String>, SyncError> {
        let timeout_minutes = self.config.stale_sync_timeout_minutes as i64;
        let cutoff = OffsetDateTime::now_utc() - time::Duration::minutes(timeout_minutes);

        let stale_syncs: Vec<(String, String, SourceType)> = sqlx::query_as(
            r#"
            SELECT sr.id, sr.source_id, s.source_type
            FROM sync_runs sr
            JOIN sources s ON sr.source_id = s.id
            WHERE sr.status = 'running'
            AND (
                (sr.last_activity_at IS NOT NULL AND sr.last_activity_at < $1)
                OR (sr.last_activity_at IS NULL AND sr.created_at < $1)
            )
            "#,
        )
        .bind(cutoff)
        .fetch_all(&self.pool)
        .await
        .map_err(|e| SyncError::DatabaseError(e.to_string()))?;

        let mut marked_stale = Vec::new();
        for (sync_run_id, source_id, source_type) in stale_syncs {
            warn!(
                "Marking stale sync {} for source {}",
                sync_run_id, source_id
            );

            if let Some(connector_url) =
                get_connector_url_for_source(&self.redis_client, source_type).await
            {
                if let Err(e) = self
                    .connector_client
                    .cancel_sync(&connector_url, &sync_run_id)
                    .await
                {
                    warn!(
                        "Failed to cancel stale sync {} on connector: {}",
                        sync_run_id, e
                    );
                }
            }

            if let Err(e) = self
                .mark_sync_failed(&sync_run_id, "Sync timed out (no activity detected)")
                .await
            {
                error!("Failed to mark sync as stale: {}", e);
                continue;
            }

            marked_stale.push(sync_run_id);
        }

        Ok(marked_stale)
    }

    async fn mark_sync_failed(&self, sync_run_id: &str, error: &str) -> Result<(), SyncError> {
        self.sync_run_repo
            .mark_failed(sync_run_id, error)
            .await
            .map_err(|e| SyncError::DatabaseError(e.to_string()))?;

        self.resume_attempts.remove(sync_run_id);
        Ok(())
    }
}

#[derive(Debug, thiserror::Error)]
pub enum SyncError {
    #[error("Source not found: {0}")]
    SourceNotFound(String),

    #[error("Source is inactive: {0}")]
    SourceInactive(String),

    #[error("Sync already running for source: {0}")]
    SyncAlreadyRunning(String),

    #[error("Sync run not found: {0}")]
    SyncRunNotFound(String),

    #[error("Sync is not running: {0}")]
    SyncNotRunning(String),

    #[error("Connector not configured for type: {0}")]
    ConnectorNotConfigured(String),

    #[error("Concurrency limit reached")]
    ConcurrencyLimitReached,

    #[error("Database error: {0}")]
    DatabaseError(String),

    #[error("Connector error: {0}")]
    ConnectorError(#[from] ClientError),
}

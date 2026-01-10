use crate::config::ConnectorManagerConfig;
use crate::connector_client::{ClientError, ConnectorClient};
use crate::models::{SyncRequest, TriggerType};
use dashmap::DashSet;
use shared::{DatabasePool, Repository, SourceRepository};
use sqlx::PgPool;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use time::OffsetDateTime;
use tracing::{debug, error, info, warn};

#[derive(Clone)]
pub struct SyncManager {
    pool: PgPool,
    config: ConnectorManagerConfig,
    connector_client: ConnectorClient,
    running_syncs: Arc<DashSet<String>>,
    active_sync_count: Arc<AtomicUsize>,
}

impl SyncManager {
    pub fn new(db_pool: &DatabasePool, config: ConnectorManagerConfig) -> Self {
        Self {
            pool: db_pool.pool().clone(),
            config,
            connector_client: ConnectorClient::new(),
            running_syncs: Arc::new(DashSet::new()),
            active_sync_count: Arc::new(AtomicUsize::new(0)),
        }
    }

    pub async fn trigger_sync(
        &self,
        source_id: &str,
        sync_mode: Option<String>,
        trigger_type: TriggerType,
    ) -> Result<String, SyncError> {
        // Check if sync is already running for this source
        if self.running_syncs.contains(source_id) {
            return Err(SyncError::SyncAlreadyRunning(source_id.to_string()));
        }

        // Check global concurrency limit
        let current_count = self.active_sync_count.load(Ordering::SeqCst);
        if current_count >= self.config.max_concurrent_syncs {
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

        // Get connector URL
        let connector_url = self
            .config
            .get_connector_url(source.source_type)
            .ok_or_else(|| SyncError::ConnectorNotConfigured(format!("{:?}", source.source_type)))?
            .clone();

        // Create sync run
        let sync_run_id = shared::utils::generate_ulid();
        let sync_type = sync_mode.as_deref().unwrap_or("incremental");

        self.create_sync_run(&sync_run_id, source_id, sync_type, trigger_type)
            .await?;

        // Mark source as syncing
        self.running_syncs.insert(source_id.to_string());
        self.active_sync_count.fetch_add(1, Ordering::SeqCst);

        // Build sync request - connectors fetch their own config/credentials from DB
        let sync_request = SyncRequest {
            sync_run_id: sync_run_id.clone(),
            source_id: source_id.to_string(),
            sync_mode: sync_type.to_string(),
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
                Ok(sync_run_id)
            }
            Err(e) => {
                // Clean up on failure
                self.running_syncs.remove(source_id);
                self.active_sync_count.fetch_sub(1, Ordering::SeqCst);
                self.mark_sync_failed(&sync_run_id, &e.to_string()).await?;
                Err(SyncError::ConnectorError(e))
            }
        }
    }

    pub async fn cancel_sync(&self, sync_run_id: &str) -> Result<(), SyncError> {
        // Get sync run details
        let sync_run = self.get_sync_run(sync_run_id).await?;

        if sync_run.status != "running" {
            return Err(SyncError::SyncNotRunning(sync_run_id.to_string()));
        }

        // Get connector URL
        let source_repo = SourceRepository::new(&self.pool);
        let source = Repository::find_by_id(&source_repo, sync_run.source_id.clone())
            .await
            .map_err(|e| SyncError::DatabaseError(e.to_string()))?
            .ok_or_else(|| SyncError::SourceNotFound(sync_run.source_id.clone()))?;

        let connector_url = self
            .config
            .get_connector_url(source.source_type)
            .ok_or_else(|| SyncError::ConnectorNotConfigured(format!("{:?}", source.source_type)))?
            .clone();

        // Send cancel request to connector
        if let Err(e) = self
            .connector_client
            .cancel_sync(&connector_url, sync_run_id)
            .await
        {
            warn!("Failed to send cancel request to connector: {}", e);
        }

        // Mark sync as cancelled
        self.mark_sync_cancelled(sync_run_id).await?;

        // Clean up tracking
        self.running_syncs.remove(&sync_run.source_id);
        self.active_sync_count.fetch_sub(1, Ordering::SeqCst);

        info!("Sync {} cancelled", sync_run_id);
        Ok(())
    }

    pub fn is_sync_running(&self, source_id: &str) -> bool {
        self.running_syncs.contains(source_id)
    }

    pub fn active_sync_count(&self) -> usize {
        self.active_sync_count.load(Ordering::SeqCst)
    }

    pub async fn handle_sync_completed(&self, source_id: &str, sync_run_id: &str) {
        debug!("Sync {} completed for source {}", sync_run_id, source_id);
        self.running_syncs.remove(source_id);
        self.active_sync_count.fetch_sub(1, Ordering::SeqCst);
    }

    pub async fn detect_stale_syncs(&self) -> Result<Vec<String>, SyncError> {
        let timeout_minutes = self.config.stale_sync_timeout_minutes as i64;
        let cutoff = OffsetDateTime::now_utc() - time::Duration::minutes(timeout_minutes);

        let stale_syncs: Vec<(String, String)> = sqlx::query_as(
            r#"
            SELECT id, source_id FROM sync_runs
            WHERE status = 'running'
            AND (last_activity_at IS NULL OR last_activity_at < $1)
            "#,
        )
        .bind(cutoff)
        .fetch_all(&self.pool)
        .await
        .map_err(|e| SyncError::DatabaseError(e.to_string()))?;

        let mut marked_stale = Vec::new();
        for (sync_run_id, source_id) in stale_syncs {
            warn!(
                "Marking stale sync {} for source {}",
                sync_run_id, source_id
            );

            if let Err(e) = self
                .mark_sync_failed(&sync_run_id, "Sync timed out (no activity detected)")
                .await
            {
                error!("Failed to mark sync as stale: {}", e);
                continue;
            }

            self.running_syncs.remove(&source_id);
            self.active_sync_count.fetch_sub(1, Ordering::SeqCst);
            marked_stale.push(sync_run_id);
        }

        Ok(marked_stale)
    }

    async fn create_sync_run(
        &self,
        id: &str,
        source_id: &str,
        sync_type: &str,
        trigger_type: TriggerType,
    ) -> Result<(), SyncError> {
        let now = OffsetDateTime::now_utc();

        sqlx::query(
            r#"
            INSERT INTO sync_runs (id, source_id, sync_type, status, trigger_type, queued_at, started_at, last_activity_at)
            VALUES ($1, $2, $3, 'running', $4, $5, $5, $5)
            "#,
        )
        .bind(id)
        .bind(source_id)
        .bind(sync_type)
        .bind(trigger_type.to_string())
        .bind(now)
        .execute(&self.pool)
        .await
        .map_err(|e| SyncError::DatabaseError(e.to_string()))?;

        // Update source sync_status
        sqlx::query("UPDATE sources SET sync_status = 'syncing' WHERE id = $1")
            .bind(source_id)
            .execute(&self.pool)
            .await
            .map_err(|e| SyncError::DatabaseError(e.to_string()))?;

        Ok(())
    }

    async fn mark_sync_failed(&self, sync_run_id: &str, error: &str) -> Result<(), SyncError> {
        let now = OffsetDateTime::now_utc();

        sqlx::query(
            r#"
            UPDATE sync_runs
            SET status = 'failed', completed_at = $1, error_message = $2, updated_at = $1
            WHERE id = $3
            "#,
        )
        .bind(now)
        .bind(error)
        .bind(sync_run_id)
        .execute(&self.pool)
        .await
        .map_err(|e| SyncError::DatabaseError(e.to_string()))?;

        // Update source status
        let sync_run = self.get_sync_run(sync_run_id).await?;
        sqlx::query("UPDATE sources SET sync_status = 'failed', sync_error = $1 WHERE id = $2")
            .bind(error)
            .bind(&sync_run.source_id)
            .execute(&self.pool)
            .await
            .map_err(|e| SyncError::DatabaseError(e.to_string()))?;

        Ok(())
    }

    async fn mark_sync_cancelled(&self, sync_run_id: &str) -> Result<(), SyncError> {
        let now = OffsetDateTime::now_utc();

        sqlx::query(
            r#"
            UPDATE sync_runs
            SET status = 'cancelled', completed_at = $1, error_message = 'Cancelled by user', updated_at = $1
            WHERE id = $2
            "#,
        )
        .bind(now)
        .bind(sync_run_id)
        .execute(&self.pool)
        .await
        .map_err(|e| SyncError::DatabaseError(e.to_string()))?;

        // Update source status
        let sync_run = self.get_sync_run(sync_run_id).await?;
        sqlx::query("UPDATE sources SET sync_status = 'pending' WHERE id = $1")
            .bind(&sync_run.source_id)
            .execute(&self.pool)
            .await
            .map_err(|e| SyncError::DatabaseError(e.to_string()))?;

        Ok(())
    }

    async fn get_sync_run(&self, sync_run_id: &str) -> Result<SyncRunInfo, SyncError> {
        sqlx::query_as::<_, SyncRunInfo>(
            "SELECT id, source_id, status FROM sync_runs WHERE id = $1",
        )
        .bind(sync_run_id)
        .fetch_optional(&self.pool)
        .await
        .map_err(|e| SyncError::DatabaseError(e.to_string()))?
        .ok_or_else(|| SyncError::SyncRunNotFound(sync_run_id.to_string()))
    }
}

#[derive(Debug, sqlx::FromRow)]
struct SyncRunInfo {
    id: String,
    source_id: String,
    status: String,
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

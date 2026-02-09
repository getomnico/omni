use crate::config::ConnectorManagerConfig;
use crate::connector_client::{ClientError, ConnectorClient};
use crate::models::{SyncRequest, TriggerType};
use shared::db::repositories::SyncRunRepository;
use shared::models::{SyncStatus, SyncType};
use shared::{DatabasePool, Repository, SourceRepository};
use sqlx::PgPool;
use time::OffsetDateTime;
use tracing::{error, info, warn};

#[derive(Clone)]
pub struct SyncManager {
    pool: PgPool,
    config: ConnectorManagerConfig,
    connector_client: ConnectorClient,
    sync_run_repo: SyncRunRepository,
}

impl SyncManager {
    pub fn new(db_pool: &DatabasePool, config: ConnectorManagerConfig) -> Self {
        Self {
            pool: db_pool.pool().clone(),
            config,
            connector_client: ConnectorClient::new(),
            sync_run_repo: SyncRunRepository::new(db_pool.pool()),
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

        // Get connector URL
        let connector_url = self
            .config
            .get_connector_url(source.source_type)
            .ok_or_else(|| SyncError::ConnectorNotConfigured(format!("{:?}", source.source_type)))?
            .clone();

        let sync_run = self
            .sync_run_repo
            .create(source_id, sync_type, &trigger_type.to_string())
            .await
            .map_err(|e| SyncError::DatabaseError(e.to_string()))?;

        self.update_source_sync_status(source_id, "syncing").await?;

        let sync_request = SyncRequest {
            sync_run_id: sync_run.id.clone(),
            source_id: source_id.to_string(),
            // TODO: Change type of sync_mode to SyncType
            sync_mode: match sync_type {
                SyncType::Full => "full",
                SyncType::Incremental => "incremental",
            }
            .to_string(),
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

        let connector_url = self
            .config
            .get_connector_url(source.source_type)
            .ok_or_else(|| SyncError::ConnectorNotConfigured(format!("{:?}", source.source_type)))?
            .clone();

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
        self.update_source_sync_status(&sync_run.source_id, "pending")
            .await?;

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

            marked_stale.push(sync_run_id);
        }

        Ok(marked_stale)
    }

    async fn mark_sync_failed(&self, sync_run_id: &str, error: &str) -> Result<(), SyncError> {
        self.sync_run_repo
            .mark_failed(sync_run_id, error)
            .await
            .map_err(|e| SyncError::DatabaseError(e.to_string()))?;

        let sync_run = self
            .sync_run_repo
            .find_by_id(sync_run_id)
            .await
            .map_err(|e| SyncError::DatabaseError(e.to_string()))?
            .ok_or_else(|| SyncError::SyncRunNotFound(sync_run_id.to_string()))?;

        sqlx::query("UPDATE sources SET sync_status = 'failed', sync_error = $1 WHERE id = $2")
            .bind(error)
            .bind(&sync_run.source_id)
            .execute(&self.pool)
            .await
            .map_err(|e| SyncError::DatabaseError(e.to_string()))?;

        Ok(())
    }

    async fn update_source_sync_status(
        &self,
        source_id: &str,
        status: &str,
    ) -> Result<(), SyncError> {
        sqlx::query("UPDATE sources SET sync_status = $1 WHERE id = $2")
            .bind(status)
            .bind(source_id)
            .execute(&self.pool)
            .await
            .map_err(|e| SyncError::DatabaseError(e.to_string()))?;
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

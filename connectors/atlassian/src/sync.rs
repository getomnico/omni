use anyhow::{anyhow, Result};
use chrono::{DateTime, Utc};
use redis::{AsyncCommands, Client as RedisClient};
use shared::db::repositories::ServiceCredentialsRepo;
use shared::models::{ServiceCredentials, ServiceProvider, Source, SourceType};
use shared::{Repository, SourceRepository};
use sqlx::PgPool;
use std::collections::HashSet;
use tracing::{debug, error, info};

use crate::auth::{AtlassianCredentials, AuthManager};
use crate::confluence::ConfluenceProcessor;
use crate::jira::JiraProcessor;
use shared::SdkClient;

pub struct SyncManager {
    source_repo: SourceRepository,
    service_credentials_repo: ServiceCredentialsRepo,
    auth_manager: AuthManager,
    confluence_processor: ConfluenceProcessor,
    jira_processor: JiraProcessor,
}

pub struct SyncState {
    redis_client: RedisClient,
}

impl SyncState {
    pub fn new(redis_client: RedisClient) -> Self {
        Self { redis_client }
    }

    pub fn get_confluence_sync_key(&self, source_id: &str, space_key: &str) -> String {
        format!("atlassian:confluence:sync:{}:{}", source_id, space_key)
    }

    pub fn get_jira_sync_key(&self, source_id: &str, project_key: &str) -> String {
        format!("atlassian:jira:sync:{}:{}", source_id, project_key)
    }

    pub fn get_test_confluence_sync_key(&self, source_id: &str, space_key: &str) -> String {
        format!("atlassian:confluence:sync:test:{}:{}", source_id, space_key)
    }

    pub fn get_test_jira_sync_key(&self, source_id: &str, project_key: &str) -> String {
        format!("atlassian:jira:sync:test:{}:{}", source_id, project_key)
    }

    pub async fn get_confluence_last_sync(
        &self,
        source_id: &str,
        space_key: &str,
    ) -> Result<Option<DateTime<Utc>>> {
        let mut conn = self.redis_client.get_multiplexed_async_connection().await?;
        let key = if cfg!(test) {
            self.get_test_confluence_sync_key(source_id, space_key)
        } else {
            self.get_confluence_sync_key(source_id, space_key)
        };

        let result: Option<String> = conn.get(&key).await?;
        if let Some(timestamp_str) = result {
            if let Ok(timestamp) = timestamp_str.parse::<i64>() {
                if let Some(dt) = DateTime::from_timestamp(timestamp, 0) {
                    return Ok(Some(dt));
                }
            }
        }
        Ok(None)
    }

    pub async fn set_confluence_last_sync(
        &self,
        source_id: &str,
        space_key: &str,
        sync_time: DateTime<Utc>,
    ) -> Result<()> {
        let mut conn = self.redis_client.get_multiplexed_async_connection().await?;
        let key = if cfg!(test) {
            self.get_test_confluence_sync_key(source_id, space_key)
        } else {
            self.get_confluence_sync_key(source_id, space_key)
        };

        let timestamp = sync_time.timestamp();
        let _: () = conn.set_ex(&key, timestamp, 30 * 24 * 60 * 60).await?; // 30 days expiry
        Ok(())
    }

    pub async fn get_jira_last_sync(
        &self,
        source_id: &str,
        project_key: &str,
    ) -> Result<Option<DateTime<Utc>>> {
        let mut conn = self.redis_client.get_multiplexed_async_connection().await?;
        let key = if cfg!(test) {
            self.get_test_jira_sync_key(source_id, project_key)
        } else {
            self.get_jira_sync_key(source_id, project_key)
        };

        let result: Option<String> = conn.get(&key).await?;
        if let Some(timestamp_str) = result {
            if let Ok(timestamp) = timestamp_str.parse::<i64>() {
                if let Some(dt) = DateTime::from_timestamp(timestamp, 0) {
                    return Ok(Some(dt));
                }
            }
        }
        Ok(None)
    }

    pub async fn set_jira_last_sync(
        &self,
        source_id: &str,
        project_key: &str,
        sync_time: DateTime<Utc>,
    ) -> Result<()> {
        let mut conn = self.redis_client.get_multiplexed_async_connection().await?;
        let key = if cfg!(test) {
            self.get_test_jira_sync_key(source_id, project_key)
        } else {
            self.get_jira_sync_key(source_id, project_key)
        };

        let timestamp = sync_time.timestamp();
        let _: () = conn.set_ex(&key, timestamp, 30 * 24 * 60 * 60).await?; // 30 days expiry
        Ok(())
    }

    pub async fn get_all_synced_confluence_spaces(
        &self,
        source_id: &str,
    ) -> Result<HashSet<String>> {
        let mut conn = self.redis_client.get_multiplexed_async_connection().await?;
        let pattern = if cfg!(test) {
            format!("atlassian:confluence:sync:test:{}:*", source_id)
        } else {
            format!("atlassian:confluence:sync:{}:*", source_id)
        };

        let keys: Vec<String> = conn.keys(&pattern).await?;
        let prefix = if cfg!(test) {
            format!("atlassian:confluence:sync:test:{}:", source_id)
        } else {
            format!("atlassian:confluence:sync:{}:", source_id)
        };

        let space_keys: HashSet<String> = keys
            .into_iter()
            .filter_map(|key| key.strip_prefix(&prefix).map(|s| s.to_string()))
            .collect();

        Ok(space_keys)
    }

    pub async fn get_all_synced_jira_projects(&self, source_id: &str) -> Result<HashSet<String>> {
        let mut conn = self.redis_client.get_multiplexed_async_connection().await?;
        let pattern = if cfg!(test) {
            format!("atlassian:jira:sync:test:{}:*", source_id)
        } else {
            format!("atlassian:jira:sync:{}:*", source_id)
        };

        let keys: Vec<String> = conn.keys(&pattern).await?;
        let prefix = if cfg!(test) {
            format!("atlassian:jira:sync:test:{}:", source_id)
        } else {
            format!("atlassian:jira:sync:{}:", source_id)
        };

        let project_keys: HashSet<String> = keys
            .into_iter()
            .filter_map(|key| key.strip_prefix(&prefix).map(|s| s.to_string()))
            .collect();

        Ok(project_keys)
    }

    pub fn get_confluence_page_sync_key(
        &self,
        source_id: &str,
        space_id: &str,
        page_id: &str,
    ) -> String {
        if cfg!(test) {
            format!(
                "atlassian:confluence:page:test:{}:{}:{}",
                source_id, space_id, page_id
            )
        } else {
            format!(
                "atlassian:confluence:page:{}:{}:{}",
                source_id, space_id, page_id
            )
        }
    }

    pub async fn get_confluence_page_version(
        &self,
        source_id: &str,
        space_id: &str,
        page_id: &str,
    ) -> Result<Option<i32>> {
        let mut conn = self.redis_client.get_multiplexed_async_connection().await?;
        let key = self.get_confluence_page_sync_key(source_id, space_id, page_id);

        let result: Option<String> = conn.get(&key).await?;
        if let Some(version_str) = result {
            if let Ok(version) = version_str.parse::<i32>() {
                return Ok(Some(version));
            }
        }
        Ok(None)
    }

    pub async fn set_confluence_page_version(
        &self,
        source_id: &str,
        space_id: &str,
        page_id: &str,
        version: i32,
    ) -> Result<()> {
        let mut conn = self.redis_client.get_multiplexed_async_connection().await?;
        let key = self.get_confluence_page_sync_key(source_id, space_id, page_id);

        let _: () = conn.set_ex(&key, version, 30 * 24 * 60 * 60).await?; // 30 days expiry
        Ok(())
    }
}

impl SyncManager {
    pub async fn new(
        pool: PgPool,
        redis_client: RedisClient,
        sdk_client: SdkClient,
    ) -> Result<Self> {
        let source_repo = SourceRepository::new(&pool);
        let service_credentials_repo = ServiceCredentialsRepo::new(pool.clone())?;
        let sync_run_repo = shared::db::repositories::SyncRunRepository::new(&pool);

        Ok(Self {
            source_repo,
            service_credentials_repo,
            auth_manager: AuthManager::new(),
            confluence_processor: ConfluenceProcessor::new(
                sdk_client.clone(),
                sync_run_repo.clone(),
                redis_client.clone(),
            ),
            jira_processor: JiraProcessor::new(sdk_client.clone(), sync_run_repo.clone()),
        })
    }

    pub async fn sync_all_sources(&mut self) -> Result<()> {
        let sources = self.get_active_sources().await?;

        info!("Found {} active Atlassian sources", sources.len());

        for source in sources {
            if let Err(e) = self.sync_source(&source).await {
                error!("Failed to sync source {}: {:?}", source.id, e);
                self.update_source_status(&source.id, "failed", None, Some(e.to_string()))
                    .await?;
            }
        }

        Ok(())
    }

    pub async fn sync_source_by_id(&mut self, source_id: String) -> Result<()> {
        let source = self
            .source_repo
            .find_by_id(source_id.clone())
            .await?
            .ok_or_else(|| anyhow::anyhow!("Source not found: {}", source_id))?;

        if !source.is_active {
            return Err(anyhow::anyhow!("Source is not active: {}", source_id));
        }

        let source_type = source.source_type.clone();
        if source_type != SourceType::Confluence && source_type != SourceType::Jira {
            return Err(anyhow::anyhow!(
                "Invalid source type for Atlassian connector: {:?}",
                source_type
            ));
        }

        self.sync_source(&source).await
    }

    async fn sync_source(&mut self, source: &Source) -> Result<()> {
        info!("Starting sync for Atlassian source: {}", source.name);

        let service_creds = self.get_service_credentials(&source.id).await?;
        let (base_url, user_email, api_token) =
            self.extract_atlassian_credentials(&service_creds)?;

        debug!("Validating Atlassian credentials...");
        let mut credentials = self
            .get_or_validate_credentials(&base_url, &user_email, &api_token)
            .await?;
        debug!("Successfully validated Atlassian credentials.");

        self.auth_manager
            .ensure_valid_credentials(&mut credentials)
            .await?;

        // Extract source-specific config based on source type
        let (_confluence_config, _jira_config) = match source.source_type {
            SourceType::Confluence => {
                let config: shared::ConfluenceSourceConfig =
                    serde_json::from_value(source.config.clone()).map_err(|e| {
                        anyhow::anyhow!("Failed to parse Confluence source config: {}", e)
                    })?;
                (Some(config), None)
            }
            SourceType::Jira => {
                let config: shared::JiraSourceConfig =
                    serde_json::from_value(source.config.clone()).map_err(|e| {
                        anyhow::anyhow!("Failed to parse JIRA source config: {}", e)
                    })?;
                (None, Some(config))
            }
            _ => (None, None),
        };

        let sync_start = Utc::now();
        self.update_source_status(&source.id, "syncing", None, None)
            .await?;

        // Determine sync strategy based on last sync time
        let should_do_full_sync = source.last_sync_at.is_none()
            || source
                .last_sync_at
                .map(|last| {
                    let last_utc =
                        DateTime::from_timestamp(last.unix_timestamp(), 0).unwrap_or_default();
                    (sync_start - last_utc).num_hours() > 24
                })
                .unwrap_or(true);

        let mut total_processed = 0;

        if should_do_full_sync {
            info!("Performing full sync for source: {}", source.name);

            if source.source_type == SourceType::Confluence {
                match self
                    .confluence_processor
                    .sync_all_spaces(&credentials, &source.id)
                    .await
                {
                    Ok(pages_count) => {
                        total_processed += pages_count;
                        info!("Full Confluence sync completed: {} pages", pages_count);
                    }
                    Err(e) => {
                        error!("Full Confluence sync failed: {}", e);
                    }
                }
            } else if source.source_type == SourceType::Jira {
                match self
                    .jira_processor
                    .sync_all_projects(&credentials, &source.id)
                    .await
                {
                    Ok(issues_count) => {
                        total_processed += issues_count;
                        info!("Full JIRA sync completed: {} issues", issues_count);
                    }
                    Err(e) => {
                        error!("Full JIRA sync failed: {}", e);
                    }
                }
            } else {
                error!("Unsupported source type: {:?}", source.source_type);
                return Err(anyhow!("Unsupported source type: {:?}", source.source_type));
            }
        } else {
            info!("Performing incremental sync for source: {}", source.name);

            // Get last sync time for incremental sync
            let last_sync = source
                .last_sync_at
                .and_then(|last| DateTime::from_timestamp(last.unix_timestamp(), 0))
                .unwrap_or_else(|| sync_start - chrono::Duration::hours(24));

            if source.source_type == SourceType::Confluence {
                // Confluence incremental sync uses same flow as full sync
                // but version checking in process_pages() skips unchanged pages
                match self
                    .confluence_processor
                    .sync_all_spaces_incremental(&credentials, &source.id)
                    .await
                {
                    Ok(pages_count) => {
                        total_processed += pages_count;
                        info!(
                            "Incremental Confluence sync completed: {} pages",
                            pages_count
                        );
                    }
                    Err(e) => {
                        error!("Incremental Confluence sync failed: {}", e);
                    }
                }
            } else if source.source_type == SourceType::Jira {
                // Jira incremental sync uses JQL to fetch only updated issues
                match self
                    .jira_processor
                    .sync_issues_updated_since(&credentials, &source.id, last_sync, None)
                    .await
                {
                    Ok(issues_count) => {
                        total_processed += issues_count;
                        info!("Incremental JIRA sync completed: {} issues", issues_count);
                    }
                    Err(e) => {
                        error!("Incremental JIRA sync failed: {}", e);
                    }
                }
            }
        }

        // Update source status
        if total_processed > 0 {
            self.update_source_status(&source.id, "completed", Some(sync_start), None)
                .await?;
            info!(
                "Successfully synced {} documents from source: {}",
                total_processed, source.name
            );
        } else {
            self.update_source_status(&source.id, "completed", Some(sync_start), None)
                .await?;
            info!(
                "Sync completed with no new documents for source: {}",
                source.name
            );
        }

        Ok(())
    }

    async fn get_active_sources(&self) -> Result<Vec<Source>> {
        let sources = self
            .source_repo
            .find_active_by_types(vec![SourceType::Confluence, SourceType::Jira])
            .await?;

        Ok(sources)
    }

    async fn get_service_credentials(&self, source_id: &str) -> Result<ServiceCredentials> {
        let creds = self
            .service_credentials_repo
            .get_by_source_id(source_id)
            .await?
            .ok_or_else(|| {
                anyhow::anyhow!("Service credentials not found for source {}", source_id)
            })?;

        if creds.provider != ServiceProvider::Atlassian {
            return Err(anyhow::anyhow!(
                "Expected Atlassian credentials for source {}, found {:?}",
                source_id,
                creds.provider
            ));
        }

        Ok(creds)
    }

    fn extract_atlassian_credentials(
        &self,
        creds: &ServiceCredentials,
    ) -> Result<(String, String, String)> {
        let base_url = creds
            .config
            .get("base_url")
            .and_then(|v| v.as_str())
            .ok_or_else(|| anyhow::anyhow!("Missing base_url in service credentials config"))?
            .to_string();

        let user_email = creds
            .principal_email
            .as_ref()
            .ok_or_else(|| anyhow::anyhow!("Missing principal_email in service credentials"))?
            .to_string();

        let api_token = creds
            .credentials
            .get("api_token")
            .and_then(|v| v.as_str())
            .ok_or_else(|| anyhow::anyhow!("Missing api_token in service credentials"))?
            .to_string();

        Ok((base_url, user_email, api_token))
    }

    async fn get_or_validate_credentials(
        &self,
        base_url: &str,
        user_email: &str,
        api_token: &str,
    ) -> Result<AtlassianCredentials> {
        // Always validate credentials to ensure they're working
        self.auth_manager
            .validate_credentials(base_url, user_email, api_token)
            .await
    }

    async fn update_source_status(
        &self,
        source_id: &str,
        status: &str,
        last_sync_at: Option<DateTime<Utc>>,
        sync_error: Option<String>,
    ) -> Result<()> {
        self.source_repo
            .update_sync_status(source_id, status, last_sync_at, sync_error)
            .await?;

        Ok(())
    }

    pub async fn test_connection(
        &self,
        config: &(String, String, String),
    ) -> Result<(Vec<String>, Vec<String>)> {
        let (base_url, user_email, api_token) = config;
        let credentials = self
            .get_or_validate_credentials(base_url, user_email, api_token)
            .await?;

        let jira_projects = self
            .auth_manager
            .test_jira_permissions(&credentials)
            .await?;
        let confluence_spaces = self
            .auth_manager
            .test_confluence_permissions(&credentials)
            .await?;

        Ok((jira_projects, confluence_spaces))
    }
}

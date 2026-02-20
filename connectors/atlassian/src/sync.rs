use anyhow::{anyhow, Context, Result};
use chrono::{DateTime, Utc};
use dashmap::DashMap;
use redis::{AsyncCommands, Client as RedisClient};
use shared::models::{ServiceCredentials, ServiceProvider, SourceType, SyncRequest, SyncType};
use std::collections::HashSet;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use tracing::{debug, error, info, warn};

use crate::auth::{AtlassianCredentials, AuthManager};
use crate::client::AtlassianApi;
use crate::confluence::ConfluenceProcessor;
use crate::jira::JiraProcessor;
use crate::models::{AtlassianConnectorState, AtlassianWebhookEvent};
use shared::SdkClient;

pub struct SyncManager {
    sdk_client: SdkClient,
    auth_manager: AuthManager,
    client: Arc<dyn AtlassianApi>,
    confluence_processor: ConfluenceProcessor,
    jira_processor: JiraProcessor,
    active_syncs: DashMap<String, Arc<AtomicBool>>,
    webhook_url: Option<String>,
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
    pub fn new(
        redis_client: RedisClient,
        sdk_client: SdkClient,
        webhook_url: Option<String>,
    ) -> Self {
        let client: Arc<dyn AtlassianApi> = Arc::new(crate::client::AtlassianClient::new());
        Self::with_client(client, redis_client, sdk_client, webhook_url)
    }

    pub fn with_client(
        client: Arc<dyn AtlassianApi>,
        redis_client: RedisClient,
        sdk_client: SdkClient,
        webhook_url: Option<String>,
    ) -> Self {
        Self {
            sdk_client: sdk_client.clone(),
            auth_manager: AuthManager::new(),
            confluence_processor: ConfluenceProcessor::new(
                client.clone(),
                sdk_client.clone(),
                redis_client.clone(),
            ),
            jira_processor: JiraProcessor::new(client.clone(), sdk_client),
            client,
            active_syncs: DashMap::new(),
            webhook_url,
        }
    }

    pub fn cancel_sync(&self, sync_run_id: &str) -> bool {
        if let Some(cancelled) = self.active_syncs.get(sync_run_id) {
            cancelled.store(true, Ordering::SeqCst);
            true
        } else {
            false
        }
    }

    /// Execute a sync based on the request from connector-manager
    pub async fn sync_source(&mut self, request: SyncRequest) -> Result<()> {
        let sync_run_id = &request.sync_run_id;
        let source_id = &request.source_id;

        info!(
            "Starting sync for source: {} (sync_run_id: {})",
            source_id, sync_run_id
        );

        // Fetch source via SDK
        let source = self
            .sdk_client
            .get_source(source_id)
            .await
            .context("Failed to fetch source via SDK")?;

        if !source.is_active {
            let err_msg = format!("Source is not active: {}", source_id);
            self.sdk_client.fail(sync_run_id, &err_msg).await?;
            return Err(anyhow::anyhow!(err_msg));
        }

        let source_type = source.source_type.clone();
        if source_type != SourceType::Confluence && source_type != SourceType::Jira {
            let err_msg = format!(
                "Invalid source type for Atlassian connector: {:?}",
                source_type
            );
            self.sdk_client.fail(sync_run_id, &err_msg).await?;
            return Err(anyhow::anyhow!(err_msg));
        }

        // Fetch and validate credentials
        let service_creds = self.get_service_credentials(source_id).await?;
        let (base_url, user_email, api_token) =
            self.extract_atlassian_credentials(&service_creds)?;

        debug!("Validating Atlassian credentials...");
        let mut credentials = match self
            .get_or_validate_credentials(&base_url, &user_email, &api_token)
            .await
        {
            Ok(creds) => creds,
            Err(e) => {
                self.sdk_client.fail(sync_run_id, &e.to_string()).await?;
                return Err(e);
            }
        };
        debug!("Successfully validated Atlassian credentials.");

        if let Err(e) = self
            .auth_manager
            .ensure_valid_credentials(&mut credentials)
            .await
        {
            self.sdk_client.fail(sync_run_id, &e.to_string()).await?;
            return Err(e);
        }

        let cancelled = Arc::new(AtomicBool::new(false));
        self.active_syncs
            .insert(sync_run_id.to_string(), cancelled.clone());

        let sync_start = Utc::now();
        let is_full_sync = request.sync_mode == "full";
        let last_sync_time = request
            .last_sync_at
            .as_deref()
            .and_then(|s| DateTime::parse_from_rfc3339(s).ok())
            .map(|dt| dt.with_timezone(&Utc));

        let result = if is_full_sync {
            info!("Performing full sync for source: {}", source.name);
            self.execute_full_sync(
                &credentials,
                source_id,
                sync_run_id,
                &source.source_type,
                &cancelled,
            )
            .await
        } else {
            info!("Performing incremental sync for source: {}", source.name);
            let last_sync =
                last_sync_time.unwrap_or_else(|| sync_start - chrono::Duration::hours(24));

            self.execute_incremental_sync(
                &credentials,
                source_id,
                sync_run_id,
                &source.source_type,
                last_sync,
                &cancelled,
            )
            .await
        };

        if cancelled.load(Ordering::SeqCst) {
            info!("Sync {} was cancelled", sync_run_id);
            let _ = self.sdk_client.cancel(sync_run_id).await;
            self.active_syncs.remove(sync_run_id);
            return Ok(());
        }

        self.active_syncs.remove(sync_run_id);

        match result {
            Ok(total_processed) => {
                info!(
                    "Sync completed for source {}: {} documents processed",
                    source.name, total_processed
                );
                self.sdk_client
                    .complete(
                        sync_run_id,
                        total_processed as i32,
                        total_processed as i32,
                        None,
                    )
                    .await?;

                if let Err(e) = self
                    .ensure_webhook_registered(source_id, &credentials)
                    .await
                {
                    warn!("Failed to register webhook for source {}: {}", source_id, e);
                }

                Ok(())
            }
            Err(e) => {
                error!("Sync failed for source {}: {}", source.name, e);
                self.sdk_client.fail(sync_run_id, &e.to_string()).await?;
                Err(e)
            }
        }
    }

    async fn execute_full_sync(
        &mut self,
        credentials: &AtlassianCredentials,
        source_id: &str,
        sync_run_id: &str,
        source_type: &SourceType,
        cancelled: &AtomicBool,
    ) -> Result<u32> {
        match source_type {
            SourceType::Confluence => {
                self.confluence_processor
                    .sync_all_spaces(credentials, source_id, sync_run_id, cancelled)
                    .await
            }
            SourceType::Jira => {
                self.jira_processor
                    .sync_all_projects(credentials, source_id, sync_run_id, cancelled)
                    .await
            }
            _ => Err(anyhow!("Unsupported source type: {:?}", source_type)),
        }
    }

    async fn execute_incremental_sync(
        &mut self,
        credentials: &AtlassianCredentials,
        source_id: &str,
        sync_run_id: &str,
        source_type: &SourceType,
        last_sync: DateTime<Utc>,
        cancelled: &AtomicBool,
    ) -> Result<u32> {
        match source_type {
            SourceType::Confluence => {
                self.confluence_processor
                    .sync_all_spaces_incremental(
                        credentials,
                        source_id,
                        sync_run_id,
                        last_sync,
                        cancelled,
                    )
                    .await
            }
            SourceType::Jira => {
                self.jira_processor
                    .sync_issues_updated_since(
                        credentials,
                        source_id,
                        last_sync,
                        None,
                        sync_run_id,
                        cancelled,
                    )
                    .await
            }
            _ => Err(anyhow!("Unsupported source type: {:?}", source_type)),
        }
    }

    async fn get_service_credentials(&self, source_id: &str) -> Result<ServiceCredentials> {
        let creds = self
            .sdk_client
            .get_credentials(source_id)
            .await
            .context("Failed to fetch credentials via SDK")?;

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
        self.auth_manager
            .validate_credentials(base_url, user_email, api_token)
            .await
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

    pub async fn ensure_webhook_registered(
        &self,
        source_id: &str,
        creds: &AtlassianCredentials,
    ) -> Result<()> {
        let webhook_url = match &self.webhook_url {
            Some(url) => url,
            None => return Ok(()),
        };

        let state: AtlassianConnectorState = self
            .sdk_client
            .get_connector_state(source_id)
            .await
            .ok()
            .flatten()
            .and_then(|v| serde_json::from_value(v).ok())
            .unwrap_or_default();

        if let Some(webhook_id) = state.webhook_id {
            match self.client.get_webhook(creds, webhook_id).await {
                Ok(true) => {
                    debug!(
                        "Webhook {} still exists for source {}",
                        webhook_id, source_id
                    );
                    return Ok(());
                }
                Ok(false) => {
                    info!("Webhook {} no longer exists, re-registering", webhook_id);
                }
                Err(e) => {
                    warn!(
                        "Failed to check webhook {}: {}, re-registering",
                        webhook_id, e
                    );
                }
            }
        }

        let full_url = format!("{}?source_id={}", webhook_url, source_id);
        let webhook_id = self.client.register_webhook(creds, &full_url).await?;
        info!("Registered webhook {} for source {}", webhook_id, source_id);

        let new_state = AtlassianConnectorState {
            webhook_id: Some(webhook_id),
        };
        let state_value = serde_json::to_value(&new_state)?;
        self.sdk_client
            .save_connector_state(source_id, state_value)
            .await?;

        Ok(())
    }

    pub async fn handle_webhook_event(
        &mut self,
        source_id: &str,
        event: AtlassianWebhookEvent,
    ) -> Result<()> {
        info!(
            "Handling webhook event '{}' for source {}",
            event.webhook_event, source_id
        );

        match event.webhook_event.as_str() {
            "jira:issue_deleted" => {
                if let Some(issue) = &event.issue {
                    let project_key = issue
                        .fields
                        .as_ref()
                        .and_then(|f| f.project.as_ref())
                        .map(|p| p.key.as_str())
                        .unwrap_or("");

                    if project_key.is_empty() {
                        warn!("Cannot delete issue without project key");
                        return Ok(());
                    }

                    let sync_run_id = self
                        .sdk_client
                        .create_sync_run(source_id, SyncType::Incremental)
                        .await?;

                    let result = self
                        .jira_processor
                        .delete_issue(source_id, &sync_run_id, project_key, &issue.key)
                        .await;

                    match &result {
                        Ok(_) => self.sdk_client.complete(&sync_run_id, 1, 1, None).await?,
                        Err(e) => self.sdk_client.fail(&sync_run_id, &e.to_string()).await?,
                    }
                    result
                } else {
                    Ok(())
                }
            }
            "page_removed" | "page_trashed" => {
                if let Some(page) = &event.page {
                    let space_key = page
                        .space_key
                        .as_deref()
                        .or_else(|| page.space.as_ref().map(|s| s.key.as_str()))
                        .unwrap_or("");

                    if space_key.is_empty() {
                        warn!("Cannot delete page without space key");
                        return Ok(());
                    }

                    let sync_run_id = self
                        .sdk_client
                        .create_sync_run(source_id, SyncType::Incremental)
                        .await?;

                    let result = self
                        .confluence_processor
                        .delete_page(source_id, &sync_run_id, space_key, &page.id)
                        .await;

                    match &result {
                        Ok(_) => self.sdk_client.complete(&sync_run_id, 1, 1, None).await?,
                        Err(e) => self.sdk_client.fail(&sync_run_id, &e.to_string()).await?,
                    }
                    result
                } else {
                    Ok(())
                }
            }
            "jira:issue_created" | "jira:issue_updated" | "page_created" | "page_updated" => {
                self.sdk_client
                    .notify_webhook(source_id, &event.webhook_event)
                    .await?;
                Ok(())
            }
            _ => {
                debug!("Ignoring unhandled webhook event: {}", event.webhook_event);
                Ok(())
            }
        }
    }

    pub async fn ensure_webhooks_for_all_sources(&mut self) {
        let source_types = ["confluence", "jira"];

        for source_type in &source_types {
            let sources = match self.sdk_client.get_sources_by_type(source_type).await {
                Ok(s) => s,
                Err(e) => {
                    debug!("Failed to list {:?} sources: {}", source_type, e);
                    continue;
                }
            };

            for source in sources {
                let source_id = &source.id;
                let service_creds = match self.get_service_credentials(source_id).await {
                    Ok(c) => c,
                    Err(e) => {
                        debug!("Failed to get credentials for source {}: {}", source_id, e);
                        continue;
                    }
                };

                let (base_url, user_email, api_token) =
                    match self.extract_atlassian_credentials(&service_creds) {
                        Ok(c) => c,
                        Err(e) => {
                            debug!("Failed to extract credentials for {}: {}", source_id, e);
                            continue;
                        }
                    };

                let creds = match self
                    .get_or_validate_credentials(&base_url, &user_email, &api_token)
                    .await
                {
                    Ok(c) => c,
                    Err(e) => {
                        debug!("Failed to validate credentials for {}: {}", source_id, e);
                        continue;
                    }
                };

                if let Err(e) = self.ensure_webhook_registered(source_id, &creds).await {
                    warn!("Failed to ensure webhook for source {}: {}", source_id, e);
                }
            }
        }
    }
}

use anyhow::{anyhow, Context, Result};
use dashmap::DashMap;
use redis::{AsyncCommands, Client as RedisClient};
use std::collections::{HashMap, HashSet};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use time::{self, OffsetDateTime};
use tracing::{debug, error, info, warn};

use crate::admin::AdminClient;
use crate::auth::{GoogleAuth, OAuthAuth, ServiceAccountAuth};
use crate::cache::LruFolderCache;
use crate::drive::DriveClient;
use crate::gmail::{GmailClient, MessageFormat};
use crate::models::{
    GmailThread, GoogleConnectorState, SyncRequest, UserFile, WebhookChannel,
    WebhookChannelResponse, WebhookNotification,
};
use shared::models::{
    AuthType, ConnectorEvent, ServiceCredentials, ServiceProvider, Source, SourceType, SyncType,
};
use shared::SdkClient;
use shared::{AIClient, RateLimiter};

struct ActiveSync {
    cancelled: AtomicBool,
}

pub struct SyncManager {
    redis_client: RedisClient,
    drive_client: DriveClient,
    gmail_client: GmailClient,
    admin_client: Arc<AdminClient>,
    pub sdk_client: SdkClient,
    folder_cache: LruFolderCache,
    active_syncs: DashMap<String, Arc<ActiveSync>>,
    webhook_url: Option<String>,
}

#[derive(Clone)]
pub struct SyncState {
    redis_client: RedisClient,
}

impl SyncState {
    pub fn new(redis_client: RedisClient) -> Self {
        Self { redis_client }
    }

    pub fn get_file_sync_key(&self, source_id: &str, file_id: &str) -> String {
        format!("google:drive:{}:{}", source_id, file_id)
    }

    pub fn get_test_file_sync_key(&self, source_id: &str, file_id: &str) -> String {
        format!("google:drive:test:{}:{}", source_id, file_id)
    }

    pub async fn get_file_sync_state(
        &self,
        source_id: &str,
        file_id: &str,
    ) -> Result<Option<String>> {
        let mut conn = self.redis_client.get_multiplexed_async_connection().await?;
        let key = self.get_file_sync_key(source_id, file_id);

        let result: Option<String> = conn.get(&key).await?;
        Ok(result)
    }

    pub async fn set_file_sync_state(
        &self,
        source_id: &str,
        file_id: &str,
        modified_time: &str,
    ) -> Result<()> {
        self.set_file_sync_state_with_expiry(source_id, file_id, modified_time, 30 * 24 * 60 * 60)
            .await
    }

    pub async fn set_file_sync_state_with_expiry(
        &self,
        source_id: &str,
        file_id: &str,
        modified_time: &str,
        expiry_seconds: u64,
    ) -> Result<()> {
        let mut conn = self.redis_client.get_multiplexed_async_connection().await?;
        let key = if cfg!(test) {
            self.get_test_file_sync_key(source_id, file_id)
        } else {
            self.get_file_sync_key(source_id, file_id)
        };

        let _: () = conn.set_ex(&key, modified_time, expiry_seconds).await?;
        Ok(())
    }

    pub async fn delete_file_sync_state(&self, source_id: &str, file_id: &str) -> Result<()> {
        let mut conn = self.redis_client.get_multiplexed_async_connection().await?;
        let key = if cfg!(test) {
            self.get_test_file_sync_key(source_id, file_id)
        } else {
            self.get_file_sync_key(source_id, file_id)
        };

        let _: () = conn.del(&key).await?;
        Ok(())
    }

    pub async fn get_all_synced_file_ids(&self, source_id: &str) -> Result<HashSet<String>> {
        let mut conn = self.redis_client.get_multiplexed_async_connection().await?;
        let pattern = if cfg!(test) {
            format!("google:drive:test:{}:*", source_id)
        } else {
            format!("google:drive:{}:*", source_id)
        };

        let keys: Vec<String> = conn.keys(&pattern).await?;
        let prefix = if cfg!(test) {
            format!("google:drive:test:{}:", source_id)
        } else {
            format!("google:drive:{}:", source_id)
        };
        let file_ids: HashSet<String> = keys
            .into_iter()
            .filter_map(|key| key.strip_prefix(&prefix).map(|s| s.to_string()))
            .collect();

        Ok(file_ids)
    }

    // Gmail thread sync state methods
    pub fn get_thread_sync_key(&self, source_id: &str, thread_id: &str) -> String {
        format!("google:gmail:sync:{}:{}", source_id, thread_id)
    }

    pub fn get_test_thread_sync_key(&self, source_id: &str, thread_id: &str) -> String {
        format!("google:gmail:sync:test:{}:{}", source_id, thread_id)
    }

    pub async fn get_thread_sync_state(
        &self,
        source_id: &str,
        thread_id: &str,
    ) -> Result<Option<String>> {
        let mut conn = self.redis_client.get_multiplexed_async_connection().await?;
        let key = if cfg!(test) {
            self.get_test_thread_sync_key(source_id, thread_id)
        } else {
            self.get_thread_sync_key(source_id, thread_id)
        };

        let result: Option<String> = conn.get(&key).await?;
        Ok(result)
    }

    pub async fn set_thread_sync_state(
        &self,
        source_id: &str,
        thread_id: &str,
        latest_date: &str,
    ) -> Result<()> {
        self.set_thread_sync_state_with_expiry(source_id, thread_id, latest_date, 30 * 24 * 60 * 60)
            .await
    }

    pub async fn set_thread_sync_state_with_expiry(
        &self,
        source_id: &str,
        thread_id: &str,
        latest_date: &str,
        expiry_seconds: u64,
    ) -> Result<()> {
        let mut conn = self.redis_client.get_multiplexed_async_connection().await?;
        let key = if cfg!(test) {
            self.get_test_thread_sync_key(source_id, thread_id)
        } else {
            self.get_thread_sync_key(source_id, thread_id)
        };

        let _: () = conn.set_ex(&key, latest_date, expiry_seconds).await?;
        Ok(())
    }
}

impl SyncManager {
    pub fn new(
        redis_client: RedisClient,
        ai_service_url: String,
        admin_client: Arc<AdminClient>,
        sdk_client: SdkClient,
        webhook_url: Option<String>,
    ) -> Self {
        // Google API Rate limits:
        //   - Drive API (list files, etc.): 12,000 req/min
        //   - Docs API (get content, etc.): 3,000 req/min/project, 300 req/min/user
        // The below rate limit is for the Drive API only.
        // For the Docs API, we need to have a separate rate limiter for each user.
        let api_rate_limit = std::env::var("GOOGLE_API_RATE_LIMIT")
            .unwrap_or_else(|_| "50".to_string())
            .parse::<u32>()
            .unwrap_or(50);

        let max_retries = std::env::var("GOOGLE_MAX_RETRIES")
            .unwrap_or_else(|_| "5".to_string())
            .parse::<u32>()
            .unwrap_or(5);

        let rate_limiter = Arc::new(RateLimiter::new(api_rate_limit, max_retries));
        let ai_client = AIClient::new(ai_service_url);
        let drive_client = DriveClient::with_rate_limiter(rate_limiter.clone(), ai_client.clone());
        let gmail_client = GmailClient::with_rate_limiter(rate_limiter);

        Self {
            redis_client,
            drive_client,
            gmail_client,
            admin_client,
            sdk_client,
            folder_cache: LruFolderCache::new(10_000),
            active_syncs: DashMap::new(),
            webhook_url,
        }
    }

    /// Sync a source from a SyncRequest (called by connector-manager)
    pub async fn sync_source_from_request(&self, request: SyncRequest) -> Result<()> {
        let sync_run_id = request.sync_run_id.clone();
        let source_id = request.source_id.clone();
        let sync_mode = request.sync_mode.clone();

        info!(
            "Starting sync for source {} (sync_run_id: {})",
            source_id, sync_run_id
        );

        // Register this sync as active for cancellation tracking
        let active_sync = Arc::new(ActiveSync {
            cancelled: AtomicBool::new(false),
        });
        self.active_syncs
            .insert(sync_run_id.clone(), active_sync.clone());

        // Get the source via SDK
        let source = self
            .sdk_client
            .get_source(&source_id)
            .await
            .context("Failed to fetch source via SDK")?;

        // Determine sync type from mode
        let sync_type = match sync_mode.as_str() {
            "incremental" => SyncType::Incremental,
            _ => SyncType::Full,
        };

        // Run the sync
        let result = match source.source_type {
            SourceType::GoogleDrive => {
                self.sync_drive_source_internal(&source, &sync_run_id, sync_type)
                    .await
            }
            SourceType::Gmail => {
                self.sync_gmail_source_internal(&source, &sync_run_id, sync_type)
                    .await
            }
            _ => Err(anyhow!("Unsupported source type: {:?}", source.source_type)),
        };

        // Auto-register webhook after successful Drive sync
        if result.is_ok() && source.source_type == SourceType::GoogleDrive {
            self.ensure_webhook_registered(&source_id).await;
        }

        // Check if cancelled
        if active_sync.cancelled.load(Ordering::SeqCst) {
            info!("Sync {} was cancelled", sync_run_id);
            let _ = self.sdk_client.cancel(&sync_run_id).await;
            self.active_syncs.remove(&sync_run_id);
            return Ok(());
        }

        // Update sync run based on result via SDK
        match &result {
            Ok((files_scanned, _files_processed, files_updated)) => {
                self.sdk_client
                    .complete(
                        &sync_run_id,
                        *files_scanned as i32,
                        *files_updated as i32,
                        None,
                    )
                    .await?;
            }
            Err(e) => {
                self.sdk_client.fail(&sync_run_id, &e.to_string()).await?;
            }
        }

        self.active_syncs.remove(&sync_run_id);
        result.map(|_| ())
    }

    /// Cancel a running sync
    pub fn cancel_sync(&self, sync_run_id: &str) -> bool {
        if let Some(active_sync) = self.active_syncs.get(sync_run_id) {
            active_sync.cancelled.store(true, Ordering::SeqCst);
            true
        } else {
            false
        }
    }

    /// Check if a sync has been cancelled
    fn is_cancelled(&self, sync_run_id: &str) -> bool {
        self.active_syncs
            .get(sync_run_id)
            .map(|s| s.cancelled.load(Ordering::SeqCst))
            .unwrap_or(false)
    }

    fn get_cutoff_date(&self) -> Result<(String, String)> {
        let max_age_days = std::env::var("GOOGLE_MAX_AGE_DAYS")
            .unwrap_or_else(|_| "730".to_string())
            .parse::<i64>()
            .unwrap_or(730);

        let cutoff_date = OffsetDateTime::now_utc() - time::Duration::days(max_age_days);

        // Format for Drive API (RFC 3339): "2012-06-04T12:00:00-08:00"
        // Use UTC timezone for simplicity
        let drive_format = format!(
            "{:04}-{:02}-{:02}T00:00:00Z",
            cutoff_date.year(),
            cutoff_date.month() as u8,
            cutoff_date.day()
        );

        // Format for Gmail API: "YYYY/MM/DD"
        let gmail_format = format!(
            "{:04}/{:02}/{:02}",
            cutoff_date.year(),
            cutoff_date.month() as u8,
            cutoff_date.day()
        );

        Ok((drive_format, gmail_format))
    }

    async fn sync_drive_for_user(
        &self,
        user_email: &str,
        service_auth: Arc<GoogleAuth>,
        source_id: &str,
        sync_run_id: &str,
        sync_state: &SyncState,
        current_files: Arc<std::sync::Mutex<HashSet<String>>>,
        created_after: Option<&str>,
    ) -> Result<(usize, usize)> {
        info!("Processing Drive files for user: {}", user_email);

        let mut total_processed = 0;
        let mut total_updated = 0;
        let mut page_token: Option<String> = None;
        let mut file_batch = Vec::new();
        const BATCH_SIZE: usize = 200;

        loop {
            debug!(
                "Listing files for user {} with page_token: '{:?}'",
                user_email, page_token
            );

            let response = self
                .drive_client
                .list_files(
                    &service_auth,
                    &user_email,
                    page_token.as_deref(),
                    created_after,
                )
                .await
                .with_context(|| {
                    format!(
                        "Failed to list files for user {} (page_token: {:?})",
                        user_email, page_token
                    )
                })?;

            let page_file_count = response.files.len();
            debug!(
                "Got {} files in this page with page_token: '{:?}' for user {}",
                page_file_count, page_token, user_email
            );

            // Process files in this page
            for file in response.files {
                // Track this file as currently existing
                {
                    let mut current_files_guard = current_files.lock().unwrap();
                    current_files_guard.insert(file.id.clone());
                }

                if self.should_index_file(&file) {
                    let should_process = if let Some(modified_time) = &file.modified_time {
                        match sync_state.get_file_sync_state(source_id, &file.id).await {
                            Ok(Some(last_modified)) => {
                                if last_modified != *modified_time {
                                    debug!(
                                        "File {} has been modified (was: {}, now: {})",
                                        file.name, last_modified, modified_time
                                    );
                                    true
                                } else {
                                    debug!("File {} unchanged, skipping", file.name);
                                    false
                                }
                            }
                            Ok(None) => {
                                debug!("File {} is new, processing", file.name);
                                true
                            }
                            Err(e) => {
                                warn!("Failed to get sync state for file {}: {}", file.name, e);
                                true // Process anyway
                            }
                        }
                    } else {
                        warn!("File {} has no modified_time, processing anyway", file.name);
                        true
                    };

                    if should_process {
                        file_batch.push(UserFile {
                            user_email: Arc::new(user_email.to_string()),
                            file,
                        });

                        // Process batch when it reaches the desired size
                        if file_batch.len() >= BATCH_SIZE {
                            let (processed, updated) = self
                                .process_file_batch(
                                    file_batch.clone(),
                                    source_id,
                                    sync_run_id,
                                    sync_state,
                                    service_auth.clone(),
                                )
                                .await?;

                            total_processed += processed;
                            total_updated += updated;
                            file_batch.clear();
                        }
                    }
                }
            }

            // Update scanned count for this page via SDK
            self.sdk_client
                .increment_scanned(sync_run_id, page_file_count as i32)
                .await?;

            // Check for cancellation
            if self.is_cancelled(sync_run_id) {
                info!(
                    "Sync {} cancelled, stopping Drive sync for user {}",
                    sync_run_id, user_email
                );
                break;
            }

            // Check if there are more pages
            page_token = response.next_page_token;
            if page_token.is_none() {
                break;
            }
        }

        // Process any remaining files in the batch
        if !file_batch.is_empty() {
            let (processed, updated) = self
                .process_file_batch(
                    file_batch,
                    source_id,
                    sync_run_id,
                    sync_state,
                    service_auth.clone(),
                )
                .await?;

            total_processed += processed;
            total_updated += updated;
        }

        info!(
            "Completed processing user {}: {} processed, {} updated",
            user_email, total_processed, total_updated
        );
        Ok((total_processed, total_updated))
    }

    async fn sync_drive_for_user_incremental(
        &self,
        user_email: &str,
        service_auth: Arc<GoogleAuth>,
        source_id: &str,
        sync_run_id: &str,
        sync_state: &SyncState,
        start_page_token: &str,
    ) -> Result<(usize, usize)> {
        info!(
            "Processing incremental Drive sync for user {} from pageToken {}",
            user_email, start_page_token
        );

        let access_token = service_auth.get_access_token(user_email).await?;

        let mut all_changes = Vec::new();
        let mut current_token = start_page_token.to_string();

        loop {
            let response = self
                .drive_client
                .list_changes(&access_token, &current_token)
                .await?;

            all_changes.extend(response.changes);

            if self.is_cancelled(sync_run_id) {
                info!(
                    "Sync {} cancelled during changes listing for user {}",
                    sync_run_id, user_email
                );
                break;
            }

            match response.next_page_token {
                Some(token) => current_token = token,
                None => break,
            }
        }

        info!(
            "Incremental sync found {} changes for user {}",
            all_changes.len(),
            user_email
        );

        self.sdk_client
            .increment_scanned(sync_run_id, all_changes.len() as i32)
            .await?;

        let mut file_batch = Vec::new();
        let mut total_processed = 0;
        let mut total_updated = 0;
        const BATCH_SIZE: usize = 200;

        for change in all_changes {
            let is_removed = change.removed.unwrap_or(false);

            if is_removed {
                if let Some(file_id) = &change.file_id {
                    info!(
                        "File {} was removed (incremental), publishing deletion",
                        file_id
                    );
                    self.publish_deletion_event(sync_run_id, source_id, file_id)
                        .await?;
                    sync_state
                        .delete_file_sync_state(source_id, file_id)
                        .await?;
                }
                continue;
            }

            if let Some(file) = change.file {
                if !self.should_index_file(&file) {
                    continue;
                }

                let should_process = if let Some(modified_time) = &file.modified_time {
                    match sync_state.get_file_sync_state(source_id, &file.id).await {
                        Ok(Some(last_modified)) => last_modified != *modified_time,
                        Ok(None) => true,
                        Err(_) => true,
                    }
                } else {
                    true
                };

                if should_process {
                    file_batch.push(UserFile {
                        user_email: Arc::new(user_email.to_string()),
                        file,
                    });

                    if file_batch.len() >= BATCH_SIZE {
                        let (processed, updated) = self
                            .process_file_batch(
                                file_batch.clone(),
                                source_id,
                                sync_run_id,
                                sync_state,
                                service_auth.clone(),
                            )
                            .await?;
                        total_processed += processed;
                        total_updated += updated;
                        file_batch.clear();
                    }
                }
            }
        }

        if !file_batch.is_empty() {
            let (processed, updated) = self
                .process_file_batch(
                    file_batch,
                    source_id,
                    sync_run_id,
                    sync_state,
                    service_auth.clone(),
                )
                .await?;
            total_processed += processed;
            total_updated += updated;
        }

        info!(
            "Completed incremental Drive sync for user {}: {} processed, {} updated",
            user_email, total_processed, total_updated
        );
        Ok((total_processed, total_updated))
    }

    async fn process_file_batch(
        &self,
        files: Vec<UserFile>,
        source_id: &str,
        sync_run_id: &str,
        sync_state: &SyncState,
        service_auth: Arc<GoogleAuth>,
    ) -> Result<(usize, usize)> {
        info!("Processing batch of {} files", files.len());

        let mut processed = 0;
        let mut updated = 0;

        // Process files concurrently within the batch
        let sync_run_id_owned = sync_run_id.to_string();
        let tasks = files.into_iter().map(|user_file| {
            let service_auth = service_auth.clone();
            let source_id = source_id.to_string();
            let sync_run_id = sync_run_id_owned.clone();
            let sync_state = sync_state.clone();
            let drive_client = self.drive_client.clone();
            let sdk_client = self.sdk_client.clone();

            async move {
                debug!("Processing file: {} ({}) for user: {}", user_file.file.name, user_file.file.id, user_file.user_email);

                // Use rate limiter for file content download
                let result = drive_client
                    .get_file_content(&service_auth, &user_file.user_email, &user_file.file)
                    .await
                    .with_context(|| format!("Getting content for file {} ({})", user_file.file.name, user_file.file.id));

                match result {
                    Ok(content) => {
                        if !content.is_empty() {
                            match sdk_client.store_content(&sync_run_id, &content).await {
                                Ok(content_id) => {
                                    // Resolve the full path for this file
                                    let file_path = match self
                                        .resolve_file_path(
                                            &service_auth,
                                            &user_file.user_email,
                                            &user_file.file,
                                        )
                                        .await
                                    {
                                        Ok(path) => Some(path),
                                        Err(e) => {
                                            warn!("Failed to resolve path for file {}: {}", user_file.file.name, e);
                                            Some(format!("/{}", user_file.file.name))
                                        }
                                    };

                                    let event = user_file.file.to_connector_event(
                                        &sync_run_id,
                                        &source_id,
                                        &content_id,
                                        file_path,
                                    );

                                    match sdk_client.emit_event(&sync_run_id, &source_id, event).await {
                                        Ok(_) => {
                                            if let Some(modified_time) = &user_file.file.modified_time {
                                                if let Err(e) = sync_state
                                                    .set_file_sync_state(&source_id, &user_file.file.id, modified_time)
                                                    .await
                                                {
                                                    error!("Failed to update sync state for file {}: {:?}", user_file.file.name, e);
                                                    return (1, 0); // Processed but not updated
                                                }
                                            }
                                            (1, 1) // Processed and updated
                                        }
                                        Err(e) => {
                                            error!("Failed to queue event for file {}: {:?}", user_file.file.name, e);
                                            (1, 0) // Processed but failed
                                        }
                                    }
                                }
                                Err(e) => {
                                    error!("Failed to store content for file {}: {}", user_file.file.name, e);
                                    (1, 0) // Processed but failed
                                }
                            }
                        } else {
                            debug!("File {} has empty content, skipping", user_file.file.name);
                            (1, 0) // Processed but skipped
                        }
                    }
                    Err(e) => {
                        warn!("Failed to get content for file {} ({}): {:?}", user_file.file.name, user_file.file.id, e);
                        (1, 0) // Processed but failed
                    }
                }
            }
        });

        // Execute all tasks concurrently
        let results = futures::future::join_all(tasks).await;

        // Aggregate results
        for (p, u) in results {
            processed += p;
            updated += u;
        }

        info!(
            "Batch processing complete: {} processed, {} updated",
            processed, updated
        );
        Ok((processed, updated))
    }

    async fn sync_drive_source_internal(
        &self,
        source: &Source,
        sync_run_id: &str,
        sync_type: SyncType,
    ) -> Result<(usize, usize, usize)> {
        let service_creds = self.get_service_credentials(&source.id).await?;
        let service_auth = Arc::new(self.create_auth(&service_creds, source.source_type).await?);

        // Calculate cutoff date for filtering
        let (drive_cutoff_date, _gmail_cutoff_date) = self.get_cutoff_date()?;
        info!("Using Drive cutoff date: {}", drive_cutoff_date);

        // Build user list: single OAuth user or all domain users
        let user_emails: Vec<String> = if service_auth.is_oauth() {
            let email = service_auth
                .oauth_user_email()
                .ok_or_else(|| anyhow::anyhow!("OAuth auth missing user_email"))?
                .to_string();
            info!("OAuth Drive sync for single user: {}", email);
            vec![email]
        } else {
            let domain = self.get_domain_from_credentials(&service_creds)?;
            let user_email = self.get_user_email_from_source(&source.id).await
                .map_err(|e| anyhow::anyhow!("Failed to get user email for source {}: {}. Make sure the source has a valid creator.", source.id, e))?;

            info!("Listing all users in domain: {}", domain);
            info!("Using user email: {}", user_email);
            let admin_access_token = service_auth.get_access_token(&user_email).await
                .map_err(|e| anyhow::anyhow!("Failed to get access token for user {}: {}. Make sure the user is a super-admin and the service account has domain-wide delegation enabled.", user_email, e))?;
            let all_users = self
                .admin_client
                .list_all_users(&admin_access_token, &domain)
                .await?;
            info!("Found {} users in domain {}", all_users.len(), domain);

            let filtered: Vec<String> = all_users
                .into_iter()
                .filter(|user| source.should_index_user(&user.primary_email))
                .map(|user| user.primary_email)
                .collect();
            info!("After filtering: {} users will be indexed", filtered.len());
            filtered
        };

        let is_incremental = matches!(sync_type, SyncType::Incremental);

        let existing_state: GoogleConnectorState =
            if let Ok(Some(raw_state)) = self.sdk_client.get_connector_state(&source.id).await {
                serde_json::from_value(raw_state).unwrap_or_else(|e| {
                    warn!(
                        "Failed to parse connector state for Drive source {}: {}",
                        source.id, e
                    );
                    GoogleConnectorState::default()
                })
            } else {
                GoogleConnectorState::default()
            };

        let old_page_tokens = existing_state.drive_page_tokens.unwrap_or_default();
        let new_page_tokens = Arc::new(std::sync::Mutex::new(HashMap::<String, String>::new()));

        let sync_state = SyncState::new(self.redis_client.clone());
        let synced_files = sync_state.get_all_synced_file_ids(&source.id).await?;
        let current_files = Arc::new(std::sync::Mutex::new(HashSet::new()));

        info!(
            "Starting user processing for {} users (Drive, incremental={})",
            user_emails.len(),
            is_incremental
        );

        let mut total_processed = 0;
        let mut total_updated = 0;
        let mut total_scanned = 0;
        let mut errors = 0;

        for cur_user_email in &user_emails {
            if self.is_cancelled(sync_run_id) {
                info!("Sync {} cancelled, stopping Drive sync early", sync_run_id);
                break;
            }

            match service_auth.get_access_token(cur_user_email).await {
                Ok(access_token) => {
                    info!("Processing user: {}", cur_user_email);

                    // Capture the current page token as watermark for the next sync
                    let current_page_token =
                        match self.drive_client.get_start_page_token(&access_token).await {
                            Ok(token) => Some(token),
                            Err(e) => {
                                warn!(
                                    "Failed to get start page token for user {}: {}",
                                    cur_user_email, e
                                );
                                None
                            }
                        };

                    let stored_page_token = old_page_tokens.get(cur_user_email.as_str());
                    let use_incremental = is_incremental && stored_page_token.is_some();

                    let result = if use_incremental {
                        let start_token = stored_page_token.unwrap();
                        info!(
                            "Using incremental Drive sync for user {} from pageToken {}",
                            cur_user_email, start_token
                        );
                        match self
                            .sync_drive_for_user_incremental(
                                &cur_user_email,
                                service_auth.clone(),
                                &source.id,
                                sync_run_id,
                                &sync_state,
                                start_token,
                            )
                            .await
                        {
                            Ok(result) => Ok(result),
                            Err(e) => {
                                let err_str = format!("{}", e);
                                if err_str.contains("HTTP 404")
                                    || err_str.contains("notFound")
                                    || err_str.contains("pageToken")
                                {
                                    warn!(
                                        "Page token expired for user {}, falling back to full sync",
                                        cur_user_email
                                    );
                                    self.sync_drive_for_user(
                                        &cur_user_email,
                                        service_auth.clone(),
                                        &source.id,
                                        sync_run_id,
                                        &sync_state,
                                        current_files.clone(),
                                        Some(&drive_cutoff_date),
                                    )
                                    .await
                                } else {
                                    Err(e)
                                }
                            }
                        }
                    } else {
                        self.sync_drive_for_user(
                            &cur_user_email,
                            service_auth.clone(),
                            &source.id,
                            sync_run_id,
                            &sync_state,
                            current_files.clone(),
                            Some(&drive_cutoff_date),
                        )
                        .await
                    };

                    match result {
                        Ok((processed, updated)) => {
                            total_processed += processed;
                            total_updated += updated;
                            info!(
                                "User {} Drive sync completed: {} processed, {} updated",
                                cur_user_email, processed, updated
                            );
                        }
                        Err(e) => {
                            error!("Failed to process Drive for user {}: {}", cur_user_email, e);
                            errors += 1;
                        }
                    }

                    if let Some(page_token) = current_page_token {
                        new_page_tokens
                            .lock()
                            .unwrap()
                            .insert(cur_user_email.clone(), page_token);
                    }
                }
                Err(e) => {
                    warn!("Failed to get access token for user {}: {}. This user may not have Drive access.", cur_user_email, e);
                    errors += 1;
                }
            }
        }

        info!(
            "User processing complete. Total: {} processed, {} updated, {} errors",
            total_processed, total_updated, errors
        );

        // Only run deletion scan during full sync; incremental handles deletions via DriveChange.removed
        if !is_incremental {
            let current_files_set = {
                let current_files_guard = current_files.lock().unwrap();
                current_files_guard.clone()
            };
            total_scanned = current_files_set.len();

            for deleted_file_id in synced_files.difference(&current_files_set) {
                info!(
                    "File {} was deleted, publishing deletion event",
                    deleted_file_id
                );
                self.publish_deletion_event(sync_run_id, &source.id, deleted_file_id)
                    .await?;
                sync_state
                    .delete_file_sync_state(&source.id, deleted_file_id)
                    .await?;
            }
        }

        // Save updated connector state, preserving webhook and gmail fields
        let final_page_tokens = {
            let tokens = new_page_tokens.lock().unwrap();
            tokens.clone()
        };
        let updated_state = GoogleConnectorState {
            webhook_channel_id: existing_state.webhook_channel_id,
            webhook_resource_id: existing_state.webhook_resource_id,
            webhook_expires_at: existing_state.webhook_expires_at,
            gmail_history_ids: existing_state.gmail_history_ids,
            drive_page_tokens: if final_page_tokens.is_empty() {
                None
            } else {
                Some(final_page_tokens)
            },
        };

        if let Err(e) = self
            .sdk_client
            .save_connector_state(&source.id, serde_json::to_value(&updated_state)?)
            .await
        {
            error!(
                "Failed to save Drive page tokens in connector state for source {}: {}",
                source.id, e
            );
        }

        info!(
            "Sync completed for source {}: {} processed, {} updated",
            source.id, total_processed, total_updated
        );

        // Clear folder cache to free memory after sync
        self.folder_cache.clear();

        info!("Completed sync for source: {}", source.id);
        Ok((total_scanned, total_processed, total_updated))
    }

    async fn sync_gmail_source_internal(
        &self,
        source: &Source,
        sync_run_id: &str,
        sync_type: SyncType,
    ) -> Result<(usize, usize, usize)> {
        let service_creds = self.get_service_credentials(&source.id).await?;
        let service_auth = Arc::new(self.create_auth(&service_creds, source.source_type).await?);

        let (_drive_cutoff_date, gmail_cutoff_date) = self.get_cutoff_date()?;
        info!("Using Gmail cutoff date: {}", gmail_cutoff_date);

        // Build user list: single OAuth user or all domain users
        let user_emails: Vec<String> = if service_auth.is_oauth() {
            let email = service_auth
                .oauth_user_email()
                .ok_or_else(|| anyhow::anyhow!("OAuth auth missing user_email"))?
                .to_string();
            info!("OAuth Gmail sync for single user: {}", email);
            vec![email]
        } else {
            let domain = self.get_domain_from_credentials(&service_creds)?;
            let user_email = self.get_user_email_from_source(&source.id).await
                .map_err(|e| anyhow::anyhow!("Failed to get user email for source {}: {}. Make sure the source has a valid creator.", source.id, e))?;

            info!("Listing all users in domain: {}", domain);
            info!("Using user email: {}", user_email);
            let admin_access_token = service_auth.get_access_token(&user_email).await
                .map_err(|e| anyhow::anyhow!("Failed to get access token for user {}: {}. Make sure the user is a super-admin and the service account has domain-wide delegation enabled.", user_email, e))?;
            let all_users = self
                .admin_client
                .list_all_users(&admin_access_token, &domain)
                .await?;
            info!("Found {} users in domain {}", all_users.len(), domain);

            let filtered: Vec<String> = all_users
                .into_iter()
                .filter(|user| source.should_index_user(&user.primary_email))
                .map(|user| user.primary_email)
                .collect();
            info!("After filtering: {} users will be indexed", filtered.len());
            filtered
        };

        let is_incremental = matches!(sync_type, SyncType::Incremental);

        let existing_state: GoogleConnectorState =
            if let Ok(Some(raw_state)) = self.sdk_client.get_connector_state(&source.id).await {
                serde_json::from_value(raw_state).unwrap_or_else(|e| {
                    warn!(
                        "Failed to parse connector state for Gmail source {}: {}",
                        source.id, e
                    );
                    GoogleConnectorState::default()
                })
            } else {
                GoogleConnectorState::default()
            };

        let old_history_ids = existing_state.gmail_history_ids.unwrap_or_default();
        let mut new_history_ids: HashMap<String, String> = HashMap::new();

        let processed_threads = Arc::new(std::sync::Mutex::new(HashSet::<String>::new()));

        info!(
            "Starting sequential user processing for {} users (Gmail, incremental={})",
            user_emails.len(),
            is_incremental
        );

        let mut total_processed = 0;
        let mut total_updated = 0;

        for cur_user_email in &user_emails {
            if self.is_cancelled(sync_run_id) {
                info!("Sync {} cancelled, stopping Gmail sync early", sync_run_id);
                break;
            }

            match service_auth.get_access_token(cur_user_email).await {
                Ok(_token) => {
                    info!("Processing user: {}", cur_user_email);

                    let current_history_id = match self
                        .gmail_client
                        .get_profile(&service_auth, &cur_user_email)
                        .await
                    {
                        Ok(profile) => Some(profile.history_id),
                        Err(e) => {
                            warn!(
                                "Failed to get Gmail profile for user {}: {}",
                                cur_user_email, e
                            );
                            None
                        }
                    };

                    let stored_history_id = old_history_ids.get(cur_user_email.as_str());
                    let use_incremental = is_incremental && stored_history_id.is_some();

                    let result = if use_incremental {
                        let start_id = stored_history_id.unwrap();
                        info!(
                            "Using incremental Gmail sync for user {} from historyId {}",
                            cur_user_email, start_id
                        );
                        match self
                            .sync_gmail_for_user_incremental(
                                &cur_user_email,
                                service_auth.clone(),
                                &source.id,
                                sync_run_id,
                                start_id,
                                processed_threads.clone(),
                            )
                            .await
                        {
                            Ok(result) => Ok(result),
                            Err(e) => {
                                let err_str = format!("{}", e);
                                if err_str.contains("HTTP 404") {
                                    warn!(
                                        "History expired for user {}, falling back to full sync",
                                        cur_user_email
                                    );
                                    self.sync_gmail_for_user(
                                        &cur_user_email,
                                        service_auth.clone(),
                                        &source.id,
                                        sync_run_id,
                                        processed_threads.clone(),
                                        Some(&gmail_cutoff_date),
                                    )
                                    .await
                                } else {
                                    Err(e)
                                }
                            }
                        }
                    } else {
                        self.sync_gmail_for_user(
                            &cur_user_email,
                            service_auth.clone(),
                            &source.id,
                            sync_run_id,
                            processed_threads.clone(),
                            Some(&gmail_cutoff_date),
                        )
                        .await
                    };

                    match result {
                        Ok((processed, updated)) => {
                            total_processed += processed;
                            total_updated += updated;
                            info!(
                                "User {} Gmail sync completed: {} processed, {} updated",
                                cur_user_email, processed, updated
                            );
                        }
                        Err(e) => {
                            error!("Failed to process Gmail for user {}: {}", cur_user_email, e);
                        }
                    }

                    if let Some(history_id) = current_history_id {
                        new_history_ids.insert(cur_user_email.clone(), history_id);
                    }
                }
                Err(e) => {
                    warn!("Failed to get access token for user {}: {}. This user may not have Gmail access.", cur_user_email, e);
                }
            }
        }

        let updated_state = GoogleConnectorState {
            webhook_channel_id: existing_state.webhook_channel_id,
            webhook_resource_id: existing_state.webhook_resource_id,
            webhook_expires_at: existing_state.webhook_expires_at,
            gmail_history_ids: if new_history_ids.is_empty() {
                None
            } else {
                Some(new_history_ids)
            },
            drive_page_tokens: existing_state.drive_page_tokens,
        };

        if let Err(e) = self
            .sdk_client
            .save_connector_state(&source.id, serde_json::to_value(&updated_state)?)
            .await
        {
            error!(
                "Failed to save Gmail history IDs in connector state for source {}: {}",
                source.id, e
            );
        }

        info!(
            "Gmail sync completed for source {}: {} total processed, {} total updated",
            source.id, total_processed, total_updated
        );

        info!("Completed Gmail sync for source: {}", source.id);
        Ok((total_processed, total_processed, total_updated))
    }

    fn should_index_file(&self, file: &crate::models::GoogleDriveFile) -> bool {
        matches!(
            file.mime_type.as_str(),
            "application/vnd.google-apps.document"
                | "application/vnd.google-apps.spreadsheet"
                | "application/vnd.google-apps.presentation"
                | "text/plain"
                | "text/html"
                | "text/csv"
                | "application/pdf"
                | "application/vnd.openxmlformats-officedocument.wordprocessingml.document"
                | "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet"
                | "application/vnd.openxmlformats-officedocument.presentationml.presentation"
                | "application/msword"
                | "application/vnd.ms-excel"
                | "application/vnd.ms-powerpoint"
        )
    }

    async fn publish_deletion_event(
        &self,
        sync_run_id: &str,
        source_id: &str,
        document_id: &str,
    ) -> Result<()> {
        let event = ConnectorEvent::DocumentDeleted {
            sync_run_id: sync_run_id.to_string(),
            source_id: source_id.to_string(),
            document_id: document_id.to_string(),
        };

        self.sdk_client
            .emit_event(sync_run_id, source_id, event)
            .await
    }

    async fn get_service_credentials(&self, source_id: &str) -> Result<ServiceCredentials> {
        let creds = self
            .sdk_client
            .get_credentials(source_id)
            .await
            .context("Failed to fetch credentials via SDK")?;

        // Verify it's a Google credentials record
        if creds.provider != ServiceProvider::Google {
            return Err(anyhow::anyhow!(
                "Expected Google credentials for source {}, found {:?}",
                source_id,
                creds.provider
            ));
        }

        Ok(creds)
    }

    fn create_service_auth(
        &self,
        creds: &ServiceCredentials,
        source_type: SourceType,
    ) -> Result<ServiceAccountAuth> {
        let service_account_json = creds
            .credentials
            .get("service_account_key")
            .and_then(|v| v.as_str())
            .ok_or_else(|| anyhow::anyhow!("Missing service_account_key in credentials"))?;

        // Check if custom scopes are provided in config, otherwise use defaults based on source type
        let scopes = creds
            .config
            .get("scopes")
            .and_then(|v| v.as_array())
            .map(|arr| {
                arr.iter()
                    .filter_map(|v| v.as_str().map(String::from))
                    .collect::<Vec<_>>()
            })
            .unwrap_or_else(|| crate::auth::get_scopes_for_source_type(source_type));

        ServiceAccountAuth::new(service_account_json, scopes)
    }

    /// Create GoogleAuth from credentials, branching on auth_type (JWT vs OAuth)
    async fn create_auth(
        &self,
        creds: &ServiceCredentials,
        source_type: SourceType,
    ) -> Result<GoogleAuth> {
        match creds.auth_type {
            AuthType::OAuth => {
                let access_token = creds
                    .credentials
                    .get("access_token")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string();

                let refresh_token = creds
                    .credentials
                    .get("refresh_token")
                    .and_then(|v| v.as_str())
                    .ok_or_else(|| anyhow::anyhow!("Missing refresh_token in OAuth credentials"))?
                    .to_string();

                let expires_at = creds
                    .credentials
                    .get("expires_at")
                    .and_then(|v| v.as_i64())
                    .unwrap_or(0);

                let user_email = creds
                    .credentials
                    .get("user_email")
                    .and_then(|v| v.as_str())
                    .or(creds.principal_email.as_deref())
                    .ok_or_else(|| anyhow::anyhow!("Missing user_email in OAuth credentials"))?
                    .to_string();

                // Fetch connector config for OAuth client_id/secret
                let connector_config = self
                    .sdk_client
                    .get_connector_config("google")
                    .await
                    .context("Failed to fetch Google connector config for OAuth")?;

                let client_id = connector_config
                    .get("oauth_client_id")
                    .and_then(|v| v.as_str())
                    .ok_or_else(|| {
                        anyhow::anyhow!("Missing oauth_client_id in Google connector config")
                    })?
                    .to_string();

                let client_secret = connector_config
                    .get("oauth_client_secret")
                    .and_then(|v| v.as_str())
                    .ok_or_else(|| {
                        anyhow::anyhow!("Missing oauth_client_secret in Google connector config")
                    })?
                    .to_string();

                let oauth_auth = OAuthAuth::new(
                    access_token,
                    refresh_token,
                    expires_at,
                    user_email,
                    client_id,
                    client_secret,
                )?;

                Ok(GoogleAuth::OAuth(oauth_auth))
            }
            _ => {
                // Default: JWT / service account
                let sa = self.create_service_auth(creds, source_type)?;
                Ok(GoogleAuth::ServiceAccount(sa))
            }
        }
    }

    fn get_domain_from_credentials(&self, creds: &ServiceCredentials) -> Result<String> {
        creds
            .config
            .get("domain")
            .and_then(|v| v.as_str())
            .map(String::from)
            .ok_or_else(|| anyhow::anyhow!("Missing domain in service credentials config"))
    }

    async fn get_user_email_from_source(&self, source_id: &str) -> Result<String> {
        self.sdk_client
            .get_user_email_for_source(source_id)
            .await
            .context("Failed to get user email via SDK")
    }

    pub async fn handle_webhook_notification(
        &self,
        notification: WebhookNotification,
    ) -> Result<()> {
        info!(
            "Handling webhook notification for channel {}, state: {}",
            notification.channel_id, notification.resource_state
        );

        let source_id = match &notification.source_id {
            Some(id) => id.clone(),
            None => {
                warn!(
                    "Received webhook notification without source_id token for channel {}",
                    notification.channel_id
                );
                return Ok(());
            }
        };

        match notification.resource_state.as_str() {
            "sync" => {
                debug!(
                    "Received sync message for channel: {}",
                    notification.channel_id
                );
            }
            "add" | "update" | "remove" | "trash" | "untrash" | "change" => {
                info!(
                    "Notifying connector-manager of webhook event for source {} (state: {})",
                    source_id, notification.resource_state
                );

                match self
                    .sdk_client
                    .notify_webhook(&source_id, &notification.resource_state)
                    .await
                {
                    Ok(sync_run_id) => {
                        info!(
                            "Connector-manager created sync run {} for webhook event",
                            sync_run_id
                        );
                    }
                    Err(e) => {
                        error!(
                            "Failed to notify connector-manager of webhook event for source {}: {}",
                            source_id, e
                        );
                    }
                }
            }
            _ => {
                debug!(
                    "Ignoring webhook notification with state: {}",
                    notification.resource_state
                );
            }
        }

        Ok(())
    }

    /// Ensure a webhook is registered for a source.
    /// No-op if webhook_url is None. Logs but never propagates errors.
    pub async fn ensure_webhook_registered(&self, source_id: &str) {
        let Some(ref webhook_url) = self.webhook_url else {
            return;
        };

        info!("Ensuring webhook registered for source {}", source_id);
        if let Err(e) = self
            .register_webhook_for_source(source_id, webhook_url.clone())
            .await
        {
            error!("Failed to register webhook for source {}: {}", source_id, e);
        }
    }

    pub async fn register_webhook_for_source(
        &self,
        source_id: &str,
        webhook_url: String,
    ) -> Result<WebhookChannelResponse> {
        // Capture old channel info before registering the new one
        let old_channel =
            if let Ok(Some(raw_state)) = self.sdk_client.get_connector_state(source_id).await {
                let state: GoogleConnectorState =
                    serde_json::from_value(raw_state).unwrap_or_else(|e| {
                        warn!(
                            "Failed to parse connector state for source {}: {}",
                            source_id, e
                        );
                        GoogleConnectorState::default()
                    });
                match (&state.webhook_channel_id, &state.webhook_resource_id) {
                    (Some(ch), Some(res)) => Some((ch.clone(), res.clone())),
                    _ => None,
                }
            } else {
                None
            };

        let service_creds = self.get_service_credentials(source_id).await?;
        let service_auth = self.create_service_auth(&service_creds, SourceType::GoogleDrive)?;
        let user_email = self.get_user_email_from_source(source_id).await
            .map_err(|e| anyhow::anyhow!("Failed to get user email for source {}: {}. Make sure the source has a valid creator.", source_id, e))?;
        let access_token = service_auth.get_access_token(&user_email).await?;

        let start_page_token = self
            .drive_client
            .get_start_page_token(&access_token)
            .await?;

        let webhook_channel = WebhookChannel::new(webhook_url.clone(), source_id);

        let webhook_response = self
            .drive_client
            .register_changes_webhook(&access_token, &webhook_channel, &start_page_token)
            .await?;

        let expires_at = webhook_response
            .expiration
            .as_ref()
            .and_then(|exp| exp.parse::<i64>().ok());

        // Store new channel info in connector_state, preserving existing gmail_history_ids
        let existing_state: GoogleConnectorState =
            if let Ok(Some(raw)) = self.sdk_client.get_connector_state(source_id).await {
                serde_json::from_value(raw).unwrap_or_default()
            } else {
                GoogleConnectorState::default()
            };
        let webhook_state = GoogleConnectorState {
            webhook_channel_id: Some(webhook_response.id.clone()),
            webhook_resource_id: Some(webhook_response.resource_id.clone()),
            webhook_expires_at: expires_at,
            gmail_history_ids: existing_state.gmail_history_ids,
            drive_page_tokens: existing_state.drive_page_tokens,
        };
        self.sdk_client
            .save_connector_state(source_id, serde_json::to_value(&webhook_state)?)
            .await?;

        info!(
            "Successfully registered webhook for source {}: channel_id={}, resource_id={}",
            source_id, webhook_response.id, webhook_response.resource_id
        );

        // Stop old channel after the new one is active to avoid gaps in coverage
        if let Some((old_channel_id, old_resource_id)) = old_channel {
            info!(
                "Stopping old webhook channel {} for source {}",
                old_channel_id, source_id
            );
            if let Err(e) = self
                .stop_webhook_for_source(source_id, &old_channel_id, &old_resource_id)
                .await
            {
                warn!("Failed to stop old webhook channel: {}", e);
            }
        }

        Ok(webhook_response)
    }

    pub async fn stop_webhook_for_source(
        &self,
        source_id: &str,
        channel_id: &str,
        resource_id: &str,
    ) -> Result<()> {
        let service_creds = self.get_service_credentials(source_id).await?;
        let service_auth = self.create_service_auth(&service_creds, SourceType::GoogleDrive)?;
        let user_email = self.get_user_email_from_source(source_id).await
            .map_err(|e| anyhow::anyhow!("Failed to get user email for source {}: {}. Make sure the source has a valid creator.", source_id, e))?;
        let access_token = service_auth.get_access_token(&user_email).await?;

        self.drive_client
            .stop_webhook_channel(&access_token, channel_id, resource_id)
            .await?;

        info!(
            "Successfully stopped webhook for source {}: channel_id={}",
            source_id, channel_id
        );
        Ok(())
    }

    async fn resolve_file_path(
        &self,
        auth: &GoogleAuth,
        user_email: &str,
        file: &crate::models::GoogleDriveFile,
    ) -> Result<String> {
        if let Some(parents) = &file.parents {
            if let Some(parent_id) = parents.first() {
                return self
                    .build_full_path(auth, user_email, parent_id, &file.name)
                    .await;
            }
        }

        // If no parents, file is in root
        Ok(format!("/{}", file.name))
    }

    async fn build_full_path(
        &self,
        auth: &GoogleAuth,
        user_email: &str,
        folder_id: &str,
        file_name: &str,
    ) -> Result<String> {
        debug!(
            "Building full path for file: {}, starting from folder: {}",
            file_name, folder_id
        );
        let mut path_components = vec![file_name.to_string()];
        let mut current_folder_id = folder_id.to_string();

        // Build path by traversing up the folder hierarchy
        let mut depth = 0;
        loop {
            depth += 1;
            debug!(
                "Path building depth: {}, current folder: {}",
                depth, current_folder_id
            );

            // TODO: Remove this
            if depth > 50 {
                warn!(
                    "Path building depth exceeded 50 levels for file: {}, folder: {}",
                    file_name, folder_id
                );
                break;
            }

            let cached_folder = self.folder_cache.get(&current_folder_id);

            let parent_folder_id: Option<String> = match cached_folder {
                Some(folder) => {
                    debug!("Found folder {} [id: {}] in cache", folder.name, folder.id);
                    path_components.push(folder.name.clone());
                    folder
                        .parents
                        .as_ref()
                        .map(|p| p.first())
                        .flatten()
                        .cloned()
                }
                None => {
                    debug!(
                        "Folder {} not found in cache, fetching metadata.",
                        current_folder_id
                    );
                    let folder_metadata = self
                        .drive_client
                        .get_folder_metadata(&auth, &user_email, &folder_id)
                        .await;

                    match folder_metadata {
                        Ok(folder_metadata) => {
                            let name = folder_metadata.name.clone();
                            debug!(
                                "Successfully fetched folder metadata: {} for folder: {}",
                                name, current_folder_id
                            );

                            let parent_folder_id = folder_metadata
                                .parents
                                .as_ref()
                                .map(|p| p.first())
                                .flatten()
                                .cloned();
                            debug!(
                                "Folder {} has parent: {:?}",
                                current_folder_id, parent_folder_id
                            );

                            // Cache the folder
                            self.folder_cache
                                .insert(current_folder_id.clone(), folder_metadata.into());

                            path_components.push(name);
                            parent_folder_id
                        }
                        Err(e) => {
                            warn!(
                                "Failed to get folder metadata for {}: {}",
                                current_folder_id, e
                            );
                            None
                        }
                    }
                }
            };

            if let Some(parent_id) = parent_folder_id {
                debug!("Folder {} has parent: {:?}", current_folder_id, parent_id);
                if parent_id == current_folder_id {
                    debug!("Reached root folder {}", current_folder_id);
                    break;
                }
                current_folder_id = parent_id;
            } else {
                debug!("Reached root folder {}", current_folder_id);
                break;
            }
        }

        // Reverse to get correct order (root to file)
        path_components.reverse();
        Ok(format!("/{}", path_components.join("/")))
    }

    async fn sync_gmail_for_user(
        &self,
        user_email: &str,
        service_auth: Arc<GoogleAuth>,
        source_id: &str,
        sync_run_id: &str,
        processed_threads: Arc<std::sync::Mutex<HashSet<String>>>,
        created_after: Option<&str>,
    ) -> Result<(usize, usize)> {
        info!("Processing Gmail for user: {}", user_email);

        let mut page_token: Option<String> = None;
        const BATCH_SIZE: usize = 500;

        // Track threads found for this user
        let mut user_threads: Vec<String> = Vec::new();

        // Step 1: List all threads for the user
        loop {
            debug!(
                "Listing Gmail threads for user {} with page_token: {:?}",
                user_email, page_token
            );

            let response = self
                .gmail_client
                .list_threads(
                    &service_auth,
                    &user_email,
                    None,
                    Some(BATCH_SIZE as u32),
                    page_token.as_deref(),
                    created_after,
                )
                .await
                .with_context(|| {
                    format!(
                        "Failed to list Gmail threads for user {} (page_token: {:?})",
                        user_email, page_token
                    )
                })?;

            // Collect thread IDs
            if let Some(threads) = response.threads {
                let page_thread_count = threads.len();
                debug!(
                    "Got {} threads in this page for user {}",
                    page_thread_count, user_email
                );

                for thread_info in threads {
                    user_threads.push(thread_info.id);
                }

                // Update scanned count for this page via SDK
                self.sdk_client
                    .increment_scanned(sync_run_id, page_thread_count as i32)
                    .await?;
            }

            // Check for cancellation
            if self.is_cancelled(sync_run_id) {
                info!(
                    "Sync {} cancelled, stopping Gmail thread listing for user {}",
                    sync_run_id, user_email
                );
                break;
            }

            // Check if there are more pages
            page_token = response.next_page_token;
            if page_token.is_none() {
                break;
            }
        }

        info!(
            "Found {} Gmail threads for user {}",
            user_threads.len(),
            user_email
        );

        self.process_gmail_threads(
            user_threads,
            user_email,
            service_auth,
            source_id,
            sync_run_id,
            processed_threads,
        )
        .await
    }

    async fn sync_gmail_for_user_incremental(
        &self,
        user_email: &str,
        service_auth: Arc<GoogleAuth>,
        source_id: &str,
        sync_run_id: &str,
        start_history_id: &str,
        processed_threads: Arc<std::sync::Mutex<HashSet<String>>>,
    ) -> Result<(usize, usize)> {
        info!(
            "Processing incremental Gmail sync for user {} from historyId {}",
            user_email, start_history_id
        );

        let mut changed_thread_ids = HashSet::new();
        let mut page_token: Option<String> = None;

        loop {
            let response = self
                .gmail_client
                .list_history(
                    &service_auth,
                    user_email,
                    start_history_id,
                    Some(500),
                    page_token.as_deref(),
                )
                .await?;

            if let Some(history_records) = response.history {
                for record in history_records {
                    if let Some(messages) = record.messages {
                        for msg in messages {
                            changed_thread_ids.insert(msg.thread_id);
                        }
                    }
                    if let Some(added) = record.messages_added {
                        for item in added {
                            changed_thread_ids.insert(item.message.thread_id);
                        }
                    }
                    if let Some(deleted) = record.messages_deleted {
                        for item in deleted {
                            changed_thread_ids.insert(item.message.thread_id);
                        }
                    }
                    if let Some(label_added) = record.labels_added {
                        for item in label_added {
                            changed_thread_ids.insert(item.message.thread_id);
                        }
                    }
                    if let Some(label_removed) = record.labels_removed {
                        for item in label_removed {
                            changed_thread_ids.insert(item.message.thread_id);
                        }
                    }
                }
            }

            if self.is_cancelled(sync_run_id) {
                info!(
                    "Sync {} cancelled during history listing for user {}",
                    sync_run_id, user_email
                );
                break;
            }

            page_token = response.next_page_token;
            if page_token.is_none() {
                break;
            }
        }

        let thread_ids: Vec<String> = changed_thread_ids.into_iter().collect();
        info!(
            "Incremental sync found {} changed threads for user {}",
            thread_ids.len(),
            user_email
        );

        self.sdk_client
            .increment_scanned(sync_run_id, thread_ids.len() as i32)
            .await?;

        self.process_gmail_threads(
            thread_ids,
            user_email,
            service_auth,
            source_id,
            sync_run_id,
            processed_threads,
        )
        .await
    }

    async fn process_gmail_threads(
        &self,
        thread_ids: Vec<String>,
        user_email: &str,
        service_auth: Arc<GoogleAuth>,
        source_id: &str,
        sync_run_id: &str,
        processed_threads: Arc<std::sync::Mutex<HashSet<String>>>,
    ) -> Result<(usize, usize)> {
        let mut total_processed = 0;
        let mut total_updated = 0;
        let sync_state = SyncState::new(self.redis_client.clone());
        const THREAD_BATCH_SIZE: usize = 50;

        for chunk in thread_ids.chunks(THREAD_BATCH_SIZE) {
            if self.is_cancelled(sync_run_id) {
                info!(
                    "Sync {} cancelled, stopping Gmail thread processing for user {}",
                    sync_run_id, user_email
                );
                break;
            }

            let mut unprocessed_threads = Vec::new();
            for thread_id in chunk {
                let already_processed = {
                    let processed_guard = processed_threads.lock().unwrap();
                    processed_guard.contains(thread_id)
                };

                if already_processed {
                    debug!(
                        "Thread {} already processed by another user, skipping",
                        thread_id
                    );
                    continue;
                }

                unprocessed_threads.push(thread_id.clone());
            }

            if unprocessed_threads.is_empty() {
                continue;
            }

            {
                let mut processed_guard = processed_threads.lock().unwrap();
                for thread_id in &unprocessed_threads {
                    processed_guard.insert(thread_id.clone());
                }
            }

            debug!("Processing batch of {} threads", unprocessed_threads.len());

            let batch_results = match self
                .gmail_client
                .batch_get_threads(
                    &service_auth,
                    user_email,
                    &unprocessed_threads,
                    MessageFormat::Full,
                )
                .await
                .with_context(|| {
                    format!("Failed to get Gmail threads batch for user {}", user_email)
                }) {
                Ok(results) => results,
                Err(e) => {
                    warn!("Failed to fetch thread batch: {}", e);
                    continue;
                }
            };

            for (i, thread_result) in batch_results.into_iter().enumerate() {
                let thread_id = &unprocessed_threads[i];
                total_processed += 1;

                let thread_response = match thread_result {
                    Ok(response) => response,
                    Err(e) => {
                        warn!("Failed to fetch thread {}: {}", thread_id, e);
                        continue;
                    }
                };

                let mut gmail_thread = GmailThread::new(thread_id.clone());
                for message in thread_response.messages {
                    gmail_thread.add_message(message);
                }

                if !gmail_thread.latest_date.is_empty() {
                    match sync_state.get_thread_sync_state(source_id, thread_id).await {
                        Ok(Some(last_synced_date)) => {
                            match (
                                gmail_thread.latest_date.parse::<i64>(),
                                last_synced_date.parse::<i64>(),
                            ) {
                                (Ok(latest_ts), Ok(synced_ts)) => {
                                    if latest_ts <= synced_ts {
                                        debug!(
                                            "Thread {} already synced (latest: {}, last synced: {}), skipping",
                                            thread_id, gmail_thread.latest_date, last_synced_date
                                        );
                                        continue;
                                    } else {
                                        debug!(
                                            "Thread {} has new messages (latest: {}, last synced: {}), processing",
                                            thread_id, gmail_thread.latest_date, last_synced_date
                                        );
                                    }
                                }
                                _ => {
                                    debug!(
                                        "Failed to parse timestamps for thread {} (latest: {}, last synced: {}), processing to be safe",
                                        thread_id, gmail_thread.latest_date, last_synced_date
                                    );
                                }
                            }
                        }
                        Ok(None) => {
                            debug!("Thread {} not previously synced, processing", thread_id);
                        }
                        Err(e) => {
                            warn!("Failed to get sync state for thread {}: {}", thread_id, e);
                        }
                    }
                }

                if gmail_thread.total_messages > 0 {
                    match gmail_thread.aggregate_content(&self.gmail_client) {
                        Ok(content) => {
                            if !content.trim().is_empty() {
                                match self.sdk_client.store_content(sync_run_id, &content).await {
                                    Ok(content_id) => {
                                        match gmail_thread.to_connector_event(
                                            sync_run_id,
                                            source_id,
                                            &content_id,
                                            &self.gmail_client,
                                        ) {
                                            Ok(event) => {
                                                match self
                                                    .sdk_client
                                                    .emit_event(sync_run_id, source_id, event)
                                                    .await
                                                {
                                                    Ok(_) => {
                                                        total_updated += 1;
                                                        info!(
                                                            "Successfully queued Gmail thread {}",
                                                            thread_id
                                                        );

                                                        if let Err(e) = sync_state
                                                            .set_thread_sync_state(
                                                                source_id,
                                                                thread_id,
                                                                &gmail_thread.latest_date,
                                                            )
                                                            .await
                                                        {
                                                            error!("Failed to update sync state for Gmail thread {}: {}", thread_id, e);
                                                        }
                                                    }
                                                    Err(e) => {
                                                        error!("Failed to queue event for Gmail thread {}: {}", thread_id, e);
                                                    }
                                                }
                                            }
                                            Err(e) => {
                                                error!("Failed to create connector event for Gmail thread {}: {}", thread_id, e);
                                            }
                                        }
                                    }
                                    Err(e) => {
                                        error!(
                                            "Failed to store content for Gmail thread {}: {}",
                                            thread_id, e
                                        );
                                    }
                                }
                            } else {
                                debug!("Gmail thread {} has empty content, skipping", thread_id);
                            }
                        }
                        Err(e) => {
                            error!(
                                "Failed to aggregate content for Gmail thread {}: {}",
                                thread_id, e
                            );
                        }
                    }
                } else {
                    debug!("Gmail thread {} has no messages, skipping", thread_id);
                }

                drop(gmail_thread);
            }

            drop(unprocessed_threads);
        }

        info!(
            "Completed Gmail processing for user {}: {} threads processed, {} updated",
            user_email, total_processed, total_updated
        );

        Ok((total_processed, total_updated))
    }
}

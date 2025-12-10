use anyhow::{anyhow, Context, Result};
use futures::stream::{self, StreamExt};
use redis::{AsyncCommands, Client as RedisClient};
use sqlx::types::time::OffsetDateTime;
use sqlx::PgPool;
use std::collections::HashSet;
use std::sync::Arc;
use time;
use tracing::{debug, error, info, warn};

use crate::admin::AdminClient;
use crate::auth::ServiceAccountAuth;
use crate::cache::LruFolderCache;
use crate::drive::DriveClient;
use crate::gmail::{GmailClient, MessageFormat};
use crate::models::{
    GmailThread, UserFile, WebhookChannel, WebhookChannelResponse, WebhookNotification,
};
use shared::db::repositories::{ServiceCredentialsRepo, SyncRunRepository};
use shared::models::{
    ConnectorEvent, ServiceCredentials, ServiceProvider, Source, SourceType, SyncRun, SyncStatus,
    SyncType, WebhookChannel as DatabaseWebhookChannel,
};
use shared::queue::EventQueue;
use shared::utils::generate_ulid;
use shared::{AIClient, ObjectStorage, RateLimiter, Repository, SourceRepository};

pub struct SyncManager {
    pool: PgPool,
    redis_client: RedisClient,
    drive_client: DriveClient,
    gmail_client: GmailClient,
    admin_client: Arc<AdminClient>,
    event_queue: EventQueue,
    content_storage: Arc<dyn ObjectStorage>,
    service_credentials_repo: ServiceCredentialsRepo,
    source_repo: SourceRepository,
    sync_run_repo: SyncRunRepository,
    folder_cache: LruFolderCache,
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

    pub async fn delete_thread_sync_state(&self, source_id: &str, thread_id: &str) -> Result<()> {
        let mut conn = self.redis_client.get_multiplexed_async_connection().await?;
        let key = if cfg!(test) {
            self.get_test_thread_sync_key(source_id, thread_id)
        } else {
            self.get_thread_sync_key(source_id, thread_id)
        };

        let _: () = conn.del(&key).await?;
        Ok(())
    }

    pub async fn get_all_synced_thread_ids(&self, source_id: &str) -> Result<HashSet<String>> {
        let mut conn = self.redis_client.get_multiplexed_async_connection().await?;
        let pattern = if cfg!(test) {
            format!("google:gmail:sync:test:{}:*", source_id)
        } else {
            format!("google:gmail:sync:{}:*", source_id)
        };

        let keys: Vec<String> = conn.keys(&pattern).await?;
        let prefix = if cfg!(test) {
            format!("google:gmail:sync:test:{}:", source_id)
        } else {
            format!("google:gmail:sync:{}:", source_id)
        };
        let thread_ids: HashSet<String> = keys
            .into_iter()
            .filter_map(|key| key.strip_prefix(&prefix).map(|s| s.to_string()))
            .collect();

        Ok(thread_ids)
    }
}

impl SyncManager {
    pub async fn new(
        pool: PgPool,
        redis_client: RedisClient,
        ai_service_url: String,
        admin_client: Arc<AdminClient>,
    ) -> Result<Self> {
        let event_queue = EventQueue::new(pool.clone());
        let service_credentials_repo = ServiceCredentialsRepo::new(pool.clone())?;

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

        let content_storage = shared::StorageFactory::from_env(pool.clone()).await?;
        let source_repo = SourceRepository::new(&pool);
        let sync_run_repo = SyncRunRepository::new(&pool);

        Ok(Self {
            pool,
            redis_client,
            drive_client,
            gmail_client,
            admin_client,
            event_queue,
            content_storage,
            service_credentials_repo,
            source_repo,
            sync_run_repo,
            folder_cache: LruFolderCache::new(10_000), // Cache up to 10,000 folder metadata entries
        })
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

    fn get_storage_prefix(sync_run: &SyncRun) -> String {
        format!(
            "{}/{}",
            sync_run
                .created_at
                .format(&time::format_description::well_known::Iso8601::DATE)
                .unwrap_or_else(|_| "unknown-date".to_string()),
            sync_run.id
        )
    }

    pub async fn sync_all_sources(&self) -> Result<()> {
        let sources = self.get_active_sources().await?;

        info!(
            "Found {} active Google sources (Drive & Gmail)",
            sources.len()
        );

        for source in sources {
            // Check if this source already has a running sync
            match self.get_running_sync_for_source(&source.id).await {
                Ok(Some(running_sync)) => {
                    info!(
                        "Source {} already has a running sync (id: {}), skipping scheduled sync",
                        source.id, running_sync.id
                    );
                    continue;
                }
                Ok(None) => {
                    // No running sync, proceed with checking if we should run a full sync
                }
                Err(e) => {
                    error!(
                        "Failed to check for running sync for source {}: {}",
                        source.id, e
                    );
                    continue;
                }
            }

            // Check if we should run a full sync for this source
            match self.should_run_full_sync(&source.id).await {
                Ok(should_sync) => {
                    if should_sync {
                        if let Err(e) = self.sync_source(&source).await {
                            error!("Failed to sync source {}: {}", source.id, e);
                            self.update_source_status(&source.id, "failed").await?;
                        }
                    }
                }
                Err(e) => {
                    error!(
                        "Failed to check sync status for source {}: {}",
                        source.id, e
                    );
                }
            }
        }

        Ok(())
    }

    async fn get_active_sources(&self) -> Result<Vec<Source>> {
        let sources = self
            .source_repo
            .find_active_by_types(vec![SourceType::GoogleDrive, SourceType::Gmail])
            .await?;

        // Filter to only sources that have Google service credentials
        let mut sources_with_creds = Vec::new();
        for source in sources {
            if let Ok(Some(creds)) = self
                .service_credentials_repo
                .get_by_source_id(&source.id)
                .await
            {
                // Verify it's a Google credential
                if creds.provider == ServiceProvider::Google {
                    sources_with_creds.push(source);
                }
            }
        }

        Ok(sources_with_creds)
    }

    async fn sync_source(&self, source: &Source) -> Result<()> {
        info!("Syncing source: {} ({})", source.name, source.id);

        // Double-check for running sync to prevent race conditions
        if let Some(running_sync) = self.get_running_sync_for_source(&source.id).await? {
            warn!(
                "Found running sync for source {} (id: {}) just before creating new sync, aborting",
                source.id, running_sync.id
            );
            return Ok(());
        }

        // Check if this source type should be synced
        if !matches!(
            source.source_type,
            SourceType::GoogleDrive | SourceType::Gmail
        ) {
            info!(
                "Skipping sync for source {} ({}) - source type {:?} is not a Google service",
                source.name, source.id, source.source_type
            );
            return Ok(());
        }

        // Create a sync run record
        let sync_run = self
            .sync_run_repo
            .create(&source.id, SyncType::Full)
            .await?;

        let result = match source.source_type {
            SourceType::GoogleDrive => self.sync_drive_source_internal(source, &sync_run).await,
            SourceType::Gmail => self.sync_gmail_source_internal(source, &sync_run).await,
            _ => {
                return Err(anyhow::anyhow!(
                    "Unsupported source type: {:?}",
                    source.source_type
                ))
            }
        };

        // Update sync run based on result
        match &result {
            Ok((files_scanned, files_processed, files_updated)) => {
                self.sync_run_repo
                    .mark_completed(
                        &sync_run.id,
                        *files_scanned as i32,
                        *files_processed as i32,
                        *files_updated as i32,
                    )
                    .await?;
            }
            Err(e) => {
                self.sync_run_repo
                    .mark_failed(&sync_run.id, &e.to_string())
                    .await?;
            }
        }

        result.map(|_| ())
    }

    async fn sync_drive_for_user(
        &self,
        user_email: &str,
        service_auth: Arc<ServiceAccountAuth>,
        source_id: &str,
        sync_run: &SyncRun,
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

            debug!(
                "Got {} files in this page with page_token: '{:?}' for user {}",
                response.files.len(),
                page_token,
                user_email
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
                                    sync_run,
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
                    sync_run,
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

    async fn process_file_batch(
        &self,
        files: Vec<UserFile>,
        source_id: &str,
        sync_run: &SyncRun,
        sync_state: &SyncState,
        service_auth: Arc<ServiceAccountAuth>,
    ) -> Result<(usize, usize)> {
        info!("Processing batch of {} files", files.len());

        let mut processed = 0;
        let mut updated = 0;

        // Create storage prefix from sync_run
        let storage_prefix = Self::get_storage_prefix(sync_run);

        // Process files concurrently within the batch
        let tasks = files.into_iter().map(|user_file| {
            let service_auth = service_auth.clone();
            let source_id = source_id.to_string();
            let sync_run_id = sync_run.id.clone();
            let storage_prefix = storage_prefix.clone();
            let sync_state = sync_state.clone();
            let drive_client = self.drive_client.clone();
            let event_queue = self.event_queue.clone();
            let content_storage = self.content_storage.clone();

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
                            match content_storage.store_text(&content, Some(&storage_prefix)).await {
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

                                    match event_queue.enqueue(&source_id, &event).await {
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
        sync_run: &SyncRun,
    ) -> Result<(usize, usize, usize)> {
        let service_creds = self.get_service_credentials(&source.id).await?;
        let service_auth = Arc::new(self.create_service_auth(&service_creds, source.source_type)?);
        let domain = self.get_domain_from_credentials(&service_creds)?;
        let user_email = self.get_user_email_from_source(&source.id).await
            .map_err(|e| anyhow::anyhow!("Failed to get user email for source {}: {}. Make sure the source has a valid creator.", source.id, e))?;

        // Calculate cutoff date for filtering
        let (drive_cutoff_date, _gmail_cutoff_date) = self.get_cutoff_date()?;
        info!("Using Drive cutoff date: {}", drive_cutoff_date);

        // Get all users in the organization
        info!("Listing all users in domain: {}", domain);

        // Use the logged-in user's email to list all users (they should be a super-admin)
        info!("Using user email: {}", user_email);
        let admin_access_token = service_auth.get_access_token(&user_email).await
            .map_err(|e| anyhow::anyhow!("Failed to get access token for user {}: {}. Make sure the user is a super-admin and the service account has domain-wide delegation enabled.", user_email, e))?;
        let all_users = self
            .admin_client
            .list_all_users(&admin_access_token, &domain)
            .await?;
        info!("Found {} users in domain {}", all_users.len(), domain);

        // Apply user filtering based on source settings
        let filtered_users = all_users
            .into_iter()
            .filter(|user| source.should_index_user(&user.primary_email))
            .collect::<Vec<_>>();
        info!(
            "After filtering: {} users will be indexed",
            filtered_users.len()
        );

        let sync_state = SyncState::new(self.redis_client.clone());
        let synced_files = sync_state.get_all_synced_file_ids(&source.id).await?;
        let current_files = Arc::new(std::sync::Mutex::new(HashSet::new()));

        info!(
            "Starting sequential user processing for {} users",
            filtered_users.len()
        );

        let results: Vec<Result<(usize, usize)>> = stream::iter(filtered_users)
            .map(|user| {
                let service_auth = service_auth.clone();
                let sync_state = sync_state.clone();
                let current_files = current_files.clone();
                let drive_cutoff_date = drive_cutoff_date.clone();

                async move {
                    info!("Processing user: {}", user.primary_email);
                    let _token = service_auth.get_access_token(&user.primary_email)
                        .await
                        .inspect_err(|e| {
                            error!("Failed to get access token for user {}: {}. This user may not have Drive access.", user.primary_email, e);
                        })
                        .with_context(|| format!("Failed to get access token for user: {}", user.primary_email))?;

                    let res =
                        self.sync_drive_for_user(
                            &user.primary_email,
                            service_auth.clone(),
                            &source.id,
                            sync_run,
                            &sync_state,
                            current_files.clone(),
                            Some(&drive_cutoff_date),
                        )
                        .await;

                    match &res {
                        Ok((processed, updated)) => {
                            info!(
                                "User {} completed: {} processed, {} updated",
                                user.primary_email, processed, updated
                            );
                        }
                        Err(e) => {
                            error!("Failed to process user {}: {}", user.primary_email, e);
                        }
                    }

                    res
                }
            })
            .buffer_unordered(10)
            .collect()
            .await;

        let mut total_processed = 0;
        let mut total_updated = 0;
        let mut errors = 0;

        for result in results {
            match result {
                Ok((processed, updated)) => {
                    total_processed += processed;
                    total_updated += updated;
                }
                Err(_) => errors += 1,
            }
        }

        info!(
            "User processing complete. Total: {} processed, {} updated, {} errors",
            total_processed, total_updated, errors
        );

        // Handle deletions
        let current_files_set = {
            let current_files_guard = current_files.lock().unwrap();
            current_files_guard.clone()
        };

        for deleted_file_id in synced_files.difference(&current_files_set) {
            info!(
                "File {} was deleted, publishing deletion event",
                deleted_file_id
            );
            self.publish_deletion_event(&sync_run.id, &source.id, deleted_file_id)
                .await?;
            sync_state
                .delete_file_sync_state(&source.id, deleted_file_id)
                .await?;
        }

        info!(
            "Sync completed for source {}: {} files discovered, {} processed, {} updated",
            source.id,
            current_files_set.len(),
            total_processed,
            total_updated
        );

        self.update_source_status(&source.id, "completed").await?;

        // Clear folder cache to free memory after sync
        self.folder_cache.clear();

        info!("Completed sync for source: {}", source.id);
        Ok((current_files_set.len(), total_processed, total_updated))
    }

    async fn sync_gmail_source_internal(
        &self,
        source: &Source,
        sync_run: &SyncRun,
    ) -> Result<(usize, usize, usize)> {
        let service_creds = self.get_service_credentials(&source.id).await?;
        let service_auth = Arc::new(self.create_service_auth(&service_creds, source.source_type)?);
        let domain = self.get_domain_from_credentials(&service_creds)?;
        let user_email = self.get_user_email_from_source(&source.id).await
            .map_err(|e| anyhow::anyhow!("Failed to get user email for source {}: {}. Make sure the source has a valid creator.", source.id, e))?;

        // Calculate cutoff date for filtering
        let (_drive_cutoff_date, gmail_cutoff_date) = self.get_cutoff_date()?;
        info!("Using Gmail cutoff date: {}", gmail_cutoff_date);

        // Get all users in the organization
        info!("Listing all users in domain: {}", domain);

        // Use the logged-in user's email to list all users (they should be a super-admin)
        info!("Using user email: {}", user_email);
        let admin_access_token = service_auth.get_access_token(&user_email).await
            .map_err(|e| anyhow::anyhow!("Failed to get access token for user {}: {}. Make sure the user is a super-admin and the service account has domain-wide delegation enabled.", user_email, e))?;
        let all_users = self
            .admin_client
            .list_all_users(&admin_access_token, &domain)
            .await?;
        info!("Found {} users in domain {}", all_users.len(), domain);

        // Apply user filtering based on source settings
        let filtered_users = all_users
            .into_iter()
            .filter(|user| source.should_index_user(&user.primary_email))
            .collect::<Vec<_>>();
        info!(
            "After filtering: {} users will be indexed",
            filtered_users.len()
        );

        let processed_threads = Arc::new(std::sync::Mutex::new(HashSet::<String>::new()));

        info!(
            "Starting sequential user processing for {} users (Gmail only)",
            filtered_users.len()
        );

        let mut total_processed = 0;
        let mut total_updated = 0;

        for user in filtered_users {
            let user_email = user.primary_email.clone();

            // Get access token for this user
            match service_auth.get_access_token(&user_email).await {
                Ok(_token) => {
                    info!("Processing user: {}", user_email);
                    match self
                        .sync_gmail_for_user(
                            &user_email,
                            service_auth.clone(),
                            &source.id,
                            sync_run,
                            processed_threads.clone(),
                            Some(&gmail_cutoff_date),
                        )
                        .await
                    {
                        Ok((processed, updated)) => {
                            total_processed += processed;
                            total_updated += updated;
                            info!(
                                "User {} Gmail sync completed: {} processed, {} updated",
                                user_email, processed, updated
                            );
                        }
                        Err(e) => {
                            error!("Failed to process Gmail for user {}: {}", user_email, e);
                        }
                    }
                }
                Err(e) => {
                    warn!("Failed to get access token for user {}: {}. This user may not have Gmail access.", user_email, e);
                }
            }
        }

        info!(
            "Gmail sync completed for source {}: {} total processed, {} total updated",
            source.id, total_processed, total_updated
        );

        self.update_source_status(&source.id, "completed").await?;

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

    async fn publish_connector_event(&self, event: ConnectorEvent) -> Result<()> {
        let source_id = event.source_id();
        self.event_queue.enqueue(source_id, &event).await?;
        Ok(())
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

        self.publish_connector_event(event).await
    }

    async fn get_service_credentials(&self, source_id: &str) -> Result<ServiceCredentials> {
        let creds = self
            .service_credentials_repo
            .get_by_source_id(source_id)
            .await?
            .ok_or_else(|| {
                anyhow::anyhow!("Service credentials not found for source {}", source_id)
            })?;

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

    fn get_domain_from_credentials(&self, creds: &ServiceCredentials) -> Result<String> {
        creds
            .config
            .get("domain")
            .and_then(|v| v.as_str())
            .map(String::from)
            .ok_or_else(|| anyhow::anyhow!("Missing domain in service credentials config"))
    }

    async fn get_user_email_from_source(&self, source_id: &str) -> Result<String> {
        let user_email = sqlx::query_scalar::<_, String>(
            "SELECT u.email FROM sources s
             JOIN users u ON s.created_by = u.id
             WHERE s.id = $1",
        )
        .bind(source_id)
        .fetch_one(&self.pool)
        .await?;

        Ok(user_email)
    }

    async fn update_source_status(&self, source_id: &str, status: &str) -> Result<()> {
        let now = chrono::Utc::now();
        self.source_repo
            .update_sync_status(source_id, status, Some(now), None)
            .await?;

        Ok(())
    }

    pub async fn sync_source_by_id(&self, source_id: String) -> Result<()> {
        info!("Manually triggered sync for source: {}", source_id);

        let source = self
            .source_repo
            .find_by_id(source_id.clone())
            .await?
            .ok_or_else(|| anyhow::anyhow!("Source {} not found", source_id))?;

        // Verify it's a Google source type
        if source.source_type != SourceType::GoogleDrive && source.source_type != SourceType::Gmail
        {
            return Err(anyhow::anyhow!(
                "Source {} is not a Google Drive or Gmail source",
                source_id
            ));
        }

        if !source.is_active {
            return Err(anyhow::anyhow!("Source {} is not active", source_id));
        }

        // Manual sync always runs regardless of last sync time
        self.sync_source(&source).await
    }

    async fn get_last_completed_full_sync(&self, source_id: &str) -> Result<Option<SyncRun>> {
        Ok(self
            .sync_run_repo
            .get_last_completed_for_source(source_id, SyncType::Full)
            .await?)
    }

    pub async fn should_run_full_sync(&self, source_id: &str) -> Result<bool> {
        let last_sync = self.get_last_completed_full_sync(source_id).await?;

        match last_sync {
            Some(sync_run) => {
                let sync_interval_seconds = std::env::var("GOOGLE_SYNC_INTERVAL_SECONDS")
                    .unwrap_or_else(|_| "86400".to_string())
                    .parse::<i64>()
                    .expect("GOOGLE_SYNC_INTERVAL_SECONDS must be a valid number");

                if let Some(completed_at) = sync_run.completed_at {
                    let now = OffsetDateTime::now_utc();
                    let elapsed = now - completed_at;
                    let should_sync = elapsed.whole_seconds() >= sync_interval_seconds;

                    if !should_sync {
                        info!(
                            "Skipping full sync for source {}. Last sync was {} seconds ago, interval is {} seconds",
                            source_id,
                            elapsed.whole_seconds(),
                            sync_interval_seconds
                        );
                    }

                    Ok(should_sync)
                } else {
                    // If completed_at is None, the sync didn't complete properly
                    Ok(true)
                }
            }
            None => {
                // No previous sync found, should run
                info!(
                    "No previous full sync found for source {}, will run full sync",
                    source_id
                );
                Ok(true)
            }
        }
    }

    pub async fn handle_webhook_notification(
        &self,
        notification: WebhookNotification,
    ) -> Result<()> {
        info!(
            "Handling webhook notification for channel {}, state: {}",
            notification.channel_id, notification.resource_state
        );

        // Find the source associated with this webhook channel
        let webhook_channel = match self
            .get_webhook_channel_by_channel_id(&notification.channel_id)
            .await?
        {
            Some(channel) => channel,
            None => {
                warn!(
                    "Received webhook notification for unknown channel: {}",
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
            "add" | "update" | "remove" | "trash" | "untrash" => {
                // Trigger incremental sync for the specific source
                info!(
                    "Triggering incremental sync for source {} due to resource state: {}",
                    webhook_channel.source_id, notification.resource_state
                );

                // Get the source
                if let Some(source) = self
                    .source_repo
                    .find_by_id(webhook_channel.source_id.clone())
                    .await?
                {
                    if let Err(e) = self.sync_source_incremental(&source).await {
                        error!(
                            "Failed to run incremental sync for source {}: {}",
                            source.id, e
                        );
                    }
                } else {
                    warn!(
                        "Source {} not found for webhook channel",
                        webhook_channel.source_id
                    );
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

    pub async fn register_webhook_for_source(
        &self,
        source_id: &str,
        webhook_url: String,
    ) -> Result<WebhookChannelResponse> {
        // Check if there's already an active webhook for this source
        if let Some(existing_channel) = self.get_webhook_channel_by_source_id(source_id).await? {
            info!(
                "Found existing webhook channel for source {}, stopping it first",
                source_id
            );
            if let Err(e) = self
                .stop_webhook_for_source(
                    source_id,
                    &existing_channel.channel_id,
                    &existing_channel.resource_id,
                )
                .await
            {
                warn!("Failed to stop existing webhook channel: {}", e);
            }
        }

        let service_creds = self.get_service_credentials(source_id).await?;
        let service_auth = self.create_service_auth(&service_creds, SourceType::GoogleDrive)?;
        let user_email = self.get_user_email_from_source(source_id).await
            .map_err(|e| anyhow::anyhow!("Failed to get user email for source {}: {}. Make sure the source has a valid creator.", source_id, e))?;
        let access_token = service_auth.get_access_token(&user_email).await?;

        // Get the current start page token for change tracking
        let start_page_token = self
            .drive_client
            .get_start_page_token(&access_token)
            .await?;

        // Create webhook channel
        let webhook_channel = WebhookChannel::new(webhook_url.clone(), None);

        // Register the webhook with Google
        let webhook_response = self
            .drive_client
            .register_changes_webhook(&access_token, &webhook_channel, &start_page_token)
            .await?;

        // Parse expiration timestamp from Google response
        let expires_at = webhook_response.expiration.as_ref().and_then(|exp| {
            exp.parse::<i64>().ok().and_then(|millis| {
                sqlx::types::time::OffsetDateTime::from_unix_timestamp(millis / 1000).ok()
            })
        });

        // Store webhook channel in database
        self.save_webhook_channel(
            source_id,
            &webhook_response.id,
            &webhook_response.resource_id,
            Some(&webhook_response.resource_uri),
            &webhook_url,
            expires_at,
        )
        .await?;

        info!(
            "Successfully registered and saved webhook for source {}: channel_id={}, resource_id={}",
            source_id, webhook_response.id, webhook_response.resource_id
        );

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

        // Stop the webhook with Google
        self.drive_client
            .stop_webhook_channel(&access_token, channel_id, resource_id)
            .await?;

        // Remove from database
        self.delete_webhook_channel_by_channel_id(channel_id)
            .await?;

        info!(
            "Successfully stopped and removed webhook for source {}: channel_id={}",
            source_id, channel_id
        );
        Ok(())
    }

    async fn sync_source_incremental(&self, source: &Source) -> Result<()> {
        info!(
            "Running incremental sync for source: {} ({})",
            source.name, source.id
        );

        let service_creds = self.get_service_credentials(&source.id).await?;
        let service_auth = self.create_service_auth(&service_creds, source.source_type)?;
        let user_email = self.get_user_email_from_source(&source.id).await
            .map_err(|e| anyhow::anyhow!("Failed to get user email for source {}: {}. Make sure the source has a valid creator.", source.id, e))?;
        let access_token = service_auth.get_access_token(&user_email).await?;

        // Create rate limiter for incremental sync API calls
        let rate_limiter = Arc::new(RateLimiter::new(5, 3)); // 5 req/sec, 3 retries

        // For incremental sync, we would ideally use the changes API with a stored page token
        // For now, we'll just get the latest changes using the current start page token
        let start_page_token = self
            .drive_client
            .get_start_page_token(&access_token)
            .await?;

        // Create a sync run record for incremental sync
        let sync_run = self
            .sync_run_repo
            .create(&source.id, SyncType::Incremental)
            .await?;

        // Create storage prefix from sync_run
        let storage_prefix = Self::get_storage_prefix(&sync_run);

        // List recent changes
        match self
            .drive_client
            .list_changes(&access_token, &start_page_token)
            .await
        {
            Ok(changes_response) => {
                let mut processed_count = 0;
                let mut updated_count = 0;

                for change in changes_response.changes {
                    processed_count += 1;

                    match change.removed {
                        Some(true) => {
                            // File was removed
                            if let Some(file_id) = &change.file_id {
                                info!("Processing deletion for file_id: {}", file_id);
                                self.publish_deletion_event(&sync_run.id, &source.id, file_id)
                                    .await?;

                                let sync_state = SyncState::new(self.redis_client.clone());
                                sync_state
                                    .delete_file_sync_state(&source.id, file_id)
                                    .await?;
                                updated_count += 1;
                            }
                        }
                        _ => {
                            // File was added or updated
                            if let Some(file) = change.file {
                                if self.should_index_file(&file) {
                                    match self
                                        .drive_client
                                        .get_file_content(&service_auth, &user_email, &file)
                                        .await
                                    {
                                        Ok(content) => {
                                            if !content.is_empty() {
                                                // Resolve the full path for this file
                                                let file_path = match self
                                                    .resolve_file_path(
                                                        &service_auth,
                                                        &user_email,
                                                        &file,
                                                    )
                                                    .await
                                                {
                                                    Ok(path) => Some(path),
                                                    Err(e) => {
                                                        warn!("Failed to resolve path for file {}: {}", file.name, e);
                                                        None
                                                    }
                                                };

                                                // Store content in LOB and get OID
                                                let content_id = match self
                                                    .content_storage
                                                    .store_text(&content, Some(&storage_prefix))
                                                    .await
                                                {
                                                    Ok(oid) => oid,
                                                    Err(e) => {
                                                        error!("Failed to store content in LOB storage for file {}: {}", file.name, e);
                                                        continue;
                                                    }
                                                };

                                                let event = file.to_connector_event(
                                                    &sync_run.id,
                                                    &source.id,
                                                    &content_id,
                                                    file_path,
                                                );

                                                match self.publish_connector_event(event).await {
                                                    Ok(_) => {
                                                        updated_count += 1;
                                                        if let Some(modified_time) =
                                                            &file.modified_time
                                                        {
                                                            let sync_state = SyncState::new(
                                                                self.redis_client.clone(),
                                                            );
                                                            sync_state
                                                                .set_file_sync_state(
                                                                    &source.id,
                                                                    &file.id,
                                                                    modified_time,
                                                                )
                                                                .await?;
                                                        }
                                                    }
                                                    Err(e) => {
                                                        error!(
                                                            "Failed to queue event for file {}: {}",
                                                            file.name, e
                                                        );
                                                    }
                                                }
                                            }
                                        }
                                        Err(e) => {
                                            warn!(
                                                "Failed to get content for file {}: {}",
                                                file.name, e
                                            );
                                        }
                                    }
                                }
                            }
                        }
                    }
                }

                self.sync_run_repo
                    .mark_completed(
                        &sync_run.id,
                        processed_count as i32,
                        processed_count as i32,
                        updated_count as i32,
                    )
                    .await?;
                info!(
                    "Incremental sync completed for source {}: {} changes processed, {} updated",
                    source.id, processed_count, updated_count
                );
            }
            Err(e) => {
                error!("Failed to list changes for source {}: {}", source.id, e);
                self.sync_run_repo
                    .mark_failed(&sync_run.id, &e.to_string())
                    .await?;
            }
        }

        Ok(())
    }

    // Database operations for webhook channels
    async fn save_webhook_channel(
        &self,
        source_id: &str,
        channel_id: &str,
        resource_id: &str,
        resource_uri: Option<&str>,
        webhook_url: &str,
        expires_at: Option<sqlx::types::time::OffsetDateTime>,
    ) -> Result<()> {
        let id = generate_ulid();

        sqlx::query(
            "INSERT INTO webhook_channels (id, source_id, channel_id, resource_id, resource_uri, webhook_url, expires_at)
             VALUES ($1, $2, $3, $4, $5, $6, $7)",
        )
        .bind(&id)
        .bind(source_id)
        .bind(channel_id)
        .bind(resource_id)
        .bind(resource_uri)
        .bind(webhook_url)
        .bind(expires_at)
        .execute(&self.pool)
        .await?;

        Ok(())
    }

    async fn get_webhook_channel_by_channel_id(
        &self,
        channel_id: &str,
    ) -> Result<Option<DatabaseWebhookChannel>> {
        let webhook_channel = sqlx::query_as::<_, DatabaseWebhookChannel>(
            "SELECT * FROM webhook_channels WHERE channel_id = $1",
        )
        .bind(channel_id)
        .fetch_optional(&self.pool)
        .await?;

        Ok(webhook_channel)
    }

    async fn get_webhook_channel_by_source_id(
        &self,
        source_id: &str,
    ) -> Result<Option<DatabaseWebhookChannel>> {
        let webhook_channel = sqlx::query_as::<_, DatabaseWebhookChannel>(
            "SELECT * FROM webhook_channels WHERE source_id = $1",
        )
        .bind(source_id)
        .fetch_optional(&self.pool)
        .await?;

        Ok(webhook_channel)
    }

    async fn delete_webhook_channel_by_channel_id(&self, channel_id: &str) -> Result<()> {
        sqlx::query("DELETE FROM webhook_channels WHERE channel_id = $1")
            .bind(channel_id)
            .execute(&self.pool)
            .await?;

        Ok(())
    }

    pub async fn get_expiring_webhook_channels(
        &self,
        hours_ahead: i64,
    ) -> Result<Vec<DatabaseWebhookChannel>> {
        let threshold =
            sqlx::types::time::OffsetDateTime::now_utc() + time::Duration::hours(hours_ahead);

        let channels = sqlx::query_as::<_, DatabaseWebhookChannel>(
            "SELECT * FROM webhook_channels WHERE expires_at IS NOT NULL AND expires_at <= $1",
        )
        .bind(threshold)
        .fetch_all(&self.pool)
        .await?;

        Ok(channels)
    }

    pub async fn renew_webhook_channel(
        &self,
        webhook_channel: &DatabaseWebhookChannel,
    ) -> Result<()> {
        info!(
            "Renewing webhook channel {} for source {}",
            webhook_channel.channel_id, webhook_channel.source_id
        );

        // Stop the old webhook
        if let Err(e) = self
            .stop_webhook_for_source(
                &webhook_channel.source_id,
                &webhook_channel.channel_id,
                &webhook_channel.resource_id,
            )
            .await
        {
            warn!("Failed to stop expiring webhook channel: {}", e);
        }

        // Register a new webhook
        match self
            .register_webhook_for_source(
                &webhook_channel.source_id,
                webhook_channel.webhook_url.clone(),
            )
            .await
        {
            Ok(new_response) => {
                info!(
                    "Successfully renewed webhook for source {}: old_channel={}, new_channel={}",
                    webhook_channel.source_id, webhook_channel.channel_id, new_response.id
                );
            }
            Err(e) => {
                error!(
                    "Failed to renew webhook for source {}: {}",
                    webhook_channel.source_id, e
                );
                return Err(e);
            }
        }

        Ok(())
    }

    pub async fn auto_register_webhooks(&self) -> Result<()> {
        info!("Starting automatic webhook registration for all sources");

        let webhook_url = match std::env::var("GOOGLE_WEBHOOK_URL") {
            Ok(url) if !url.is_empty() => url,
            _ => {
                info!("GOOGLE_WEBHOOK_URL not set, skipping automatic webhook registration");
                return Ok(());
            }
        };

        let sources = self.get_active_sources().await?;
        info!(
            "Found {} active Google sources (Drive & Gmail) for webhook registration",
            sources.len()
        );

        for source in sources {
            match self.get_webhook_channel_by_source_id(&source.id).await? {
                Some(_existing_channel) => {
                    info!(
                        "Webhook already exists for source {}, skipping registration",
                        source.id
                    );
                }
                None => {
                    info!(
                        "No webhook found for source {}, registering new webhook",
                        source.id
                    );
                    match self
                        .register_webhook_for_source(&source.id, webhook_url.clone())
                        .await
                    {
                        Ok(response) => {
                            info!(
                                "Successfully registered webhook for source {}: channel_id={}",
                                source.id, response.id
                            );
                        }
                        Err(e) => {
                            error!("Failed to register webhook for source {}: {}", source.id, e);
                            // Continue with other sources even if one fails
                        }
                    }
                }
            }
        }

        info!("Completed automatic webhook registration");
        Ok(())
    }

    pub async fn get_running_sync_for_source(&self, source_id: &str) -> Result<Option<SyncRun>> {
        Ok(self.sync_run_repo.get_running_for_source(source_id).await?)
    }

    pub async fn recover_interrupted_syncs(&self) -> Result<()> {
        info!("Checking for interrupted running syncs from previous connector instance");

        let running_syncs = self.sync_run_repo.find_all_running().await?;

        if running_syncs.is_empty() {
            info!("No interrupted running syncs found");
            return Ok(());
        }

        info!(
            "Found {} interrupted running syncs, marking as failed",
            running_syncs.len()
        );

        for sync_run in running_syncs {
            info!(
                "Marking interrupted sync as failed: id={}, source_id={}, started_at={:?}",
                sync_run.id, sync_run.source_id, sync_run.started_at
            );

            let error_message = "Sync interrupted by connector restart";

            if let Err(e) = self
                .sync_run_repo
                .mark_failed(&sync_run.id, error_message)
                .await
            {
                error!(
                    "Failed to mark interrupted sync {} as failed: {}",
                    sync_run.id, e
                );
            }
        }

        info!("Completed interrupted sync recovery");
        Ok(())
    }

    async fn resolve_file_path(
        &self,
        auth: &ServiceAccountAuth,
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
        auth: &ServiceAccountAuth,
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
        service_auth: Arc<ServiceAccountAuth>,
        source_id: &str,
        sync_run: &SyncRun,
        processed_threads: Arc<std::sync::Mutex<HashSet<String>>>,
        created_after: Option<&str>,
    ) -> Result<(usize, usize)> {
        info!("Processing Gmail for user: {}", user_email);

        // Create storage prefix from sync_run
        let storage_prefix = Self::get_storage_prefix(sync_run);

        let mut total_processed = 0;
        let mut total_updated = 0;
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
                debug!(
                    "Got {} threads in this page for user {}",
                    threads.len(),
                    user_email
                );

                for thread_info in threads {
                    user_threads.push(thread_info.id);
                }
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

        // Step 2: Process threads in batches of 100
        let sync_state = SyncState::new(self.redis_client.clone());
        const THREAD_BATCH_SIZE: usize = 50;

        for chunk in user_threads.chunks(THREAD_BATCH_SIZE) {
            // Filter out already processed threads
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

            // Mark threads as processed to prevent other users from processing them
            {
                let mut processed_guard = processed_threads.lock().unwrap();
                for thread_id in &unprocessed_threads {
                    processed_guard.insert(thread_id.clone());
                }
            }

            debug!("Processing batch of {} threads", unprocessed_threads.len());

            // Step 3: Fetch threads in batch
            let batch_results = match self
                .gmail_client
                .batch_get_threads(
                    &service_auth,
                    &user_email,
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

            // Process each thread response
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

                // Convert API response to our GmailThread model
                let mut gmail_thread = GmailThread::new(thread_id.clone());
                for message in thread_response.messages {
                    gmail_thread.add_message(message);
                }

                // Check if we've already indexed this thread by comparing timestamps
                if !gmail_thread.latest_date.is_empty() {
                    match sync_state
                        .get_thread_sync_state(source_id, &thread_id)
                        .await
                    {
                        Ok(Some(last_synced_date)) => {
                            // Parse timestamps for proper comparison
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
                            // Continue processing if we can't check sync state
                        }
                    }
                }

                // Step 4: Generate content and store
                if gmail_thread.total_messages > 0 {
                    match gmail_thread.aggregate_content(&self.gmail_client) {
                        Ok(content) => {
                            if !content.trim().is_empty() {
                                // Store content in LOB storage
                                match self
                                    .content_storage
                                    .store_text(&content, Some(&storage_prefix))
                                    .await
                                {
                                    Ok(content_id) => {
                                        // Create connector event
                                        match gmail_thread.to_connector_event(
                                            &sync_run.id,
                                            source_id,
                                            &content_id,
                                            &self.gmail_client,
                                        ) {
                                            Ok(event) => {
                                                match self
                                                    .event_queue
                                                    .enqueue(source_id, &event)
                                                    .await
                                                {
                                                    Ok(_) => {
                                                        total_updated += 1;
                                                        info!(
                                                            "Successfully queued Gmail thread {}",
                                                            thread_id
                                                        );

                                                        // Update sync state with thread's latest date
                                                        if let Err(e) = sync_state
                                                            .set_thread_sync_state(
                                                                source_id,
                                                                &thread_id,
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

                // Explicitly drop thread data to free memory
                drop(gmail_thread);
            }

            // Explicitly drop batch results to free memory
            drop(unprocessed_threads);
        }

        info!(
            "Completed Gmail processing for user {}: {} threads processed, {} updated",
            user_email, total_processed, total_updated
        );

        Ok((total_processed, total_updated))
    }

    pub async fn startup_sync_check(&self) -> Result<()> {
        info!(
            "Running startup sync check: recovering interrupted syncs and checking sync schedule"
        );

        // First, check for interrupted running syncs
        let running_syncs = sqlx::query_as::<_, SyncRun>(
            "SELECT * FROM sync_runs WHERE status = $1 ORDER BY started_at ASC",
        )
        .bind(SyncStatus::Running)
        .fetch_all(&self.pool)
        .await?;

        if !running_syncs.is_empty() {
            info!(
                "Found {} interrupted running syncs, continuing them",
                running_syncs.len()
            );

            for sync_run in running_syncs {
                info!(
                    "Continuing interrupted sync: id={}, source_id={}, started_at={:?}",
                    sync_run.id, sync_run.source_id, sync_run.started_at
                );

                // Get the source for this sync run
                if let Some(source) = self
                    .source_repo
                    .find_by_id(sync_run.source_id.clone())
                    .await?
                {
                    if source.is_active {
                        // Continue the sync using the existing sync run ID
                        let result = match source.source_type {
                            SourceType::GoogleDrive => {
                                self.sync_drive_source_internal(&source, &sync_run).await
                            }
                            SourceType::Gmail => {
                                self.sync_gmail_source_internal(&source, &sync_run).await
                            }
                            _ => Err(anyhow::anyhow!(
                                "Unsupported source type: {:?}",
                                source.source_type
                            )),
                        };

                        // Update sync run based on result
                        match result {
                            Ok((files_scanned, files_processed, files_updated)) => {
                                self.sync_run_repo
                                    .mark_completed(
                                        &sync_run.id,
                                        files_scanned as i32,
                                        files_processed as i32,
                                        files_updated as i32,
                                    )
                                    .await?;
                                info!("Successfully continued and completed sync {}", sync_run.id);
                            }
                            Err(e) => {
                                error!("Failed to continue sync {}: {}", sync_run.id, e);
                                self.sync_run_repo
                                    .mark_failed(&sync_run.id, &e.to_string())
                                    .await?;
                                self.update_source_status(&source.id, "failed").await?;
                            }
                        }
                    } else {
                        info!("Source {} is not active, marking sync as failed", source.id);
                        self.sync_run_repo
                            .mark_failed(&sync_run.id, "Source is not active")
                            .await?;
                    }
                } else {
                    warn!(
                        "Source {} not found for sync run {}, marking as failed",
                        sync_run.source_id, sync_run.id
                    );
                    self.sync_run_repo
                        .mark_failed(&sync_run.id, "Source not found")
                        .await?;
                }
            }

            info!("Interrupted sync recovery completed");
        } else {
            info!("No interrupted running syncs found");
        }

        // Now check for scheduled syncs for sources without running syncs
        let sources = self.get_active_sources().await?;
        info!(
            "Found {} active Google sources (Drive & Gmail)",
            sources.len()
        );

        for source in sources {
            // Check if this source already has a running sync (from recovery above)
            if let Some(_running_sync) = self.get_running_sync_for_source(&source.id).await? {
                info!(
                    "Source {} already has a running sync, skipping scheduled check",
                    source.id
                );
                continue;
            }

            match self.should_run_full_sync(&source.id).await {
                Ok(should_sync) => {
                    if should_sync {
                        if let Err(e) = self.sync_source(&source).await {
                            error!("Failed to sync source {}: {}", source.id, e);
                            self.update_source_status(&source.id, "failed").await?;
                        }
                    }
                }
                Err(e) => {
                    error!(
                        "Failed to check sync status for source {}: {}",
                        source.id, e
                    );
                }
            }
        }

        info!("Startup sync check completed");
        Ok(())
    }
}

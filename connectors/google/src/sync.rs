use anyhow::{Context, Result};
use futures::stream::{self, StreamExt};
use redis::{AsyncCommands, Client as RedisClient};
use sqlx::types::time::OffsetDateTime;
use sqlx::PgPool;
use std::collections::{HashMap, HashSet};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::Instant;
use time;
use tokio::sync::Semaphore;
use tokio::time::{interval, Duration};
use tracing::{debug, error, info, warn};

use crate::admin::AdminClient;
use crate::auth::ServiceAccountAuth;
use crate::drive::DriveClient;
use crate::models::{WebhookChannel, WebhookChannelResponse, WebhookNotification};
use shared::models::{
    ConnectorEvent, ServiceCredentials, Source, SourceType, SyncRun, SyncStatus, SyncType,
    WebhookChannel as DatabaseWebhookChannel,
};
use shared::queue::EventQueue;
use shared::utils::generate_ulid;
use shared::RateLimiter;

pub struct SyncManager {
    pool: PgPool,
    redis_client: RedisClient,
    drive_client: DriveClient,
    admin_client: AdminClient,
    event_queue: EventQueue,
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
        format!("google:sync:{}:{}", source_id, file_id)
    }

    pub fn get_test_file_sync_key(&self, source_id: &str, file_id: &str) -> String {
        format!("google:sync:test:{}:{}", source_id, file_id)
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
            format!("google:sync:test:{}:*", source_id)
        } else {
            format!("google:sync:{}:*", source_id)
        };

        let keys: Vec<String> = conn.keys(&pattern).await?;
        let prefix = if cfg!(test) {
            format!("google:sync:test:{}:", source_id)
        } else {
            format!("google:sync:{}:", source_id)
        };
        let file_ids: HashSet<String> = keys
            .into_iter()
            .filter_map(|key| key.strip_prefix(&prefix).map(|s| s.to_string()))
            .collect();

        Ok(file_ids)
    }
}

impl SyncManager {
    pub async fn new(pool: PgPool, redis_client: RedisClient) -> Result<Self> {
        let event_queue = EventQueue::new(pool.clone());

        let api_rate_limit = std::env::var("GOOGLE_API_RATE_LIMIT")
            .unwrap_or_else(|_| "180".to_string())
            .parse::<u32>()
            .unwrap_or(180);

        let max_retries = std::env::var("GOOGLE_MAX_RETRIES")
            .unwrap_or_else(|_| "5".to_string())
            .parse::<u32>()
            .unwrap_or(5);

        let rate_limiter = Arc::new(RateLimiter::new(api_rate_limit, max_retries));
        let drive_client = DriveClient::with_rate_limiter(rate_limiter.clone());
        let admin_client = AdminClient::with_rate_limiter(rate_limiter);

        Ok(Self {
            pool,
            redis_client,
            drive_client,
            admin_client,
            event_queue,
        })
    }

    pub async fn sync_all_sources(&self) -> Result<()> {
        let sources = self.get_active_sources().await?;

        info!("Found {} active Google Drive sources", sources.len());

        for source in sources {
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
        let sources = sqlx::query_as::<_, Source>(
            "SELECT s.* FROM sources s
             INNER JOIN service_credentials sc ON s.id = sc.source_id
             WHERE s.source_type = $1 
             AND s.is_active = true 
             AND sc.provider = 'google'",
        )
        .bind(SourceType::GoogleDrive)
        .fetch_all(&self.pool)
        .await?;

        Ok(sources)
    }

    async fn sync_source(&self, source: &Source) -> Result<()> {
        info!("Syncing source: {} ({})", source.name, source.id);

        // Create a sync run record
        let sync_run_id = self.create_sync_run(&source.id, SyncType::Full).await?;

        let result = self.sync_source_internal(source, &sync_run_id).await;

        // Update sync run based on result
        match &result {
            Ok((files_processed, files_updated)) => {
                self.update_sync_run_completed(
                    &sync_run_id,
                    *files_processed as i32,
                    *files_updated as i32,
                )
                .await?;
            }
            Err(e) => {
                self.update_sync_run_failed(&sync_run_id, &e.to_string())
                    .await?;
            }
        }

        result.map(|_| ())
    }

    async fn sync_source_internal(
        &self,
        source: &Source,
        sync_run_id: &str,
    ) -> Result<(usize, usize)> {
        let service_creds = self.get_service_credentials(&source.id).await?;
        let service_auth = Arc::new(self.create_service_auth(&service_creds)?);
        let domain = self.get_domain_from_credentials(&service_creds)?;
        let user_email = self.get_user_email_from_source(&source.id).await
            .map_err(|e| anyhow::anyhow!("Failed to get user email for source {}: {}. Make sure the source has a valid creator.", source.id, e))?;

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

        let sync_state = SyncState::new(self.redis_client.clone());
        let synced_files = sync_state.get_all_synced_file_ids(&source.id).await?;
        let mut current_files = HashSet::new();

        // Create maps directly during file collection
        let mut access_tokens_by_user: HashMap<String, String> = HashMap::new();
        let mut files_by_user: HashMap<String, Vec<crate::models::GoogleDriveFile>> =
            HashMap::new();

        // Iterate over all users to collect files
        let mut user_count = 0;
        let total_users = all_users.len();

        for user in &all_users {
            user_count += 1;
            info!(
                "Processing files for user {} ({}/{})",
                user.primary_email, user_count, total_users
            );

            // Get access token for this user
            let user_access_token = match service_auth.get_access_token(&user.primary_email).await {
                Ok(token) => token,
                Err(e) => {
                    warn!("Failed to get access token for user {}: {}. This user may not have Drive access.", user.primary_email, e);
                    continue;
                }
            };

            let mut page_token: Option<String> = None;
            let mut user_file_count = 0;
            let mut user_files_to_process = 0;

            // List all files accessible to this user
            loop {
                debug!(
                    "Calling Drive API list_files for user {} with page_token: {:?}",
                    user.primary_email, page_token
                );

                let response = match self
                    .drive_client
                    .list_files(&user_access_token, page_token.as_deref())
                    .await
                    .with_context(|| {
                        format!(
                            "Failed to list files for user {} (page_token: {:?})",
                            user.primary_email, page_token
                        )
                    }) {
                    Ok(resp) => resp,
                    Err(e) => {
                        warn!(
                            "Failed to list files for user {}: {:?}",
                            user.primary_email, e
                        );
                        break;
                    }
                };

                debug!(
                    "Got {} files in this page for user {}",
                    response.files.len(),
                    user.primary_email
                );

                for file in response.files {
                    user_file_count += 1;
                    current_files.insert(file.id.clone());

                    if self.should_index_file(&file) {
                        let should_process = if let Some(modified_time) = &file.modified_time {
                            match sync_state.get_file_sync_state(&source.id, &file.id).await? {
                                Some(last_modified) => {
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
                                None => {
                                    debug!("File {} is new, processing", file.name);
                                    true
                                }
                            }
                        } else {
                            warn!("File {} has no modified_time, processing anyway", file.name);
                            true
                        };

                        if should_process {
                            user_files_to_process += 1;
                            // Store access token once per user and add file to user's list
                            access_tokens_by_user
                                .entry(user.primary_email.clone())
                                .or_insert_with(|| user_access_token.clone());
                            files_by_user
                                .entry(user.primary_email.clone())
                                .or_insert_with(Vec::new)
                                .push(file);
                        }
                    }
                }

                page_token = response.next_page_token;
                if page_token.is_none() {
                    debug!("No more pages for user {}", user.primary_email);
                    break;
                }
                debug!("Moving to next page for user {}", user.primary_email);
            }

            info!(
                "User {} complete: {} total files, {} to process",
                user.primary_email, user_file_count, user_files_to_process
            );
        }

        let processed_count = current_files.len();
        let total_files_to_process = files_by_user
            .values()
            .map(|files| files.len())
            .sum::<usize>();
        info!(
            "File collection complete: {} total files across all users, {} files to process",
            current_files.len(),
            total_files_to_process
        );

        let processed_count_tracker = Arc::new(AtomicUsize::new(0));
        let success_count_tracker = Arc::new(AtomicUsize::new(0));
        let failed_count_tracker = Arc::new(AtomicUsize::new(0));
        let total_to_process = total_files_to_process;
        let last_progress_time = Arc::new(std::sync::Mutex::new(Instant::now()));

        // Spawn a task to monitor progress
        let monitor_processed = Arc::clone(&processed_count_tracker);
        let monitor_success = Arc::clone(&success_count_tracker);
        let monitor_failed = Arc::clone(&failed_count_tracker);
        let monitor_last_progress = Arc::clone(&last_progress_time);
        let monitor_handle = tokio::spawn(async move {
            let mut ticker = interval(Duration::from_secs(30));
            loop {
                ticker.tick().await;
                let current = monitor_processed.load(Ordering::SeqCst);
                let success = monitor_success.load(Ordering::SeqCst);
                let failed = monitor_failed.load(Ordering::SeqCst);

                let last_update = monitor_last_progress.lock().unwrap();
                let time_since_progress = last_update.elapsed();
                drop(last_update);

                if time_since_progress > Duration::from_secs(120) {
                    warn!(
                        "No progress in {} seconds! Stuck at {}/{} files (success: {}, failed: {})",
                        time_since_progress.as_secs(),
                        current,
                        total_to_process,
                        success,
                        failed
                    );
                } else {
                    debug!(
                        "Progress heartbeat: {}/{} files (success: {}, failed: {})",
                        current, total_to_process, success, failed
                    );
                }
            }
        });

        info!(
            "Starting concurrent file processing for {} users",
            files_by_user.len()
        );

        // Create global semaphore to limit concurrent requests across all users
        // Google Drive API limit: 3000 requests per minute per project = 50 req/sec
        let max_concurrent_requests = std::env::var("GOOGLE_MAX_CONCURRENT_REQUESTS")
            .unwrap_or_else(|_| "50".to_string())
            .parse::<usize>()
            .unwrap_or(50);
        let global_semaphore = Arc::new(Semaphore::new(max_concurrent_requests));

        info!(
            "Using global semaphore with {} concurrent request limit",
            max_concurrent_requests
        );

        // Process files concurrently with per-user rate limiting
        let user_streams = files_by_user.into_iter().map(|(user_email, user_files)| {
            let user_access_token = access_tokens_by_user.get(&user_email).unwrap().clone();
            let sync_state = sync_state.clone();
            let sync_run_id = sync_run_id.to_string();
            let source_id = source.id.clone();
            let drive_client = self.drive_client.clone();
            let event_queue = self.event_queue.clone();
            let processed_count = Arc::clone(&processed_count_tracker);
            let success_count = Arc::clone(&success_count_tracker);
            let failed_count = Arc::clone(&failed_count_tracker);
            let progress_time = Arc::clone(&last_progress_time);
            let global_semaphore = Arc::clone(&global_semaphore);
            let user_file_count = user_files.len();

            // Create per-user rate limiter for 5 requests per second (300 per minute per user limit)
            // The rate limiter handles both rate limiting and retries on 429/503 errors
            let user_rate_limiter = Arc::new(RateLimiter::new(5, 3)); // 5 req/sec, 3 retries

            info!("Processing {} files for user {} with rate limiting (5 req/sec)", user_file_count, user_email);

            stream::iter(user_files)
                .map(move |file| {
                    let sync_state = sync_state.clone();
                    let sync_run_id = sync_run_id.clone();
                    let source_id = source_id.clone();
                    let processed_count = processed_count.clone();
                    let success_count = success_count.clone();
                    let failed_count = failed_count.clone();
                    let progress_time = progress_time.clone();
                    let rate_limiter = user_rate_limiter.clone();
                    let user_email = user_email.clone();
                    let drive_client = drive_client.clone();
                    let event_queue = event_queue.clone();
                    let user_access_token = user_access_token.clone();
                    let global_semaphore = global_semaphore.clone();

                    async move {
                        // Acquire global semaphore permit first to limit total concurrent requests
                        let _global_permit = global_semaphore.acquire().await.expect("Global semaphore should not be closed");

                        let file_name = file.name.clone();
                        let file_id = file.id.clone();
                        debug!("Starting to process file: {} ({}) for user {}", file_name, file_id, user_email);

                        // Use rate limiter's execute_with_retry for proper error handling and retries
                        let result = match rate_limiter.execute_with_retry(|| {
                            let drive_client = drive_client.clone();
                            let user_access_token = user_access_token.clone();
                            let file = file.clone();
                            async move {
                                drive_client
                                    .get_file_content(&user_access_token, &file)
                                    .await
                                    .with_context(|| {
                                        format!("Getting content for file {} ({})", file.name, file.id)
                                    })
                            }
                        }).await {
                            Ok(content) => {
                                if !content.is_empty() {
                                    let event = file.clone().to_connector_event(
                                        sync_run_id.clone(),
                                        source_id.clone(),
                                        content,
                                    );

                                    match event_queue.enqueue(&source_id, &event).await.with_context(
                                        || {
                                            format!(
                                                "Enqueueing event for file {} ({})",
                                                file.name, file.id
                                            )
                                        },
                                    ) {
                                        Ok(_) => {
                                            if let Some(modified_time) = &file.modified_time {
                                                if let Err(e) = sync_state
                                                    .set_file_sync_state(
                                                        &source_id,
                                                        &file.id,
                                                        modified_time,
                                                    )
                                                    .await
                                                    .with_context(|| {
                                                        format!(
                                                            "Updating sync state for file {} ({})",
                                                            file.name, file.id
                                                        )
                                                    })
                                                {
                                                    error!(
                                                        "Failed to update sync state for file {}: {:?}",
                                                        file.name, e
                                                    );
                                                    failed_count.fetch_add(1, Ordering::SeqCst);
                                                    return None;
                                                }
                                            }
                                            debug!(
                                                "Successfully processed file: {} ({})",
                                                file_name, file_id
                                            );
                                            success_count.fetch_add(1, Ordering::SeqCst);
                                            Some(())
                                        }
                                        Err(e) => {
                                            error!(
                                                "Failed to queue event for file {}: {:?}",
                                                file.name, e
                                            );
                                            failed_count.fetch_add(1, Ordering::SeqCst);
                                            None
                                        }
                                    }
                                } else {
                                    debug!("File {} has empty content, skipping", file.name);
                                    None
                                }
                            }
                            Err(e) => {
                                warn!(
                                    "Failed to get content for file {} ({}): {:?}",
                                    file.name, file.id, e
                                );
                                failed_count.fetch_add(1, Ordering::SeqCst);
                                None
                            }
                        };

                        // Update progress counters
                        let current_processed = processed_count.fetch_add(1, Ordering::SeqCst) + 1;

                        // Update last progress time
                        *progress_time.lock().unwrap() = Instant::now();

                        // Log progress every 100 files or at end
                        if current_processed % 100 == 0 || current_processed == total_to_process {
                            let current_success = success_count.load(Ordering::SeqCst);
                            let current_failed = failed_count.load(Ordering::SeqCst);
                            info!(
                                "Progress: {}/{} files processed ({:.1}%) - {} successful, {} failed",
                                current_processed,
                                total_to_process,
                                (current_processed as f64 / total_to_process as f64) * 100.0,
                                current_success,
                                current_failed
                            );
                        }

                        result
                    }
                })
                .buffer_unordered(20) // Allow more tasks to queue up, rate limiter will control actual rate
                .collect::<Vec<_>>()
        });

        // Process all user streams concurrently
        let all_results: Vec<Vec<Option<()>>> = futures::future::join_all(user_streams).await;
        let results: Vec<Option<()>> = all_results.into_iter().flatten().collect();

        // Stop the monitor task
        monitor_handle.abort();

        let updated_count = results.iter().filter(|r| r.is_some()).count();

        info!(
            "File processing complete. Total: {}, Success: {}, Failed: {}",
            processed_count_tracker.load(Ordering::SeqCst),
            success_count_tracker.load(Ordering::SeqCst),
            failed_count_tracker.load(Ordering::SeqCst)
        );

        // Handle deletions
        for deleted_file_id in synced_files.difference(&current_files) {
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

        info!(
            "Sync completed for source {}: {} files processed, {} updated",
            source.id, processed_count, updated_count
        );

        self.update_source_status(&source.id, "completed").await?;

        info!("Completed sync for source: {}", source.id);
        Ok((processed_count, updated_count))
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
        let creds = sqlx::query_as::<_, ServiceCredentials>(
            "SELECT * FROM service_credentials 
             WHERE source_id = $1 AND provider = 'google'",
        )
        .bind(source_id)
        .fetch_one(&self.pool)
        .await?;

        Ok(creds)
    }

    fn create_service_auth(&self, creds: &ServiceCredentials) -> Result<ServiceAccountAuth> {
        let service_account_json = creds
            .credentials
            .get("service_account_key")
            .and_then(|v| v.as_str())
            .ok_or_else(|| anyhow::anyhow!("Missing service_account_key in credentials"))?;

        let default_scopes = vec![
            "https://www.googleapis.com/auth/drive.readonly".to_string(),
            "https://www.googleapis.com/auth/gmail.readonly".to_string(),
            "https://www.googleapis.com/auth/admin.directory.user.readonly".to_string(),
        ];

        let scopes = creds
            .config
            .get("scopes")
            .and_then(|v| v.as_array())
            .map(|arr| {
                arr.iter()
                    .filter_map(|v| v.as_str().map(String::from))
                    .collect::<Vec<_>>()
            })
            .unwrap_or(default_scopes);

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
        sqlx::query(
            "UPDATE sources SET sync_status = $1, last_sync_at = CURRENT_TIMESTAMP, updated_at = CURRENT_TIMESTAMP WHERE id = $2"
        )
        .bind(status)
        .bind(source_id)
        .execute(&self.pool)
        .await?;

        Ok(())
    }

    pub async fn sync_source_by_id(&self, source_id: String) -> Result<()> {
        info!("Manually triggered sync for source: {}", source_id);

        let source =
            sqlx::query_as::<_, Source>("SELECT * FROM sources WHERE id = $1 AND source_type = $2")
                .bind(&source_id)
                .bind(SourceType::GoogleDrive)
                .fetch_optional(&self.pool)
                .await?;

        match source {
            Some(source) => {
                if !source.is_active {
                    return Err(anyhow::anyhow!("Source {} is not active", source_id));
                }
                // Manual sync always runs regardless of last sync time
                self.sync_source(&source).await
            }
            None => Err(anyhow::anyhow!("Source {} not found", source_id)),
        }
    }

    async fn get_last_completed_full_sync(&self, source_id: &str) -> Result<Option<SyncRun>> {
        let sync_run = sqlx::query_as::<_, SyncRun>(
            "SELECT * FROM sync_runs 
             WHERE source_id = $1 
             AND sync_type = $2 
             AND status = $3 
             ORDER BY completed_at DESC 
             LIMIT 1",
        )
        .bind(source_id)
        .bind(SyncType::Full)
        .bind(SyncStatus::Completed)
        .fetch_optional(&self.pool)
        .await?;

        Ok(sync_run)
    }

    async fn create_sync_run(&self, source_id: &str, sync_type: SyncType) -> Result<String> {
        let id = generate_ulid();

        sqlx::query(
            "INSERT INTO sync_runs (id, source_id, sync_type, status) 
             VALUES ($1, $2, $3, $4)",
        )
        .bind(&id)
        .bind(source_id)
        .bind(sync_type)
        .bind(SyncStatus::Running)
        .execute(&self.pool)
        .await?;

        Ok(id)
    }

    async fn update_sync_run_completed(
        &self,
        sync_run_id: &str,
        files_processed: i32,
        files_updated: i32,
    ) -> Result<()> {
        sqlx::query(
            "UPDATE sync_runs 
             SET status = $1, completed_at = CURRENT_TIMESTAMP, 
                 documents_processed = $2, documents_updated = $3,
                 updated_at = CURRENT_TIMESTAMP
             WHERE id = $4",
        )
        .bind(SyncStatus::Completed)
        .bind(files_processed)
        .bind(files_updated)
        .bind(sync_run_id)
        .execute(&self.pool)
        .await?;

        // Notify listeners about sync run completion
        sqlx::query("NOTIFY sync_run_update")
            .execute(&self.pool)
            .await?;

        Ok(())
    }

    async fn update_sync_run_failed(&self, sync_run_id: &str, error: &str) -> Result<()> {
        sqlx::query(
            "UPDATE sync_runs 
             SET status = $1, completed_at = CURRENT_TIMESTAMP, 
                 error_message = $2, updated_at = CURRENT_TIMESTAMP
             WHERE id = $3",
        )
        .bind(SyncStatus::Failed)
        .bind(error)
        .bind(sync_run_id)
        .execute(&self.pool)
        .await?;

        // Notify listeners about sync run failure
        sqlx::query("NOTIFY sync_run_update")
            .execute(&self.pool)
            .await?;

        Ok(())
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
                if let Some(source) = self.get_source_by_id(&webhook_channel.source_id).await? {
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
        let service_auth = self.create_service_auth(&service_creds)?;
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
        let service_auth = self.create_service_auth(&service_creds)?;
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
        let service_auth = self.create_service_auth(&service_creds)?;
        let user_email = self.get_user_email_from_source(&source.id).await
            .map_err(|e| anyhow::anyhow!("Failed to get user email for source {}: {}. Make sure the source has a valid creator.", source.id, e))?;
        let access_token = service_auth.get_access_token(&user_email).await?;

        // For incremental sync, we would ideally use the changes API with a stored page token
        // For now, we'll just get the latest changes using the current start page token
        let start_page_token = self
            .drive_client
            .get_start_page_token(&access_token)
            .await?;

        // Create a sync run record for incremental sync
        let sync_run_id = self
            .create_sync_run(&source.id, SyncType::Incremental)
            .await?;

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
                                self.publish_deletion_event(&sync_run_id, &source.id, file_id)
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
                                        .get_file_content(&access_token, &file)
                                        .await
                                    {
                                        Ok(content) => {
                                            if !content.is_empty() {
                                                let event = file.clone().to_connector_event(
                                                    sync_run_id.clone(),
                                                    source.id.clone(),
                                                    content,
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

                self.update_sync_run_completed(
                    &sync_run_id,
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
                self.update_sync_run_failed(&sync_run_id, &e.to_string())
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

    async fn get_source_by_id(&self, source_id: &str) -> Result<Option<Source>> {
        let source = sqlx::query_as::<_, Source>("SELECT * FROM sources WHERE id = $1")
            .bind(source_id)
            .fetch_optional(&self.pool)
            .await?;

        Ok(source)
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
            "Found {} active Google Drive sources for webhook registration",
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
}

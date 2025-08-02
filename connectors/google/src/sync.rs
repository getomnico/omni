use anyhow::{Context, Result};
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
use crate::models::{UserFile, WebhookChannel, WebhookChannelResponse, WebhookNotification};
use shared::db::repositories::ServiceCredentialsRepo;
use shared::models::{
    ConnectorEvent, ServiceCredentials, ServiceProvider, Source, SourceType, SyncRun, SyncStatus,
    SyncType, WebhookChannel as DatabaseWebhookChannel,
};
use shared::queue::EventQueue;
use shared::utils::generate_ulid;
use shared::{ContentStorage, RateLimiter};

pub struct SyncManager {
    pool: PgPool,
    redis_client: RedisClient,
    drive_client: DriveClient,
    admin_client: AdminClient,
    event_queue: EventQueue,
    content_storage: ContentStorage,
    service_credentials_repo: ServiceCredentialsRepo,
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
        let service_credentials_repo = ServiceCredentialsRepo::new(pool.clone())?;

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

        let content_storage = ContentStorage::new(pool.clone());

        Ok(Self {
            pool,
            redis_client,
            drive_client,
            admin_client,
            event_queue,
            content_storage,
            service_credentials_repo,
            folder_cache: LruFolderCache::new(10_000), // Cache up to 10,000 folder metadata entries
        })
    }

    pub async fn sync_all_sources(&self) -> Result<()> {
        let sources = self.get_active_sources().await?;

        info!("Found {} active Google Drive sources", sources.len());

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

        // Double-check for running sync to prevent race conditions
        if let Some(running_sync) = self.get_running_sync_for_source(&source.id).await? {
            warn!(
                "Found running sync for source {} (id: {}) just before creating new sync, aborting",
                source.id, running_sync.id
            );
            return Ok(());
        }

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

    async fn sync_source_for_user(
        &self,
        user_email: &str,
        service_auth: Arc<ServiceAccountAuth>,
        source_id: &str,
        sync_run_id: &str,
        sync_state: &SyncState,
        current_files: Arc<std::sync::Mutex<HashSet<String>>>,
        rate_limiter: Arc<RateLimiter>,
    ) -> Result<(usize, usize)> {
        info!("Processing files for user: {}", user_email);

        let mut total_processed = 0;
        let mut total_updated = 0;
        let mut page_token: Option<String> = None;
        let mut file_batch = Vec::new();
        const BATCH_SIZE: usize = 200;

        loop {
            debug!(
                "Listing files for user {} with page_token: {:?}",
                user_email, page_token
            );

            // Use rate limiter for listing files
            let response = rate_limiter
                .execute_with_retry(|| {
                    let drive_client = self.drive_client.clone();
                    let service_auth = service_auth.clone();
                    let user_email = user_email.to_string();
                    let page_token = page_token.clone();

                    async move {
                        drive_client
                            .list_files(&service_auth, &user_email, page_token.as_deref())
                            .await
                            .with_context(|| {
                                format!(
                                    "Failed to list files for user {} (page_token: {:?})",
                                    user_email, page_token
                                )
                            })
                    }
                })
                .await?;

            debug!(
                "Got {} files in this page for user {}",
                response.files.len(),
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
                                    sync_run_id,
                                    sync_state,
                                    service_auth.clone(),
                                    rate_limiter.clone(),
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
                    sync_run_id,
                    sync_state,
                    service_auth.clone(),
                    rate_limiter.clone(),
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
        sync_run_id: &str,
        sync_state: &SyncState,
        service_auth: Arc<ServiceAccountAuth>,
        rate_limiter: Arc<RateLimiter>,
    ) -> Result<(usize, usize)> {
        info!("Processing batch of {} files", files.len());

        let mut processed = 0;
        let mut updated = 0;

        // Process files concurrently within the batch
        let tasks = files.into_iter().map(|user_file| {
            let service_auth = service_auth.clone();
            let source_id = source_id.to_string();
            let sync_run_id = sync_run_id.to_string();
            let sync_state = sync_state.clone();
            let rate_limiter = rate_limiter.clone();
            let drive_client = self.drive_client.clone();
            let event_queue = self.event_queue.clone();
            let content_storage = self.content_storage.clone();

            async move {
                debug!("Processing file: {} ({}) for user: {}", user_file.file.name, user_file.file.id, user_file.user_email);

                // Use rate limiter for file content download
                let result = rate_limiter.execute_with_retry(|| {
                    let drive_client = drive_client.clone();
                    let service_auth = service_auth.clone();
                    let user_file = user_file.clone();
                    async move {
                        drive_client
                            .get_file_content(&service_auth, &user_file.user_email, &user_file.file)
                            .await
                            .with_context(|| format!("Getting content for file {} ({})", user_file.file.name, user_file.file.id))
                    }
                }).await;

                match result {
                    Ok(content) => {
                        if !content.is_empty() {
                            match content_storage.store_text(content).await {
                                Ok(content_id) => {
                                    // Resolve the full path for this file
                                    let file_path = match self
                                        .resolve_file_path(
                                            &service_auth,
                                            &user_file.user_email,
                                            &user_file.file,
                                            rate_limiter.clone(),
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
        let current_files = Arc::new(std::sync::Mutex::new(HashSet::new()));

        // Use single global rate limiter (200 req/sec) for ALL Google API calls
        let global_rate_limiter = Arc::new(RateLimiter::new(200, 5));
        info!("Using single global rate limiter at 200 req/sec for all API calls");

        info!(
            "Starting sequential user processing for {} users",
            all_users.len()
        );

        let mut total_processed = 0;
        let mut total_updated = 0;

        for user in all_users {
            let user_email = user.primary_email.clone();

            // Get access token for this user
            match service_auth.get_access_token(&user_email).await {
                Ok(_token) => {
                    info!("Processing user: {}", user_email);
                    match self
                        .sync_source_for_user(
                            &user_email,
                            service_auth.clone(),
                            &source.id,
                            sync_run_id,
                            &sync_state,
                            current_files.clone(),
                            global_rate_limiter.clone(),
                        )
                        .await
                    {
                        Ok((processed, updated)) => {
                            total_processed += processed;
                            total_updated += updated;
                            info!(
                                "User {} completed: {} processed, {} updated",
                                user_email, processed, updated
                            );
                        }
                        Err(e) => {
                            error!("Failed to process user {}: {}", user_email, e);
                        }
                    }
                }
                Err(e) => {
                    warn!("Failed to get access token for user {}: {}. This user may not have Drive access.", user_email, e);
                }
            }
        }

        info!(
            "User processing complete. Total: {} processed, {} updated",
            total_processed, total_updated
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
            self.publish_deletion_event(sync_run_id, &source.id, deleted_file_id)
                .await?;
            sync_state
                .delete_file_sync_state(&source.id, deleted_file_id)
                .await?;
        }

        info!(
            "Sync completed for source {}: {} files discovered, {} processed",
            source.id,
            current_files_set.len(),
            total_processed
        );

        self.update_source_status(&source.id, "completed").await?;

        // Clear folder cache to free memory after sync
        self.folder_cache.clear();

        info!("Completed sync for source: {}", source.id);
        Ok((current_files_set.len(), total_processed))
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

        // Create rate limiter for incremental sync API calls
        let rate_limiter = Arc::new(RateLimiter::new(5, 3)); // 5 req/sec, 3 retries

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
                                                        rate_limiter.clone(),
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
                                                    .store_text(content)
                                                    .await
                                                {
                                                    Ok(oid) => oid,
                                                    Err(e) => {
                                                        error!("Failed to store content in LOB storage for file {}: {}", file.name, e);
                                                        continue;
                                                    }
                                                };

                                                let event = file.to_connector_event(
                                                    &sync_run_id,
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

    pub async fn get_running_sync_for_source(&self, source_id: &str) -> Result<Option<SyncRun>> {
        let running_sync = sqlx::query_as::<_, SyncRun>(
            "SELECT * FROM sync_runs
             WHERE source_id = $1
             AND status = $2
             ORDER BY started_at DESC
             LIMIT 1",
        )
        .bind(source_id)
        .bind(SyncStatus::Running)
        .fetch_optional(&self.pool)
        .await?;

        Ok(running_sync)
    }

    pub async fn recover_interrupted_syncs(&self) -> Result<()> {
        info!("Checking for interrupted running syncs from previous connector instance");

        let running_syncs = sqlx::query_as::<_, SyncRun>(
            "SELECT * FROM sync_runs WHERE status = $1 ORDER BY started_at ASC",
        )
        .bind(SyncStatus::Running)
        .fetch_all(&self.pool)
        .await?;

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

            if let Err(e) = sqlx::query(
                "UPDATE sync_runs
                 SET status = $1, completed_at = CURRENT_TIMESTAMP,
                     error_message = $2, updated_at = CURRENT_TIMESTAMP
                 WHERE id = $3",
            )
            .bind(SyncStatus::Failed)
            .bind(error_message)
            .bind(&sync_run.id)
            .execute(&self.pool)
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
        rate_limiter: Arc<RateLimiter>,
    ) -> Result<String> {
        if let Some(parents) = &file.parents {
            if let Some(parent_id) = parents.first() {
                return self
                    .build_full_path(auth, user_email, parent_id, &file.name, rate_limiter)
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
        rate_limiter: Arc<RateLimiter>,
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
                    let folder_metadata = rate_limiter
                        .execute_with_retry(|| {
                            let drive_client = self.drive_client.clone();
                            let auth = auth.clone();
                            let user_email = user_email.to_string();
                            let folder_id = current_folder_id.clone();
                            async move {
                                drive_client
                                    .get_folder_metadata(&auth, &user_email, &folder_id)
                                    .await
                            }
                        })
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
                if let Some(source) = self.get_source_by_id(&sync_run.source_id).await? {
                    if source.is_active {
                        // Continue the sync using the existing sync run ID
                        let result = self.sync_source_internal(&source, &sync_run.id).await;

                        // Update sync run based on result
                        match result {
                            Ok((files_processed, files_updated)) => {
                                self.update_sync_run_completed(
                                    &sync_run.id,
                                    files_processed as i32,
                                    files_updated as i32,
                                )
                                .await?;
                                info!("Successfully continued and completed sync {}", sync_run.id);
                            }
                            Err(e) => {
                                error!("Failed to continue sync {}: {}", sync_run.id, e);
                                self.update_sync_run_failed(&sync_run.id, &e.to_string())
                                    .await?;
                                self.update_source_status(&source.id, "failed").await?;
                            }
                        }
                    } else {
                        info!("Source {} is not active, marking sync as failed", source.id);
                        self.update_sync_run_failed(&sync_run.id, "Source is not active")
                            .await?;
                    }
                } else {
                    warn!(
                        "Source {} not found for sync run {}, marking as failed",
                        sync_run.source_id, sync_run.id
                    );
                    self.update_sync_run_failed(&sync_run.id, "Source not found")
                        .await?;
                }
            }

            info!("Interrupted sync recovery completed");
        } else {
            info!("No interrupted running syncs found");
        }

        // Now check for scheduled syncs for sources without running syncs
        let sources = self.get_active_sources().await?;
        info!("Found {} active Google Drive sources", sources.len());

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

use anyhow::Result;
use futures::stream::{self, StreamExt};
use redis::{AsyncCommands, Client as RedisClient};
use sqlx::types::time::OffsetDateTime;
use sqlx::{PgPool, Row};
use std::collections::HashSet;
use std::sync::Arc;
use time;
use tokio::sync::Semaphore;
use tracing::{debug, error, info, warn};

use crate::auth::ServiceAccountAuth;
use crate::drive::DriveClient;
use crate::models::{WebhookChannel, WebhookChannelResponse, WebhookNotification};
use crate::rate_limiter::RateLimiter;
use shared::models::{
    AuthType, ConnectorEvent, ServiceCredentials, ServiceProvider, Source, SourceType, SyncRun,
    SyncStatus, SyncType, WebhookChannel as DatabaseWebhookChannel,
};
use shared::queue::EventQueue;
use shared::utils::generate_ulid;

pub struct SyncManager {
    pool: PgPool,
    redis_client: RedisClient,
    drive_client: DriveClient,
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
        let drive_client = DriveClient::with_rate_limiter(rate_limiter);

        Ok(Self {
            pool,
            redis_client,
            drive_client,
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
        let service_auth = self.create_service_auth(&service_creds)?;
        let access_token = service_auth.get_access_token().await?;

        let sync_state = SyncState::new(self.redis_client.clone());
        let synced_files = sync_state.get_all_synced_file_ids(&source.id).await?;
        let mut current_files = HashSet::new();
        let mut page_token: Option<String> = None;
        let mut all_files_to_process = Vec::new();

        // First, collect all files that need processing
        loop {
            debug!(
                "Calling Drive API list_files with page_token: {:?}",
                page_token
            );
            let response = self
                .drive_client
                .list_files(&access_token, page_token.as_deref())
                .await
                .map_err(|e| {
                    error!("Drive API list_files call failed: {}", e);
                    e
                })?;

            for file in response.files {
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
                        all_files_to_process.push(file);
                    }
                }
            }

            page_token = response.next_page_token;
            if page_token.is_none() {
                break;
            }
        }

        let processed_count = current_files.len();
        info!(
            "Found {} files to process concurrently",
            all_files_to_process.len()
        );

        // Get concurrency limit from environment
        let max_concurrent_downloads = std::env::var("GOOGLE_MAX_CONCURRENT_DOWNLOADS")
            .unwrap_or_else(|_| "10".to_string())
            .parse::<usize>()
            .unwrap_or(10);

        let semaphore = Arc::new(Semaphore::new(max_concurrent_downloads));

        // Process files concurrently
        let results = stream::iter(all_files_to_process)
            .map(|file| {
                let sync_state = sync_state.clone();
                let sync_run_id = sync_run_id.to_string();
                let source_id = source.id.clone();
                let creds = creds.clone();
                let drive_client = &self.drive_client;
                let event_queue = &self.event_queue;
                let semaphore = Arc::clone(&semaphore);

                async move {
                    let _permit = semaphore.acquire().await.unwrap();

                    match drive_client.get_file_content(&access_token, &file).await {
                        Ok(content) => {
                            if !content.is_empty() {
                                let event = file.clone().to_connector_event(
                                    sync_run_id.clone(),
                                    source_id.clone(),
                                    content,
                                );

                                match event_queue.enqueue(&source_id, &event).await {
                                    Ok(_) => {
                                        if let Some(modified_time) = &file.modified_time {
                                            if let Err(e) = sync_state
                                                .set_file_sync_state(
                                                    &source_id,
                                                    &file.id,
                                                    modified_time,
                                                )
                                                .await
                                            {
                                                error!(
                                                    "Failed to update sync state for file {}: {}",
                                                    file.name, e
                                                );
                                                return None;
                                            }
                                        }
                                        Some(())
                                    }
                                    Err(e) => {
                                        error!(
                                            "Failed to queue event for file {}: {}",
                                            file.name, e
                                        );
                                        None
                                    }
                                }
                            } else {
                                debug!("File {} has empty content, skipping", file.name);
                                None
                            }
                        }
                        Err(e) => {
                            warn!("Failed to get content for file {}: {}", file.name, e);
                            None
                        }
                    }
                }
            })
            .buffer_unordered(max_concurrent_downloads)
            .collect::<Vec<_>>()
            .await;

        let updated_count = results.iter().filter(|r| r.is_some()).count();

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
            .ok_or_else(|| anyhow!("Missing service_account_key in credentials"))?;

        let scopes = creds
            .config
            .get("scopes")
            .and_then(|v| v.as_array())
            .map(|arr| {
                arr.iter()
                    .filter_map(|v| v.as_str().map(String::from))
                    .collect()
            })
            .unwrap_or_else(|| vec!["https://www.googleapis.com/auth/drive.readonly".to_string()]);

        let delegated_user = creds
            .config
            .get("delegated_user")
            .and_then(|v| v.as_str())
            .map(String::from);

        ServiceAccountAuth::new(service_account_json, scopes, delegated_user)
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
        let access_token = service_auth.get_access_token().await?;

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
        let access_token = service_auth.get_access_token().await?;

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

    async fn sync_all_sources_incremental(&self) -> Result<()> {
        let sources = self.get_active_sources().await?;

        info!(
            "Running incremental sync for {} active Google Drive sources",
            sources.len()
        );

        for source in sources {
            if let Err(e) = self.sync_source_incremental(&source).await {
                error!(
                    "Failed to run incremental sync for source {}: {}",
                    source.id, e
                );
            }
        }

        Ok(())
    }

    async fn sync_source_incremental(&self, source: &Source) -> Result<()> {
        info!(
            "Running incremental sync for source: {} ({})",
            source.name, source.id
        );

        let service_creds = self.get_service_credentials(&source.id).await?;
        let service_auth = self.create_service_auth(&service_creds)?;
        let access_token = service_auth.get_access_token().await?;

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

use anyhow::Result;
use redis::{AsyncCommands, Client as RedisClient};
use sqlx::types::time::OffsetDateTime;
use sqlx::{PgPool, Row};
use std::collections::HashSet;
use tracing::{debug, error, info, warn};

use crate::auth::{AuthManager, OAuthCredentials};
use crate::drive::DriveClient;
use shared::models::{ConnectorEvent, Source, SourceType, SyncRun, SyncStatus, SyncType};
use shared::queue::EventQueue;
use shared::utils::generate_ulid;

pub struct SyncManager {
    pool: PgPool,
    redis_client: RedisClient,
    auth_manager: AuthManager,
    drive_client: DriveClient,
    event_queue: EventQueue,
}

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
        let client_id = std::env::var("GOOGLE_CLIENT_ID")?;
        let client_secret = std::env::var("GOOGLE_CLIENT_SECRET")?;
        let event_queue = EventQueue::new(pool.clone());

        Ok(Self {
            pool,
            redis_client,
            auth_manager: AuthManager::new(client_id, client_secret),
            drive_client: DriveClient::new(),
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
             INNER JOIN oauth_credentials oc ON s.id = oc.source_id
             WHERE s.source_type = $1 
             AND s.is_active = true 
             AND oc.provider = 'google'",
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
        let oauth_creds = self.get_oauth_credentials(&source.id).await?;
        let mut creds: OAuthCredentials = oauth_creds;

        let original_creds = creds.clone();
        self.auth_manager.ensure_valid_token(&mut creds).await?;

        if creds.access_token != original_creds.access_token
            || creds.refresh_token != original_creds.refresh_token
        {
            self.update_oauth_credentials(&source.id, &creds).await?;
        }

        let sync_state = SyncState::new(self.redis_client.clone());
        let synced_files = sync_state.get_all_synced_file_ids(&source.id).await?;
        let mut current_files = HashSet::new();
        let mut page_token: Option<String> = None;
        let mut processed_count = 0;
        let mut updated_count = 0;

        loop {
            debug!(
                "Calling Drive API list_files with page_token: {:?}",
                page_token
            );
            let response = self
                .drive_client
                .list_files(&creds.access_token, page_token.as_deref())
                .await
                .map_err(|e| {
                    error!("Drive API list_files call failed: {}", e);
                    e
                })?;

            for file in response.files {
                current_files.insert(file.id.clone());
                processed_count += 1;

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
                        match self
                            .drive_client
                            .get_file_content(&creds.access_token, &file)
                            .await
                        {
                            Ok(content) => {
                                if !content.is_empty() {
                                    let event = file.clone().to_connector_event(
                                        sync_run_id.to_string(),
                                        source.id.clone(),
                                        content,
                                    );

                                    // Only update sync state if event was successfully queued
                                    match self.publish_connector_event(event).await {
                                        Ok(_) => {
                                            updated_count += 1;
                                            if let Some(modified_time) = &file.modified_time {
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
                                warn!("Failed to get content for file {}: {}", file.name, e);
                            }
                        }
                    }
                }
            }

            page_token = response.next_page_token;
            if page_token.is_none() {
                break;
            }
        }

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

    async fn get_oauth_credentials(&self, source_id: &str) -> Result<OAuthCredentials> {
        let row = sqlx::query(
            "SELECT access_token, refresh_token, token_type, expires_at 
             FROM oauth_credentials 
             WHERE source_id = $1 AND provider = 'google'",
        )
        .bind(source_id)
        .fetch_one(&self.pool)
        .await?;

        let expires_at: chrono::DateTime<chrono::Utc> = row.get("expires_at");
        let now = chrono::Utc::now();
        let expires_in = (expires_at - now).num_seconds().max(0);

        Ok(OAuthCredentials {
            access_token: row.get("access_token"),
            refresh_token: row.get("refresh_token"),
            token_type: row.get("token_type"),
            expires_in,
            obtained_at: now.timestamp_millis(),
        })
    }

    async fn update_oauth_credentials(
        &self,
        source_id: &str,
        creds: &OAuthCredentials,
    ) -> Result<()> {
        let expires_at = chrono::DateTime::from_timestamp(creds.obtained_at / 1000, 0)
            .unwrap_or(chrono::Utc::now())
            + chrono::Duration::seconds(creds.expires_in);

        sqlx::query(
            "UPDATE oauth_credentials 
             SET access_token = $1, refresh_token = $2, token_type = $3, expires_at = $4, updated_at = CURRENT_TIMESTAMP 
             WHERE source_id = $5 AND provider = 'google'"
        )
        .bind(&creds.access_token)
        .bind(&creds.refresh_token)
        .bind(&creds.token_type)
        .bind(&expires_at)
        .bind(source_id)
        .execute(&self.pool)
        .await?;

        Ok(())
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
                 files_processed = $2, files_updated = $3,
                 updated_at = CURRENT_TIMESTAMP
             WHERE id = $4",
        )
        .bind(SyncStatus::Completed)
        .bind(files_processed)
        .bind(files_updated)
        .bind(sync_run_id)
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
}

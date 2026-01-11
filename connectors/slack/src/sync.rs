use anyhow::{anyhow, Context, Result};
use dashmap::DashMap;
use redis::{AsyncCommands, Client as RedisClient};
use shared::models::{ServiceProvider, SourceType, SyncRequest};
use std::collections::HashSet;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use tracing::{debug, error, info, warn};

use crate::auth::AuthManager;
use crate::client::SlackClient;
use crate::content::ContentProcessor;
use shared::SdkClient;

struct ActiveSync {
    cancelled: AtomicBool,
}

pub struct SyncManager {
    redis_client: RedisClient,
    auth_manager: AuthManager,
    slack_client: SlackClient,
    sdk_client: SdkClient,
    active_syncs: DashMap<String, Arc<ActiveSync>>,
}

pub struct SyncState {
    redis_client: RedisClient,
}

impl SyncState {
    pub fn new(redis_client: RedisClient) -> Self {
        Self { redis_client }
    }

    pub fn get_channel_sync_key(&self, source_id: &str, channel_id: &str) -> String {
        format!("slack:sync:{}:{}", source_id, channel_id)
    }

    pub fn get_test_channel_sync_key(&self, source_id: &str, channel_id: &str) -> String {
        format!("slack:sync:test:{}:{}", source_id, channel_id)
    }

    pub async fn get_channel_last_ts(
        &self,
        source_id: &str,
        channel_id: &str,
    ) -> Result<Option<String>> {
        let mut conn = self.redis_client.get_multiplexed_async_connection().await?;
        let key = if cfg!(test) {
            self.get_test_channel_sync_key(source_id, channel_id)
        } else {
            self.get_channel_sync_key(source_id, channel_id)
        };

        let result: Option<String> = conn.get(&key).await?;
        Ok(result)
    }

    pub async fn set_channel_last_ts(
        &self,
        source_id: &str,
        channel_id: &str,
        last_ts: &str,
    ) -> Result<()> {
        let mut conn = self.redis_client.get_multiplexed_async_connection().await?;
        let key = if cfg!(test) {
            self.get_test_channel_sync_key(source_id, channel_id)
        } else {
            self.get_channel_sync_key(source_id, channel_id)
        };

        let _: () = conn.set_ex(&key, last_ts, 30 * 24 * 60 * 60).await?; // 30 days expiry
        Ok(())
    }

    pub async fn get_all_synced_channels(&self, source_id: &str) -> Result<HashSet<String>> {
        let mut conn = self.redis_client.get_multiplexed_async_connection().await?;
        let pattern = if cfg!(test) {
            format!("slack:sync:test:{}:*", source_id)
        } else {
            format!("slack:sync:{}:*", source_id)
        };

        let keys: Vec<String> = conn.keys(&pattern).await?;
        let prefix = if cfg!(test) {
            format!("slack:sync:test:{}:", source_id)
        } else {
            format!("slack:sync:{}:", source_id)
        };

        let channel_ids: HashSet<String> = keys
            .into_iter()
            .filter_map(|key| key.strip_prefix(&prefix).map(|s| s.to_string()))
            .collect();

        Ok(channel_ids)
    }
}

impl SyncManager {
    pub fn new(redis_client: RedisClient, sdk_client: SdkClient) -> Self {
        Self {
            redis_client,
            auth_manager: AuthManager::new(),
            slack_client: SlackClient::new(),
            sdk_client,
            active_syncs: DashMap::new(),
        }
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

    /// Execute a sync based on the request from connector-manager
    pub async fn sync_source_from_request(&self, request: SyncRequest) -> Result<()> {
        let sync_run_id = &request.sync_run_id;
        let source_id = &request.source_id;

        info!(
            "Starting sync for source: {} (sync_run_id: {})",
            source_id, sync_run_id
        );

        // Register this sync for cancellation tracking
        let active_sync = Arc::new(ActiveSync {
            cancelled: AtomicBool::new(false),
        });
        self.active_syncs
            .insert(sync_run_id.to_string(), active_sync.clone());

        // Fetch source via SDK
        let source = self
            .sdk_client
            .get_source(source_id)
            .await
            .context("Failed to fetch source via SDK")?;

        if !source.is_active {
            let err_msg = format!("Source is not active: {}", source_id);
            self.sdk_client.fail(sync_run_id, &err_msg).await?;
            self.active_syncs.remove(sync_run_id);
            return Err(anyhow!(err_msg));
        }

        if source.source_type != SourceType::Slack {
            let err_msg = format!(
                "Invalid source type for Slack connector: {:?}",
                source.source_type
            );
            self.sdk_client.fail(sync_run_id, &err_msg).await?;
            self.active_syncs.remove(sync_run_id);
            return Err(anyhow!(err_msg));
        }

        let result: Result<(usize, usize, usize)> = async {
            let bot_token = self.get_bot_token(source_id).await?;
            let mut creds = self.auth_manager.validate_bot_token(&bot_token).await?;

            self.auth_manager
                .ensure_valid_credentials(&mut creds)
                .await?;

            let sync_state = SyncState::new(self.redis_client.clone());
            let mut content_processor = ContentProcessor::new();

            // First, fetch all users for name resolution
            self.fetch_all_users(&creds.bot_token, &mut content_processor)
                .await?;

            // Get all accessible channels
            let channels = self.fetch_all_channels(&creds.bot_token).await?;

            // Track sync progress
            let mut processed_channels = 0;
            let mut total_message_groups = 0;
            let mut total_files = 0;

            for channel in channels {
                // Check for cancellation before processing each channel
                if self.is_cancelled(sync_run_id) {
                    info!(
                        "Slack sync {} cancelled, stopping early after {} channels",
                        sync_run_id, processed_channels
                    );
                    break;
                }

                // Only sync channels where the bot is a member
                if !channel.is_member {
                    debug!("Skipping channel {} - bot is not a member", channel.name);
                    continue;
                }

                match self
                    .sync_channel(
                        source_id,
                        sync_run_id,
                        &channel,
                        &creds.bot_token,
                        &sync_state,
                        &content_processor,
                    )
                    .await
                {
                    Ok((message_groups, files)) => {
                        processed_channels += 1;
                        total_message_groups += message_groups;
                        total_files += files;
                        debug!(
                            "Synced channel {}: {} message groups, {} files",
                            channel.name, message_groups, files
                        );
                        // Update scanned count via SDK
                        if let Err(e) = self.sdk_client.increment_scanned(sync_run_id, 1).await {
                            error!("Failed to increment scanned count: {}", e);
                        }
                    }
                    Err(e) => {
                        warn!("Failed to sync channel {}: {}", channel.name, e);
                    }
                }
            }

            info!(
                "Sync completed for source {}: {} channels processed, {} message groups, {} files",
                source_id, processed_channels, total_message_groups, total_files
            );

            Ok((
                processed_channels,
                total_message_groups + total_files,
                total_message_groups + total_files,
            ))
        }
        .await;

        // Check if cancelled
        if self.is_cancelled(sync_run_id) {
            info!("Sync {} was cancelled", sync_run_id);
            let _ = self.sdk_client.cancel(sync_run_id).await;
            self.active_syncs.remove(sync_run_id);
            return Ok(());
        }

        // Unregister sync and report result
        self.active_syncs.remove(sync_run_id);

        match result {
            Ok((scanned, _processed, updated)) => {
                info!(
                    "Sync completed for source {}: {} documents processed",
                    source.name, updated
                );
                self.sdk_client
                    .complete(sync_run_id, scanned as i32, updated as i32, None)
                    .await?;
                Ok(())
            }
            Err(e) => {
                error!("Sync failed for source {}: {}", source.name, e);
                self.sdk_client.fail(sync_run_id, &e.to_string()).await?;
                Err(e)
            }
        }
    }

    async fn fetch_all_users(
        &self,
        token: &str,
        content_processor: &mut ContentProcessor,
    ) -> Result<()> {
        let mut cursor = None;
        let mut all_users = Vec::new();

        loop {
            let response = self
                .slack_client
                .list_users(token, cursor.as_deref())
                .await?;
            all_users.extend(response.members);

            cursor = response
                .response_metadata
                .and_then(|meta| meta.next_cursor)
                .filter(|c| !c.is_empty());

            if cursor.is_none() {
                break;
            }
        }

        content_processor.update_users(all_users);
        Ok(())
    }

    async fn fetch_all_channels(&self, token: &str) -> Result<Vec<crate::models::SlackChannel>> {
        let mut cursor = None;
        let mut all_channels = Vec::new();

        loop {
            let response = self
                .slack_client
                .list_conversations(token, cursor.as_deref())
                .await?;
            all_channels.extend(response.channels);

            cursor = response
                .response_metadata
                .and_then(|meta| meta.next_cursor)
                .filter(|c| !c.is_empty());

            if cursor.is_none() {
                break;
            }
        }

        Ok(all_channels)
    }

    async fn sync_channel(
        &self,
        source_id: &str,
        sync_run_id: &str,
        channel: &crate::models::SlackChannel,
        token: &str,
        sync_state: &SyncState,
        content_processor: &ContentProcessor,
    ) -> Result<(usize, usize)> {
        debug!("Syncing channel: {} ({})", channel.name, channel.id);

        let last_ts = sync_state
            .get_channel_last_ts(source_id, &channel.id)
            .await?;

        let mut all_messages = Vec::new();
        let mut cursor = None;
        let mut latest_ts = last_ts.clone();

        // Fetch channel messages
        loop {
            let response = self
                .slack_client
                .get_conversation_history(
                    token,
                    &channel.id,
                    cursor.as_deref(),
                    last_ts.as_deref(),
                    None,
                )
                .await?;

            // Track the latest timestamp we've seen
            if let Some(first_message) = response.messages.first() {
                latest_ts = Some(first_message.ts.clone());
            }

            all_messages.extend(response.messages);

            if !response.has_more {
                break;
            }

            cursor = response
                .response_metadata
                .and_then(|meta| meta.next_cursor)
                .filter(|c| !c.is_empty());

            if cursor.is_none() {
                break;
            }
        }

        // Group messages by date/thread
        let message_groups = content_processor.group_messages_by_date(
            channel.id.clone(),
            channel.name.clone(),
            all_messages.clone(),
        )?;

        let mut published_groups = 0;
        let mut published_files = 0;

        // Publish message groups
        for group in message_groups {
            // Store content via SDK
            let content_id = match self
                .sdk_client
                .store_content(sync_run_id, &group.to_document_content())
                .await
            {
                Ok(id) => id,
                Err(e) => {
                    error!(
                        "Failed to store content via SDK for Slack message group: {}",
                        e
                    );
                    continue;
                }
            };

            let event = group.to_connector_event(
                sync_run_id.to_string(),
                source_id.to_string(),
                content_id,
            );
            // Emit event via SDK
            if let Err(e) = self
                .sdk_client
                .emit_event(sync_run_id, source_id, event)
                .await
            {
                error!("Failed to emit message group event: {}", e);
                continue;
            }
            published_groups += 1;
        }

        // Extract and process files
        let files = content_processor.extract_files_from_messages(&all_messages);
        for file in files {
            match self.slack_client.download_file(token, file).await {
                Ok(content) if !content.is_empty() => {
                    // Store content via SDK
                    let content_id =
                        match self.sdk_client.store_content(sync_run_id, &content).await {
                            Ok(id) => id,
                            Err(e) => {
                                error!(
                                    "Failed to store content via SDK for Slack file {}: {}",
                                    file.name, e
                                );
                                continue;
                            }
                        };

                    let event = file.to_connector_event(
                        sync_run_id.to_string(),
                        source_id.to_string(),
                        channel.id.clone(),
                        channel.name.clone(),
                        content_id,
                    );
                    // Emit event via SDK
                    if let Err(e) = self
                        .sdk_client
                        .emit_event(sync_run_id, source_id, event)
                        .await
                    {
                        error!("Failed to emit file event: {}", e);
                        continue;
                    }
                    published_files += 1;
                }
                Ok(_) => debug!("Skipped empty file: {}", file.name),
                Err(e) => warn!("Failed to download file {}: {}", file.name, e),
            }
        }

        // Update sync state with latest timestamp
        if let Some(ts) = latest_ts {
            sync_state
                .set_channel_last_ts(source_id, &channel.id, &ts)
                .await?;
        }

        Ok((published_groups, published_files))
    }

    async fn get_bot_token(&self, source_id: &str) -> Result<String> {
        let creds = self
            .sdk_client
            .get_credentials(source_id)
            .await
            .context("Failed to fetch credentials via SDK")?;

        if creds.provider != ServiceProvider::Slack {
            return Err(anyhow!(
                "Expected Slack credentials for source {}, found {:?}",
                source_id,
                creds.provider
            ));
        }

        // Get access_token from credentials map
        creds
            .credentials
            .get("access_token")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string())
            .ok_or_else(|| anyhow!("Missing access_token in Slack credentials"))
    }
}

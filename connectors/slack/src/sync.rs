use anyhow::Result;
use redis::{AsyncCommands, Client as RedisClient};
use shared::models::{Source, SourceType};
use shared::queue::EventQueue;
use sqlx::{PgPool, Row};
use std::collections::HashSet;
use tracing::{debug, error, info, warn};

use crate::auth::AuthManager;
use crate::client::SlackClient;
use crate::content::ContentProcessor;

pub struct SyncManager {
    pool: PgPool,
    redis_client: RedisClient,
    auth_manager: AuthManager,
    slack_client: SlackClient,
    event_queue: EventQueue,
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
    pub async fn new(pool: PgPool, redis_client: RedisClient) -> Result<Self> {
        let event_queue = EventQueue::new(pool.clone());

        Ok(Self {
            pool,
            redis_client,
            auth_manager: AuthManager::new(),
            slack_client: SlackClient::new(),
            event_queue,
        })
    }

    pub async fn sync_all_sources(&self) -> Result<()> {
        let sources = self.get_active_sources().await?;

        info!("Found {} active Slack sources", sources.len());

        for source in sources {
            if let Err(e) = self.sync_source(&source).await {
                error!("Failed to sync source {}: {}", source.id, e);
                self.update_source_status(&source.id, "failed").await?;
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
             AND oc.provider = 'slack'",
        )
        .bind(SourceType::Slack)
        .fetch_all(&self.pool)
        .await?;

        Ok(sources)
    }

    async fn sync_source(&self, source: &Source) -> Result<()> {
        info!("Syncing source: {} ({})", source.name, source.id);

        let bot_token = self.get_bot_token(&source.id).await?;
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
        let _current_channels: HashSet<String> = channels.iter().map(|c| c.id.clone()).collect();

        // Track sync progress
        let mut processed_channels = 0;
        let mut total_message_groups = 0;
        let mut total_files = 0;

        for channel in channels {
            // Only sync channels where the bot is a member
            if !channel.is_member {
                debug!("Skipping channel {} - bot is not a member", channel.name);
                continue;
            }

            match self
                .sync_channel(
                    &source.id,
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
                }
                Err(e) => {
                    warn!("Failed to sync channel {}: {}", channel.name, e);
                }
            }
        }

        info!(
            "Sync completed for source {}: {} channels processed, {} message groups, {} files",
            source.id, processed_channels, total_message_groups, total_files
        );

        self.update_source_status(&source.id, "completed").await?;
        Ok(())
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
            // TODO: Add proper sync_run_id when sync runs are implemented for Slack
            let placeholder_sync_run_id = shared::utils::generate_ulid();
            let event = group.to_connector_event(placeholder_sync_run_id, source_id.to_string());
            match self.event_queue.enqueue(source_id, &event).await {
                Ok(_) => published_groups += 1,
                Err(e) => error!("Failed to queue message group event: {}", e),
            }
        }

        // Extract and process files
        let files = content_processor.extract_files_from_messages(&all_messages);
        for file in files {
            match self.slack_client.download_file(token, file).await {
                Ok(content) if !content.is_empty() => {
                    // TODO: Add proper sync_run_id when sync runs are implemented for Slack
                    let placeholder_sync_run_id = shared::utils::generate_ulid();
                    let event = file.to_connector_event(
                        placeholder_sync_run_id,
                        source_id.to_string(),
                        channel.id.clone(),
                        channel.name.clone(),
                        content,
                    );
                    match self.event_queue.enqueue(source_id, &event).await {
                        Ok(_) => published_files += 1,
                        Err(e) => error!("Failed to queue file event: {}", e),
                    }
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
        let row = sqlx::query(
            "SELECT access_token FROM oauth_credentials 
             WHERE source_id = $1 AND provider = 'slack'",
        )
        .bind(source_id)
        .fetch_one(&self.pool)
        .await?;

        Ok(row.get("access_token"))
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
                .bind(SourceType::Slack)
                .fetch_optional(&self.pool)
                .await?;

        match source {
            Some(source) => {
                if !source.is_active {
                    return Err(anyhow::anyhow!("Source {} is not active", source_id));
                }
                self.sync_source(&source).await
            }
            None => Err(anyhow::anyhow!("Source {} not found", source_id)),
        }
    }
}

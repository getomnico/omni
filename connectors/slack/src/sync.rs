use anyhow::{anyhow, Context, Result};
use chrono::DateTime;
use dashmap::DashMap;
use shared::models::{ServiceProvider, SourceType, SyncRequest, SyncType};
use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use tracing::{debug, error, info, warn};

use crate::auth::AuthManager;
use crate::client::SlackClient;
use crate::content::ContentProcessor;
use crate::models::SlackConnectorState;
use shared::SdkClient;

struct ActiveSync {
    cancelled: AtomicBool,
}

pub struct SyncManager {
    auth_manager: AuthManager,
    slack_client: SlackClient,
    sdk_client: SdkClient,
    active_syncs: DashMap<String, Arc<ActiveSync>>,
}

impl SyncManager {
    pub fn sdk_client(&self) -> &SdkClient {
        &self.sdk_client
    }

    pub fn new(sdk_client: SdkClient) -> Self {
        Self {
            auth_manager: AuthManager::new(),
            slack_client: SlackClient::new(),
            sdk_client,
            active_syncs: DashMap::new(),
        }
    }

    pub fn with_slack_base_url(sdk_client: SdkClient, base_url: String) -> Self {
        Self {
            auth_manager: AuthManager::with_base_url(base_url.clone()),
            slack_client: SlackClient::with_base_url(base_url),
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

        let result: Result<(usize, usize, usize, HashMap<String, String>)> = async {
            let bot_token = self.get_bot_token(source_id).await?;
            let mut creds = self.auth_manager.validate_bot_token(&bot_token).await?;

            self.auth_manager
                .ensure_valid_credentials(&mut creds)
                .await?;

            // Load typed connector state via SDK
            let mut connector_state: SlackConnectorState = self
                .sdk_client
                .get_connector_state(source_id)
                .await?
                .and_then(|state| serde_json::from_value(state).ok())
                .unwrap_or_default();

            connector_state.team_id = Some(creds.team_id.clone());
            self.sdk_client
                .save_connector_state(source_id, serde_json::to_value(&connector_state)?)
                .await?;

            let mut channel_timestamps = connector_state.channel_timestamps;

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

                if !channel.is_member {
                    if channel.is_private {
                        debug!(
                            "Skipping private channel {} - bot must be invited",
                            channel.name
                        );
                        continue;
                    }
                    info!("Auto-joining public channel: {}", channel.name);
                    if let Err(e) = self
                        .slack_client
                        .join_conversation(&creds.bot_token, &channel.id)
                        .await
                    {
                        warn!("Failed to join channel {}: {}", channel.name, e);
                        continue;
                    }
                }

                let last_ts = channel_timestamps.get(&channel.id).cloned();

                match self
                    .sync_channel(
                        source_id,
                        sync_run_id,
                        &channel,
                        &creds.bot_token,
                        last_ts.as_deref(),
                        &content_processor,
                    )
                    .await
                {
                    Ok((message_groups, files, new_latest_ts)) => {
                        processed_channels += 1;
                        total_message_groups += message_groups;
                        total_files += files;
                        if let Some(ts) = new_latest_ts {
                            channel_timestamps.insert(channel.id.clone(), ts);
                        }
                        debug!(
                            "Synced channel {}: {} message groups, {} files",
                            channel.name, message_groups, files
                        );
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
                channel_timestamps,
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
            Ok((scanned, _processed, updated, channel_timestamps)) => {
                info!(
                    "Sync completed for source {}: {} documents processed",
                    source.name, updated
                );
                let mut final_state: SlackConnectorState = self
                    .sdk_client
                    .get_connector_state(source_id)
                    .await?
                    .and_then(|s| serde_json::from_value(s).ok())
                    .unwrap_or_default();
                final_state.channel_timestamps = channel_timestamps;
                let new_state = serde_json::to_value(&final_state)?;
                self.sdk_client
                    .complete(sync_run_id, scanned as i32, updated as i32, Some(new_state))
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
        last_ts: Option<&str>,
        content_processor: &ContentProcessor,
    ) -> Result<(usize, usize, Option<String>)> {
        debug!("Syncing channel: {} ({})", channel.name, channel.id);

        // Round down to start-of-day so we always re-fetch complete days,
        // ensuring the upserted document contains all messages for that day.
        let oldest = last_ts.and_then(|ts| {
            let secs = ts.split('.').next()?.parse::<i64>().ok()?;
            let dt = DateTime::from_timestamp(secs, 0)?;
            let start_of_day = dt.date_naive().and_hms_opt(0, 0, 0)?;
            Some(format!("{}.000000", start_of_day.and_utc().timestamp() - 1))
        });

        let mut all_messages = Vec::new();
        let mut cursor = None;
        let mut latest_ts: Option<String> = last_ts.map(|s| s.to_string());

        // Fetch channel messages
        loop {
            let response = self
                .slack_client
                .get_conversation_history(
                    token,
                    &channel.id,
                    cursor.as_deref(),
                    oldest.as_deref(),
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

        Ok((published_groups, published_files, latest_ts))
    }

    pub async fn sync_realtime_event(&self, source_id: &str, channel_id: &str) -> Result<()> {
        info!(source_id, channel_id, "Starting realtime sync for channel");

        let sync_run_id = self
            .sdk_client
            .create_sync_run(source_id, SyncType::Incremental)
            .await
            .context("Failed to create sync run for realtime event")?;

        let result: Result<()> = async {
            let bot_token = self.get_bot_token(source_id).await?;
            let mut creds = self.auth_manager.validate_bot_token(&bot_token).await?;
            self.auth_manager
                .ensure_valid_credentials(&mut creds)
                .await?;

            let mut connector_state: SlackConnectorState = self
                .sdk_client
                .get_connector_state(source_id)
                .await?
                .and_then(|state| serde_json::from_value(state).ok())
                .unwrap_or_default();

            let channel = self
                .slack_client
                .get_conversation_info(&creds.bot_token, channel_id)
                .await?;

            let mut content_processor = ContentProcessor::new();
            self.fetch_all_users(&creds.bot_token, &mut content_processor)
                .await?;

            let last_ts = connector_state.channel_timestamps.get(channel_id).cloned();

            let (message_groups, files, new_latest_ts) = self
                .sync_channel_for_update(
                    source_id,
                    &sync_run_id,
                    &channel,
                    &creds.bot_token,
                    last_ts.as_deref(),
                    &content_processor,
                )
                .await?;

            if let Some(ts) = new_latest_ts {
                connector_state
                    .channel_timestamps
                    .insert(channel_id.to_string(), ts);
            }

            self.sdk_client
                .save_connector_state(source_id, serde_json::to_value(&connector_state)?)
                .await?;

            let updated = message_groups + files;
            self.sdk_client
                .complete(&sync_run_id, 1, updated as i32, None)
                .await?;

            info!(
                source_id,
                channel_id, message_groups, files, "Realtime sync completed for channel"
            );

            Ok(())
        }
        .await;

        if let Err(e) = &result {
            error!(
                source_id,
                channel_id,
                error = %e,
                "Realtime sync failed for channel"
            );
            let _ = self.sdk_client.fail(&sync_run_id, &e.to_string()).await;
        }

        result
    }

    async fn sync_channel_for_update(
        &self,
        source_id: &str,
        sync_run_id: &str,
        channel: &crate::models::SlackChannel,
        token: &str,
        last_ts: Option<&str>,
        content_processor: &ContentProcessor,
    ) -> Result<(usize, usize, Option<String>)> {
        debug!(
            "Syncing channel for update: {} ({})",
            channel.name, channel.id
        );

        let oldest = last_ts.and_then(|ts| {
            let secs = ts.split('.').next()?.parse::<i64>().ok()?;
            let dt = DateTime::from_timestamp(secs, 0)?;
            let start_of_day = dt.date_naive().and_hms_opt(0, 0, 0)?;
            Some(format!("{}.000000", start_of_day.and_utc().timestamp() - 1))
        });

        let mut all_messages = Vec::new();
        let mut cursor = None;
        let mut latest_ts: Option<String> = last_ts.map(|s| s.to_string());

        loop {
            let response = self
                .slack_client
                .get_conversation_history(
                    token,
                    &channel.id,
                    cursor.as_deref(),
                    oldest.as_deref(),
                    None,
                )
                .await?;

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

        let message_groups = content_processor.group_messages_by_date(
            channel.id.clone(),
            channel.name.clone(),
            all_messages.clone(),
        )?;

        let mut published_groups = 0;
        let mut published_files = 0;

        for group in message_groups {
            let content_id = match self
                .sdk_client
                .store_content(sync_run_id, &group.to_document_content())
                .await
            {
                Ok(id) => id,
                Err(e) => {
                    error!("Failed to store content for realtime sync: {}", e);
                    continue;
                }
            };

            let event =
                group.to_update_event(sync_run_id.to_string(), source_id.to_string(), content_id);
            if let Err(e) = self
                .sdk_client
                .emit_event(sync_run_id, source_id, event)
                .await
            {
                error!("Failed to emit update event: {}", e);
                continue;
            }
            published_groups += 1;
        }

        let files = content_processor.extract_files_from_messages(&all_messages);
        for file in files {
            match self.slack_client.download_file(token, file).await {
                Ok(content) if !content.is_empty() => {
                    let content_id =
                        match self.sdk_client.store_content(sync_run_id, &content).await {
                            Ok(id) => id,
                            Err(e) => {
                                error!("Failed to store file content for realtime sync: {}", e);
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
                    if let Err(e) = self
                        .sdk_client
                        .emit_event(sync_run_id, source_id, event)
                        .await
                    {
                        error!("Failed to emit file update event: {}", e);
                        continue;
                    }
                    published_files += 1;
                }
                Ok(_) => debug!("Skipped empty file: {}", file.name),
                Err(e) => warn!("Failed to download file {}: {}", file.name, e),
            }
        }

        Ok((published_groups, published_files, latest_ts))
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

        creds
            .credentials
            .get("bot_token")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string())
            .ok_or_else(|| anyhow!("Missing bot_token in Slack credentials"))
    }
}

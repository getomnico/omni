use anyhow::{anyhow, Context, Result};
use chrono::{DateTime, NaiveDate};
use omni_connector_sdk::SyncContext;
use omni_connector_sdk::{
    ConnectorEvent, SdkClient, ServiceCredential, ServiceProvider, Source, SourceType, SyncType,
};
use std::collections::BTreeSet;
use std::sync::atomic::AtomicBool;
use std::sync::Arc;
use tracing::{debug, error, info, warn};

use crate::auth::AuthManager;
use crate::client::SlackClient;
use crate::content::ContentProcessor;
use crate::models::{
    MessageGroup, SlackChannel, SlackConnectorState, SlackCredentials, SlackMessage,
};

/// Group identifier for a Slack channel — emitted via `GroupMembershipSync`
/// and referenced by every document's `permissions.groups`.
///
/// TODO: the `groups.email` column is named for email-shaped values (Google's
/// connector emits real Workspace group addresses); Slack channels don't have
/// emails so we use a colon-delimited synthetic. Works mechanically because
/// the column has no format constraint and the searcher's permission filter
/// does exact-string match. The right long-term fix is renaming the column to
/// `external_id`.
fn channel_group_email(team_id: &str, channel_id: &str) -> String {
    format!("slack-channel:{}:{}", team_id, channel_id)
}

/// Default initial-sync history horizon. Tweakable at runtime via the
/// `SLACK_MAX_AGE_DAYS` env var.
const DEFAULT_MAX_AGE_DAYS: i64 = 730;

/// Compute the `oldest` timestamp passed to `conversations.history`.
///
/// - Incremental: round `last_ts` down to start-of-day so the upserted daily
///   document captures every message for that day on re-fetch.
/// - First sync (no `last_ts`), or stale state older than the configured
///   ceiling: cap at `now - SLACK_MAX_AGE_DAYS` (default 2 years). Without
///   this, a fresh sync of a paid Slack workspace would pull years of history
///   at the tier-3 rate limit.
fn channel_oldest_for_sync(last_ts: Option<&str>) -> Option<String> {
    let max_age_days = std::env::var("SLACK_MAX_AGE_DAYS")
        .ok()
        .and_then(|v| v.parse::<i64>().ok())
        .unwrap_or(DEFAULT_MAX_AGE_DAYS);
    let cutoff_secs = chrono::Utc::now().timestamp() - max_age_days * 86400;

    let rounded_last_ts = last_ts
        .and_then(|ts| ts.split('.').next())
        .and_then(|s| s.parse::<i64>().ok())
        .and_then(|secs| {
            let dt = DateTime::from_timestamp(secs, 0)?;
            let start_of_day = dt.date_naive().and_hms_opt(0, 0, 0)?;
            Some(start_of_day.and_utc().timestamp() - 1)
        });

    // max(last_ts_floored, cutoff): never re-fetch past our configured horizon
    // even if state has a much older timestamp.
    let effective_secs = match rounded_last_ts {
        Some(rounded) => rounded.max(cutoff_secs),
        None => cutoff_secs,
    };

    Some(format!("{}.000000", effective_secs))
}

fn slack_ts_seconds(ts: &str) -> Result<i64> {
    ts.split('.')
        .next()
        .ok_or_else(|| anyhow!("Invalid Slack timestamp: {}", ts))?
        .parse::<i64>()
        .map_err(|e| anyhow!("Invalid Slack timestamp {}: {}", ts, e))
}

fn slack_ts_date(ts: &str) -> Result<NaiveDate> {
    let secs = slack_ts_seconds(ts)?;
    DateTime::from_timestamp(secs, 0)
        .map(|dt| dt.date_naive())
        .ok_or_else(|| anyhow!("Invalid Slack timestamp seconds: {}", secs))
}

fn day_bounds(date: NaiveDate) -> Result<(String, String)> {
    let start = date
        .and_hms_opt(0, 0, 0)
        .ok_or_else(|| anyhow!("Invalid day start for {}", date))?
        .and_utc()
        .timestamp();
    let next = date
        .succ_opt()
        .ok_or_else(|| anyhow!("Invalid next day for {}", date))?
        .and_hms_opt(0, 0, 0)
        .ok_or_else(|| anyhow!("Invalid next day start for {}", date))?
        .and_utc()
        .timestamp();

    Ok((format!("{}.999999", start - 1), format!("{}.000000", next)))
}

fn top_level_message(message: &SlackMessage) -> bool {
    match message.thread_ts.as_deref() {
        Some(thread_ts) => thread_ts == message.ts,
        None => true,
    }
}

fn sort_messages_chronological(messages: &mut [SlackMessage]) {
    messages.sort_by(|a, b| a.ts.cmp(&b.ts));
}

pub struct SyncManager {
    auth_manager: AuthManager,
    slack_client: SlackClient,
    sdk_client: SdkClient,
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
        }
    }

    pub fn with_slack_base_url(sdk_client: SdkClient, base_url: String) -> Self {
        Self {
            auth_manager: AuthManager::with_base_url(base_url.clone()),
            slack_client: SlackClient::with_base_url(base_url),
            sdk_client,
        }
    }

    /// SDK trait entrypoint. The SDK has already fetched the source, credentials,
    /// and persisted state, validated their shapes, and provided a `SyncContext`
    /// whose cancellation flag is flipped by the SDK's `/cancel` handler.
    pub async fn run_sync(
        &self,
        _source: Source,
        creds: ServiceCredential,
        state: Option<SlackConnectorState>,
        ctx: SyncContext,
    ) -> Result<()> {
        let sync_run_id = ctx.sync_run_id().to_string();
        let source_id = ctx.source_id().to_string();

        info!(
            "Starting Slack sync for source: {} (sync_run_id: {})",
            source_id, sync_run_id
        );

        if creds.provider != ServiceProvider::Slack {
            return Err(anyhow!(
                "Expected Slack credentials, found {:?}",
                creds.provider
            ));
        }
        let slack_creds: SlackCredentials = serde_json::from_value(creds.credentials)
            .map_err(|e| anyhow!("Failed to decode Slack credentials: {}", e))?;

        let mut bot_creds = self
            .auth_manager
            .validate_bot_token(&slack_creds.bot_token)
            .await?;
        self.auth_manager
            .ensure_valid_credentials(&mut bot_creds)
            .await?;

        let mut connector_state = state.unwrap_or_default();
        let is_full_sync = ctx.sync_mode() == SyncType::Full;

        let mut content_processor = ContentProcessor::new();
        self.fetch_all_users(&bot_creds.bot_token, &mut content_processor)
            .await?;

        let channels = self.fetch_all_channels(&bot_creds.bot_token).await?;

        let mut processed_channels = 0;
        let mut total_message_groups = 0;
        let mut total_files = 0;

        for channel in channels {
            if ctx.is_cancelled() {
                info!(
                    "Slack sync {} cancelled, stopping early after {} channels",
                    sync_run_id, processed_channels
                );
                // Persist completed-channel timestamps before exiting so the
                // next run doesn't redo the work that completed before cancel.
                ctx.flush().await?;
                ctx.save_connector_state(serde_json::to_value(&connector_state)?)
                    .await?;
                ctx.cancel().await?;
                return Ok(());
            }

            // IMs/MPIMs are implicitly joined; only public channels can be
            // auto-joined. Private channels require an external invite.
            if !channel.is_member && !channel.is_im && !channel.is_mpim {
                if channel.is_private {
                    debug!(
                        "Skipping private channel {} - bot must be invited",
                        channel.display_name()
                    );
                    continue;
                }
                if channel.requires_join() {
                    info!("Auto-joining public channel: {}", channel.display_name());
                    if let Err(e) = self
                        .slack_client
                        .join_conversation(&bot_creds.bot_token, &channel.id)
                        .await
                    {
                        warn!("Failed to join channel {}: {}", channel.display_name(), e);
                        continue;
                    }
                }
            }

            let last_ts = if is_full_sync {
                None
            } else {
                connector_state.channel_timestamps.get(&channel.id).cloned()
            };

            let group_email = channel_group_email(&bot_creds.team_id, &channel.id);

            match self
                .sync_channel(
                    &ctx,
                    &channel,
                    &group_email,
                    &bot_creds.bot_token,
                    last_ts.as_deref(),
                    &content_processor,
                    false,
                )
                .await
            {
                Ok((mg, fl, new_ts)) => {
                    processed_channels += 1;
                    total_message_groups += mg;
                    total_files += fl;
                    if let Some(ts) = new_ts {
                        connector_state
                            .channel_timestamps
                            .insert(channel.id.clone(), ts);
                    }
                    debug!(
                        "Synced channel {}: {} message groups, {} files",
                        channel.display_name(),
                        mg,
                        fl
                    );

                    // Per-channel checkpoint: flush emitted events FIRST so
                    // that the persisted timestamp never advances past events
                    // that haven't reached the queue. If flush fails, skip the
                    // checkpoint — next run will reprocess this channel
                    // (emit is idempotent on document_id).
                    match ctx.flush().await {
                        Ok(()) => {
                            if let Err(e) = ctx
                                .save_connector_state(serde_json::to_value(&connector_state)?)
                                .await
                            {
                                warn!(
                                    "Failed to checkpoint state after channel {}: {}",
                                    channel.display_name(),
                                    e
                                );
                            }
                        }
                        Err(e) => warn!(
                            "Failed to flush events after channel {}, skipping checkpoint: {}",
                            channel.display_name(),
                            e
                        ),
                    }

                    let emitted_documents = mg + fl;
                    if emitted_documents > 0 {
                        if let Err(e) = ctx.increment_scanned(emitted_documents as i32).await {
                            error!("Failed to increment scanned count: {}", e);
                        }
                    }
                }
                Err(e) => {
                    warn!("Failed to sync channel {}: {}", channel.display_name(), e);
                }
            }
        }

        info!(
            "Sync completed for source {}: {} channels processed, {} message groups, {} files",
            source_id, processed_channels, total_message_groups, total_files
        );

        ctx.flush().await?;
        ctx.save_connector_state(serde_json::to_value(&connector_state)?)
            .await?;
        ctx.complete().await?;

        Ok(())
    }

    /// Realtime path: invoked by Socket Mode after a debounce window expires
    /// for `channel_id`. There is no inbound `/sync` HTTP request, so we
    /// create our own sync run and `SyncContext` from the manager's
    /// `SdkClient` and then reuse the same per-channel sync helper.
    pub async fn sync_realtime_event(&self, source_id: &str, channel_id: &str) -> Result<()> {
        info!(source_id, channel_id, "Starting realtime sync for channel");

        let sync_run_id = self
            .sdk_client
            .create_sync_run(source_id, SyncType::Incremental)
            .await
            .context("Failed to create sync run for realtime event")?;

        let ctx = SyncContext::new(
            self.sdk_client.clone(),
            sync_run_id.clone(),
            source_id.to_string(),
            SourceType::Slack,
            SyncType::Incremental,
            Arc::new(AtomicBool::new(false)),
        );

        let result: Result<()> = async {
            let creds = self
                .sdk_client
                .get_credentials(source_id)
                .await
                .context("Failed to fetch credentials for realtime sync")?;
            if creds.provider != ServiceProvider::Slack {
                return Err(anyhow!(
                    "Expected Slack credentials for source {}, found {:?}",
                    source_id,
                    creds.provider
                ));
            }
            let slack_creds: SlackCredentials = serde_json::from_value(creds.credentials)
                .map_err(|e| anyhow!("Failed to decode Slack credentials: {}", e))?;

            let mut bot_creds = self
                .auth_manager
                .validate_bot_token(&slack_creds.bot_token)
                .await?;
            self.auth_manager
                .ensure_valid_credentials(&mut bot_creds)
                .await?;

            let mut connector_state: SlackConnectorState = self
                .sdk_client
                .get_connector_state(source_id)
                .await?
                .and_then(|state| serde_json::from_value(state).ok())
                .unwrap_or_default();

            let channel = self
                .slack_client
                .get_conversation_info(&bot_creds.bot_token, channel_id)
                .await?;

            let mut content_processor = ContentProcessor::new();
            self.fetch_all_users(&bot_creds.bot_token, &mut content_processor)
                .await?;

            let last_ts = connector_state.channel_timestamps.get(channel_id).cloned();

            let group_email = channel_group_email(&bot_creds.team_id, &channel.id);

            let (mg, fl, new_ts) = self
                .sync_channel(
                    &ctx,
                    &channel,
                    &group_email,
                    &bot_creds.bot_token,
                    last_ts.as_deref(),
                    &content_processor,
                    true,
                )
                .await?;

            if let Some(ts) = new_ts {
                connector_state
                    .channel_timestamps
                    .insert(channel_id.to_string(), ts);
            }

            ctx.flush().await?;
            ctx.save_connector_state(serde_json::to_value(&connector_state)?)
                .await?;

            let updated = mg + fl;
            if updated > 0 {
                ctx.increment_scanned(updated as i32).await?;
                ctx.increment_updated(updated as i32).await?;
            }
            ctx.complete().await?;

            info!(
                source_id,
                channel_id, mg, fl, "Realtime sync completed for channel"
            );
            Ok(())
        }
        .await;

        if let Err(e) = &result {
            error!(source_id, channel_id, error = %e, "Realtime sync failed for channel");
            let _ = ctx.fail(&e.to_string()).await;
        }

        result
    }

    pub async fn sync_realtime_message_event(
        &self,
        source_id: &str,
        channel_id: &str,
        message_ts: &str,
        thread_ts: Option<&str>,
    ) -> Result<()> {
        info!(
            source_id,
            channel_id, message_ts, thread_ts, "Starting realtime Slack message repair"
        );

        let sync_run_id = self
            .sdk_client
            .create_sync_run(source_id, SyncType::Realtime)
            .await
            .context("Failed to create sync run for realtime message event")?;

        let ctx = SyncContext::new(
            self.sdk_client.clone(),
            sync_run_id.clone(),
            source_id.to_string(),
            SourceType::Slack,
            SyncType::Realtime,
            Arc::new(AtomicBool::new(false)),
        );

        let result: Result<()> = async {
            let creds = self
                .sdk_client
                .get_credentials(source_id)
                .await
                .context("Failed to fetch credentials for realtime sync")?;
            if creds.provider != ServiceProvider::Slack {
                return Err(anyhow!(
                    "Expected Slack credentials for source {}, found {:?}",
                    source_id,
                    creds.provider
                ));
            }
            let slack_creds: SlackCredentials = serde_json::from_value(creds.credentials)
                .map_err(|e| anyhow!("Failed to decode Slack credentials: {}", e))?;

            let mut bot_creds = self
                .auth_manager
                .validate_bot_token(&slack_creds.bot_token)
                .await?;
            self.auth_manager
                .ensure_valid_credentials(&mut bot_creds)
                .await?;

            let channel = self
                .slack_client
                .get_conversation_info(&bot_creds.bot_token, channel_id)
                .await?;

            let mut content_processor = ContentProcessor::new();
            self.fetch_all_users(&bot_creds.bot_token, &mut content_processor)
                .await?;

            let group_email = channel_group_email(&bot_creds.team_id, &channel.id);
            self.emit_channel_group_membership(
                &ctx,
                &sync_run_id,
                source_id,
                &channel,
                &group_email,
                &bot_creds.bot_token,
                &content_processor,
            )
            .await?;

            let emitted_documents = match thread_ts {
                Some(parent_ts) if parent_ts != message_ts => {
                    self.repair_thread_document(
                        &ctx,
                        &sync_run_id,
                        source_id,
                        &channel,
                        &group_email,
                        &bot_creds.bot_token,
                        &content_processor,
                        parent_ts,
                        true,
                    )
                    .await?
                }
                _ => {
                    let date = slack_ts_date(message_ts)?;
                    self.repair_day_documents(
                        &ctx,
                        &sync_run_id,
                        source_id,
                        &channel,
                        &group_email,
                        &bot_creds.bot_token,
                        &content_processor,
                        date,
                        true,
                    )
                    .await?
                }
            };

            ctx.flush().await?;
            if emitted_documents > 0 {
                ctx.increment_scanned(emitted_documents as i32).await?;
                ctx.increment_updated(emitted_documents as i32).await?;
            }
            ctx.complete().await?;
            Ok(())
        }
        .await;

        if let Err(e) = &result {
            error!(source_id, channel_id, message_ts, error = %e, "Realtime message repair failed");
            let _ = ctx.fail(&e.to_string()).await;
        }

        result
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

    async fn fetch_all_channels(&self, token: &str) -> Result<Vec<SlackChannel>> {
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

    async fn fetch_history_window_all(
        &self,
        token: &str,
        channel_id: &str,
        oldest: Option<&str>,
        latest: Option<&str>,
    ) -> Result<Vec<SlackMessage>> {
        let mut messages = Vec::new();
        let mut cursor = None;

        loop {
            let response = self
                .slack_client
                .get_conversation_history(token, channel_id, cursor.as_deref(), oldest, latest)
                .await?;

            messages.extend(response.messages);

            if !response.has_more {
                break;
            }

            cursor = response
                .response_metadata
                .and_then(|meta| meta.next_cursor)
                .filter(|c| !c.is_empty());

            if cursor.is_none() {
                return Err(anyhow!(
                    "conversations.history for channel {} returned has_more without next_cursor",
                    channel_id
                ));
            }
        }

        sort_messages_chronological(&mut messages);
        Ok(messages)
    }

    async fn fetch_thread_permalink(
        &self,
        token: &str,
        channel_id: &str,
        thread_ts: &str,
    ) -> Option<String> {
        match self
            .slack_client
            .get_permalink(token, channel_id, thread_ts)
            .await
        {
            Ok(permalink) => Some(permalink),
            Err(e) => {
                warn!(
                    "Failed to fetch Slack permalink for channel {} thread {}: {}",
                    channel_id, thread_ts, e
                );
                None
            }
        }
    }

    async fn fetch_thread_replies_all(
        &self,
        token: &str,
        channel_id: &str,
        thread_ts: &str,
    ) -> Result<Vec<SlackMessage>> {
        let mut messages = Vec::new();
        let mut cursor = None;

        loop {
            let response = self
                .slack_client
                .get_thread_replies(token, channel_id, thread_ts, cursor.as_deref())
                .await?;
            messages.extend(response.messages);

            if !response.has_more {
                break;
            }

            cursor = response
                .response_metadata
                .and_then(|meta| meta.next_cursor)
                .filter(|c| !c.is_empty());

            if cursor.is_none() {
                return Err(anyhow!(
                    "conversations.replies for channel {} thread {} returned has_more without next_cursor",
                    channel_id,
                    thread_ts
                ));
            }
        }

        sort_messages_chronological(&mut messages);
        Ok(messages)
    }

    async fn emit_message_group(
        &self,
        ctx: &SyncContext,
        group: crate::models::MessageGroup,
        sync_run_id: &str,
        source_id: &str,
        group_email: &str,
        is_update: bool,
    ) -> Result<()> {
        let content_id = ctx.store_content(&group.to_document_content()).await?;
        let event = if is_update {
            group.to_update_event(
                sync_run_id.to_string(),
                source_id.to_string(),
                content_id,
                group_email,
            )
        } else {
            group.to_connector_event(
                sync_run_id.to_string(),
                source_id.to_string(),
                content_id,
                group_email,
            )
        };
        ctx.emit_event(event).await?;
        Ok(())
    }

    async fn emit_channel_group_membership(
        &self,
        ctx: &SyncContext,
        sync_run_id: &str,
        source_id: &str,
        channel: &SlackChannel,
        group_email: &str,
        token: &str,
        content_processor: &ContentProcessor,
    ) -> Result<()> {
        let member_ids = self.fetch_channel_members(token, &channel.id).await?;
        let member_emails = content_processor.resolve_member_emails(&member_ids);

        let group_event = ConnectorEvent::GroupMembershipSync {
            sync_run_id: sync_run_id.to_string(),
            source_id: source_id.to_string(),
            group_email: group_email.to_string(),
            group_name: Some(format!("#{}", channel.display_name())),
            member_emails,
        };
        ctx.emit_event(group_event).await?;
        Ok(())
    }

    async fn repair_thread_document(
        &self,
        ctx: &SyncContext,
        sync_run_id: &str,
        source_id: &str,
        channel: &SlackChannel,
        group_email: &str,
        token: &str,
        content_processor: &ContentProcessor,
        thread_ts: &str,
        is_update: bool,
    ) -> Result<usize> {
        let thread_messages = self
            .fetch_thread_replies_all(token, &channel.id, thread_ts)
            .await
            .with_context(|| {
                format!(
                    "Failed to fully fetch Slack thread {} for channel {}",
                    thread_ts, channel.id
                )
            })?;
        let thread_date = slack_ts_date(thread_ts)?;
        let mut thread_group = MessageGroup::new(
            channel.id.clone(),
            channel.display_name(),
            thread_date,
            true,
            Some(thread_ts.to_string()),
        );
        thread_group.set_permalink(
            self.fetch_thread_permalink(token, &channel.id, thread_ts)
                .await,
        );
        for message in thread_messages {
            let author_name = content_processor.get_author_name(&message.user);
            thread_group.add_message(message, author_name);
        }
        self.emit_message_group(
            ctx,
            thread_group,
            sync_run_id,
            source_id,
            group_email,
            is_update,
        )
        .await?;
        Ok(1)
    }

    async fn repair_day_documents(
        &self,
        ctx: &SyncContext,
        sync_run_id: &str,
        source_id: &str,
        channel: &SlackChannel,
        group_email: &str,
        token: &str,
        content_processor: &ContentProcessor,
        date: NaiveDate,
        is_update: bool,
    ) -> Result<usize> {
        let (day_oldest, day_latest) = day_bounds(date)?;
        let day_messages = self
            .fetch_history_window_all(token, &channel.id, Some(&day_oldest), Some(&day_latest))
            .await
            .with_context(|| {
                format!(
                    "Failed to fully fetch Slack day {} for channel {}",
                    date, channel.id
                )
            })?;

        let mut top_level_messages: Vec<SlackMessage> = day_messages
            .iter()
            .filter(|message| top_level_message(message))
            .cloned()
            .collect();
        sort_messages_chronological(&mut top_level_messages);

        let message_groups = content_processor.group_messages_by_date(
            channel.id.clone(),
            channel.display_name(),
            top_level_messages.clone(),
        )?;
        let mut emitted_documents = 0;
        for group in message_groups {
            self.emit_message_group(ctx, group, sync_run_id, source_id, group_email, is_update)
                .await?;
            emitted_documents += 1;
        }

        for parent in top_level_messages
            .iter()
            .filter(|message| message.reply_count.unwrap_or(0) > 0)
        {
            emitted_documents += self
                .repair_thread_document(
                    ctx,
                    sync_run_id,
                    source_id,
                    channel,
                    group_email,
                    token,
                    content_processor,
                    &parent.ts,
                    is_update,
                )
                .await?;
        }

        Ok(emitted_documents)
    }

    /// Sync a single channel, emitting documents through the SDK's
    /// `SyncContext`. When `is_update` is true the events are emitted as
    /// `DocumentUpdated` (used by the realtime path); otherwise as
    /// `DocumentCreated`.
    async fn sync_channel(
        &self,
        ctx: &SyncContext,
        channel: &SlackChannel,
        group_email: &str,
        token: &str,
        last_ts: Option<&str>,
        content_processor: &ContentProcessor,
        is_update: bool,
    ) -> Result<(usize, usize, Option<String>)> {
        let source_id = ctx.source_id().to_string();
        let sync_run_id = ctx.sync_run_id().to_string();
        debug!(
            "Syncing channel: {} ({})",
            channel.display_name(),
            channel.id
        );

        let oldest = channel_oldest_for_sync(last_ts);
        let discovery_messages = self
            .fetch_history_window_all(token, &channel.id, oldest.as_deref(), None)
            .await?;

        let latest_ts = discovery_messages
            .iter()
            .map(|message| message.ts.as_str())
            .max()
            .map(|ts| ts.to_string())
            .or_else(|| last_ts.map(|s| s.to_string()));

        let mut affected_dates = BTreeSet::new();
        for message in discovery_messages
            .iter()
            .filter(|message| top_level_message(message))
        {
            affected_dates.insert(slack_ts_date(&message.ts)?);
        }

        let mut message_groups = Vec::new();
        let mut all_messages = Vec::new();

        for date in affected_dates {
            let (day_oldest, day_latest) = day_bounds(date)?;
            let day_messages = self
                .fetch_history_window_all(token, &channel.id, Some(&day_oldest), Some(&day_latest))
                .await
                .with_context(|| {
                    format!(
                        "Failed to fully fetch Slack day {} for channel {}",
                        date, channel.id
                    )
                })?;

            let mut top_level_messages: Vec<SlackMessage> = day_messages
                .iter()
                .filter(|message| top_level_message(message))
                .cloned()
                .collect();
            sort_messages_chronological(&mut top_level_messages);
            all_messages.extend(top_level_messages.clone());

            message_groups.extend(content_processor.group_messages_by_date(
                channel.id.clone(),
                channel.display_name(),
                top_level_messages.clone(),
            )?);

            for parent in top_level_messages
                .iter()
                .filter(|message| message.reply_count.unwrap_or(0) > 0)
            {
                let thread_messages = self
                    .fetch_thread_replies_all(token, &channel.id, &parent.ts)
                    .await
                    .with_context(|| {
                        format!(
                            "Failed to fully fetch Slack thread {} for channel {}",
                            parent.ts, channel.id
                        )
                    })?;

                all_messages.extend(thread_messages.clone());
                let thread_date = slack_ts_date(&parent.ts)?;
                let mut thread_group = MessageGroup::new(
                    channel.id.clone(),
                    channel.display_name(),
                    thread_date,
                    true,
                    Some(parent.ts.clone()),
                );
                thread_group.set_permalink(
                    self.fetch_thread_permalink(token, &channel.id, &parent.ts)
                        .await,
                );
                for message in thread_messages {
                    let author_name = content_processor.get_author_name(&message.user);
                    thread_group.add_message(message, author_name);
                }
                message_groups.push(thread_group);
            }
        }

        self.emit_channel_group_membership(
            ctx,
            &sync_run_id,
            &source_id,
            channel,
            group_email,
            token,
            content_processor,
        )
        .await?;

        let mut published_groups = 0;
        let mut published_files = 0;

        for group in message_groups {
            self.emit_message_group(ctx, group, &sync_run_id, &source_id, group_email, is_update)
                .await
                .context("Failed to emit Slack message group")?;
            published_groups += 1;
        }

        let files = content_processor.extract_files_from_messages(&all_messages);
        for file in files {
            let (bytes, response_content_type) =
                match self.slack_client.download_file(token, file).await {
                    Ok(Some((bytes, ct))) if !bytes.is_empty() => (bytes, ct),
                    Ok(_) => {
                        debug!("Skipped empty/missing file: {}", file.name);
                        continue;
                    }
                    Err(e) => {
                        warn!("Failed to download file {}: {}", file.name, e);
                        continue;
                    }
                };

            // Slack's `mimetype` is more reliable than the HTTP content-type
            // (which is sometimes generic for redirected file downloads).
            let mime = file.mimetype.clone().unwrap_or(response_content_type);

            // Route through the connector-manager extractor so binary files
            // (PDFs, DOCX, images) get text extracted via Docling. Text files
            // pass through as-is. Failures here are non-fatal — we skip the
            // file but continue the sync.
            let content_id = match ctx
                .extract_and_store_content(bytes, &mime, Some(&file.name))
                .await
            {
                Ok(id) => id,
                Err(e) => {
                    warn!(
                        "Failed to extract/store content for Slack file {} ({}): {}",
                        file.name, mime, e
                    );
                    continue;
                }
            };

            let event = file.to_connector_event(
                sync_run_id.clone(),
                source_id.clone(),
                channel.id.clone(),
                channel.display_name(),
                content_id,
                group_email,
            );
            if let Err(e) = ctx.emit_event(event).await {
                error!("Failed to emit file event: {}", e);
                continue;
            }
            published_files += 1;
        }

        Ok((published_groups, published_files, latest_ts))
    }

    async fn fetch_channel_members(&self, token: &str, channel_id: &str) -> Result<Vec<String>> {
        let mut all_members = Vec::new();
        let mut cursor = None;

        loop {
            let response = self
                .slack_client
                .get_conversation_members(token, channel_id, cursor.as_deref())
                .await?;
            all_members.extend(response.members);

            cursor = response
                .response_metadata
                .and_then(|meta| meta.next_cursor)
                .filter(|c| !c.is_empty());

            if cursor.is_none() {
                break;
            }
        }

        Ok(all_members)
    }
}

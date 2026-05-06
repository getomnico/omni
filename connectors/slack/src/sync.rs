use anyhow::{anyhow, Context, Result};
use chrono::DateTime;
use omni_connector_sdk::SyncContext;
use shared::models::{
    ConnectorEvent, ServiceCredential, ServiceProvider, Source, SourceType, SyncType,
};
use shared::SdkClient;
use std::sync::atomic::AtomicBool;
use std::sync::Arc;
use tracing::{debug, error, info, warn};

use crate::auth::AuthManager;
use crate::client::SlackClient;
use crate::content::ContentProcessor;
use crate::models::{SlackChannel, SlackConnectorState};

/// Group identifier for a Slack channel — emitted via `GroupMembershipSync`
/// and referenced by every document's `permissions.groups`.
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
        let bot_token = creds
            .credentials
            .get("bot_token")
            .and_then(|v| v.as_str())
            .ok_or_else(|| anyhow!("Missing bot_token in Slack credentials"))?
            .to_string();

        let mut bot_creds = self.auth_manager.validate_bot_token(&bot_token).await?;
        self.auth_manager
            .ensure_valid_credentials(&mut bot_creds)
            .await?;

        // Initialize state with team_id so Socket Mode reconnect-on-startup can
        // use it to find sources whose first sync has completed.
        let mut connector_state = state.unwrap_or_default();
        connector_state.team_id = Some(bot_creds.team_id.clone());
        ctx.save_connector_state(serde_json::to_value(&connector_state)?)
            .await?;

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

            let last_ts = connector_state.channel_timestamps.get(&channel.id).cloned();

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

                    if let Err(e) = ctx.increment_scanned(1).await {
                        error!("Failed to increment scanned count: {}", e);
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
            let bot_token = creds
                .credentials
                .get("bot_token")
                .and_then(|v| v.as_str())
                .ok_or_else(|| anyhow!("Missing bot_token in Slack credentials"))?
                .to_string();

            let mut bot_creds = self.auth_manager.validate_bot_token(&bot_token).await?;
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
            ctx.increment_scanned(1).await?;
            ctx.increment_updated(updated as i32).await?;
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

        // Slack's `conversations.history` returns top-level messages only —
        // thread replies must be fetched per parent via `conversations.replies`.
        // For each parent with reply_count > 0, fetch its replies (skipping
        // index 0, which is the parent itself, already in `all_messages`).
        let parents_with_replies: Vec<String> = all_messages
            .iter()
            .filter(|m| m.reply_count.unwrap_or(0) > 0)
            .map(|m| m.ts.clone())
            .collect();

        for parent_ts in parents_with_replies {
            let mut cursor = None;
            loop {
                let response = match self
                    .slack_client
                    .get_thread_replies(token, &channel.id, &parent_ts, cursor.as_deref())
                    .await
                {
                    Ok(r) => r,
                    Err(e) => {
                        warn!(
                            "Failed to fetch thread replies for {} in {}: {}",
                            parent_ts,
                            channel.display_name(),
                            e
                        );
                        break;
                    }
                };
                all_messages.extend(response.messages.into_iter().skip(1));
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
        }

        let message_groups = content_processor.group_messages_by_date(
            channel.id.clone(),
            channel.display_name(),
            all_messages.clone(),
        )?;

        let member_ids = self.fetch_channel_members(token, &channel.id).await?;
        let member_emails = content_processor.resolve_member_emails(&member_ids);

        // Emit channel membership as a single group sync event. Documents in
        // this channel reference `group_email` via `permissions.groups`, so the
        // member list never needs to be inlined into individual doc events.
        let group_event = ConnectorEvent::GroupMembershipSync {
            sync_run_id: sync_run_id.clone(),
            source_id: source_id.clone(),
            group_email: group_email.to_string(),
            group_name: Some(format!("#{}", channel.display_name())),
            member_emails: member_emails.clone(),
        };
        if let Err(e) = ctx.emit_event(group_event).await {
            warn!(
                "Failed to emit group membership sync for {}: {}",
                channel.display_name(),
                e
            );
        }

        let mut published_groups = 0;
        let mut published_files = 0;

        for group in message_groups {
            let content_id = match ctx.store_content(&group.to_document_content()).await {
                Ok(id) => id,
                Err(e) => {
                    error!(
                        "Failed to store content via SDK for Slack message group: {}",
                        e
                    );
                    continue;
                }
            };

            let event = if is_update {
                group.to_update_event(
                    sync_run_id.clone(),
                    source_id.clone(),
                    content_id,
                    group_email,
                )
            } else {
                group.to_connector_event(
                    sync_run_id.clone(),
                    source_id.clone(),
                    content_id,
                    group_email,
                )
            };
            if let Err(e) = ctx.emit_event(event).await {
                error!("Failed to emit message group event: {}", e);
                continue;
            }
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

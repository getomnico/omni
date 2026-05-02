use anyhow::{anyhow, Context, Result};
use dashmap::DashMap;
use omni_connector_sdk::SyncContext;
use std::collections::{HashMap, HashSet};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};
use time::{self, OffsetDateTime};
use tokio::sync::Notify;
use tracing::{debug, error, info, warn};

use crate::admin::AdminClient;
use crate::auth::{GoogleAuth, OAuthAuth};
use crate::cache::LruFolderCache;
use crate::connector::build_attachment_doc_id;
use crate::drive::{DriveClient, FileContent};
use crate::gmail::{BatchThreadResult, ExtractedAttachment, GmailClient, MessageFormat};
use crate::models::{
    mime_type_to_content_type, AttachmentPointer, GmailThread, GoogleConnectorState, UserFile,
    WebhookChannel, WebhookChannelResponse, WebhookNotification,
};
use serde_json::json;
use shared::models::{
    AuthType, ConnectorEvent, DocumentMetadata, DocumentPermissions, ServiceCredential,
    ServiceProvider, Source, SourceType, SyncType,
};
use shared::RateLimiter;
use shared::SdkClient;

pub struct WebhookDebounce {
    pub last_received: Instant,
    pub last_event_type: String,
    pub count: u32,
}

pub struct SyncManager {
    drive_client: DriveClient,
    gmail_client: GmailClient,
    admin_client: Arc<AdminClient>,
    // TODO: Remove this one we wire in the webhook codepath to use SyncContext as well
    pub sdk_client: SdkClient,
    folder_cache: LruFolderCache,
    webhook_url: Option<String>,
    pub webhook_debounce: DashMap<String, WebhookDebounce>,
    webhook_notify: Arc<Notify>,
    pub debounce_duration_ms: AtomicU64,
}

impl SyncManager {
    pub fn new(
        admin_client: Arc<AdminClient>,
        sdk_client: SdkClient,
        webhook_url: Option<String>,
    ) -> Self {
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
        let drive_client = DriveClient::with_rate_limiter(rate_limiter.clone());
        let gmail_client = GmailClient::with_rate_limiter(rate_limiter);

        Self {
            drive_client,
            gmail_client,
            admin_client,
            sdk_client,
            folder_cache: LruFolderCache::new(10_000),
            webhook_url,
            webhook_debounce: DashMap::new(),
            webhook_notify: Arc::new(Notify::new()),
            debounce_duration_ms: AtomicU64::new(10 * 60 * 1000),
        }
    }

    pub fn gmail_client(&self) -> &GmailClient {
        &self.gmail_client
    }

    /// Run a sync driven by the SDK. The SDK passes in the full Source and
    /// optional ServiceCredential, the persisted State, and a `SyncContext`
    /// whose cancellation flag is flipped by the SDK's `/cancel` handler.
    pub async fn run_sync(
        &self,
        source: Source,
        credentials: Option<ServiceCredential>,
        state: Option<GoogleConnectorState>,
        ctx: SyncContext,
    ) -> Result<()> {
        let sync_run_id = ctx.sync_run_id().to_string();
        let source_id = ctx.source_id().to_string();

        info!(
            "Starting sync for source {} (sync_run_id: {})",
            source_id, sync_run_id
        );

        let creds =
            credentials.ok_or_else(|| anyhow!("Google sync requires service credentials"))?;
        if creds.provider != ServiceProvider::Google {
            return Err(anyhow!(
                "Expected Google credentials for source {}, found {:?}",
                source_id,
                creds.provider
            ));
        }

        let outcome = self.run_sync_inner(&source, &creds, state, &ctx).await;

        match outcome {
            Ok(Some(final_state)) => {
                // Save the final state explicitly even though run_sync_inner
                // checkpoints mid-sync — the inner pass might have made
                // additional state mutations after the last checkpoint.
                let state_json = serde_json::to_value(&final_state)?;
                ctx.save_connector_state(state_json).await?;
                ctx.complete().await?;
                Ok(())
            }
            // Cancelled mid-sync: tell the SDK so the run is marked
            // `cancelled` rather than `failed`. Returning Ok keeps the
            // SDK's default-fail branch from firing. Per-user state was
            // already checkpointed mid-sync via `ctx.save_connector_state`.
            Ok(None) => {
                info!("Sync {} was cancelled", sync_run_id);
                ctx.cancel().await?;
                Ok(())
            }
            Err(e) => Err(e),
        }
    }

    /// Inner sync body. Returns `Ok(None)` if the sync was cancelled
    /// mid-flight (distinct from a successful completion or a hard
    /// failure). Returns `Ok(Some(state))` with the final connector state
    /// on success, which the caller persists via `ctx.complete`.
    async fn run_sync_inner(
        &self,
        source: &Source,
        service_creds: &ServiceCredential,
        existing_state: Option<GoogleConnectorState>,
        ctx: &SyncContext,
    ) -> Result<Option<GoogleConnectorState>> {
        let source_id = ctx.source_id();
        let sync_type = ctx.sync_mode();

        let known_groups = self.maybe_sync_groups(source, service_creds, ctx).await;

        // The SDK passes us the persisted state on each (re-)dispatch — we use
        // it directly instead of refetching via HTTP. On a fresh sync this is
        // None; on resume after a crash, this is the last checkpoint written
        // mid-sync.
        let existing_state = existing_state.unwrap_or_default();

        let result = match source.source_type {
            SourceType::GoogleDrive => {
                self.sync_drive_source_internal(
                    source,
                    service_creds,
                    sync_type,
                    existing_state,
                    ctx,
                )
                .await
            }
            SourceType::Gmail => {
                self.sync_gmail_source_internal(
                    source,
                    service_creds,
                    sync_type,
                    existing_state,
                    known_groups,
                    ctx,
                )
                .await
            }
            _ => Err(anyhow!("Unsupported source type: {:?}", source.source_type)),
        };

        if result.is_ok() && source.source_type == SourceType::GoogleDrive {
            self.ensure_webhook_registered(source_id).await;
        }

        if ctx.is_cancelled() {
            return Ok(None);
        }

        result.map(Some)
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

    async fn sync_drive_for_user(
        &self,
        user_email: &str,
        service_auth: Arc<GoogleAuth>,
        source_id: &str,
        sync_run_id: &str,
        ctx: &SyncContext,
        created_after: Option<&str>,
    ) -> Result<(usize, usize)> {
        info!("Processing Drive files for user: {}", user_email);

        let mut total_scanned = 0;
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

            let page_file_count = response.files.len();
            debug!(
                "Got {} files in this page with page_token: '{:?}' for user {}",
                page_file_count, page_token, user_email
            );

            // Process files in this page. Indexer dedups by (external_id, modified_time);
            // we always emit and let the indexer skip unchanged docs.
            for file in response.files {
                if self.should_index_file(&file) {
                    file_batch.push(UserFile {
                        user_email: Arc::new(user_email.to_string()),
                        file,
                    });

                    if file_batch.len() >= BATCH_SIZE {
                        let (scanned, updated) = self
                            .process_file_batch(
                                file_batch.clone(),
                                source_id,
                                sync_run_id,
                                ctx,
                                service_auth.clone(),
                            )
                            .await?;

                        total_scanned += scanned;
                        total_updated += updated;
                        file_batch.clear();
                    }
                }
            }

            // Check for cancellation
            if ctx.is_cancelled() {
                info!(
                    "Sync {} cancelled, stopping Drive sync for user {}",
                    sync_run_id, user_email
                );
                break;
            }

            // Check if there are more pages
            page_token = response.next_page_token;
            if page_token.is_none() {
                break;
            }
        }

        // Process any remaining files in the batch
        if !file_batch.is_empty() {
            let (scanned, updated) = self
                .process_file_batch(
                    file_batch,
                    source_id,
                    sync_run_id,
                    ctx,
                    service_auth.clone(),
                )
                .await?;

            total_scanned += scanned;
            total_updated += updated;
        }

        info!(
            "Completed processing user {}: {} scanned, {} updated",
            user_email, total_scanned, total_updated
        );
        Ok((total_scanned, total_updated))
    }

    async fn sync_drive_for_user_incremental(
        &self,
        user_email: &str,
        service_auth: Arc<GoogleAuth>,
        source_id: &str,
        sync_run_id: &str,
        ctx: &SyncContext,
        start_page_token: &str,
    ) -> Result<(usize, usize)> {
        info!(
            "Processing incremental Drive sync for user {} from pageToken {}",
            user_email, start_page_token
        );

        let access_token = service_auth.get_access_token(user_email).await?;

        let mut all_changes = Vec::new();
        let mut current_token = start_page_token.to_string();

        loop {
            let response = self
                .drive_client
                .list_changes(&access_token, &current_token)
                .await?;

            all_changes.extend(response.changes);

            if ctx.is_cancelled() {
                info!(
                    "Sync {} cancelled during changes listing for user {}",
                    sync_run_id, user_email
                );
                break;
            }

            match response.next_page_token {
                Some(token) => current_token = token,
                None => break,
            }
        }

        info!(
            "Incremental sync found {} changes for user {}",
            all_changes.len(),
            user_email
        );

        let mut file_batch = Vec::new();
        let mut total_scanned = 0;
        let mut total_updated = 0;
        const BATCH_SIZE: usize = 200;

        for change in all_changes {
            let is_removed = change.removed.unwrap_or(false);

            if is_removed {
                if let Some(file_id) = &change.file_id {
                    info!(
                        "File {} was removed (incremental), publishing deletion",
                        file_id
                    );
                    self.publish_deletion_event(ctx, file_id).await?;
                }
                continue;
            }

            if let Some(file) = change.file {
                if !self.should_index_file(&file) {
                    continue;
                }

                file_batch.push(UserFile {
                    user_email: Arc::new(user_email.to_string()),
                    file,
                });

                if file_batch.len() >= BATCH_SIZE {
                    let (scanned, updated) = self
                        .process_file_batch(
                            file_batch.clone(),
                            source_id,
                            sync_run_id,
                            ctx,
                            service_auth.clone(),
                        )
                        .await?;
                    total_scanned += scanned;
                    total_updated += updated;
                    file_batch.clear();
                }
            }
        }

        if !file_batch.is_empty() {
            let (scanned, updated) = self
                .process_file_batch(
                    file_batch,
                    source_id,
                    sync_run_id,
                    ctx,
                    service_auth.clone(),
                )
                .await?;
            total_scanned += scanned;
            total_updated += updated;
        }

        info!(
            "Completed incremental Drive sync for user {}: {} scanned, {} updated",
            user_email, total_scanned, total_updated
        );
        Ok((total_scanned, total_updated))
    }

    async fn process_file_batch(
        &self,
        files: Vec<UserFile>,
        source_id: &str,
        sync_run_id: &str,
        ctx: &SyncContext,
        service_auth: Arc<GoogleAuth>,
    ) -> Result<(usize, usize)> {
        info!("Processing batch of {} files", files.len());

        // (scanned, updated): scanned counts files we read content from
        // (regardless of store/emit outcome); updated counts files we
        // successfully emitted as events.
        let mut scanned = 0;
        let mut updated = 0;

        let sync_run_id_owned = sync_run_id.to_string();
        let source_id_owned = source_id.to_string();

        let tasks = files.into_iter().map(|user_file| {
            let service_auth = service_auth.clone();
            let source_id = source_id_owned.clone();
            let sync_run_id = sync_run_id_owned.clone();
            let drive_client = self.drive_client.clone();

            async move {
                debug!(
                    "Processing file: {} ({}) for user: {}",
                    user_file.file.name, user_file.file.id, user_file.user_email
                );

                let result = drive_client
                    .get_file_content(&service_auth, &user_file.user_email, &user_file.file)
                    .await
                    .with_context(|| {
                        format!(
                            "Getting content for file {} ({})",
                            user_file.file.name, user_file.file.id
                        )
                    });

                match result {
                    Ok(file_content) => {
                        let store_result = match file_content {
                            FileContent::Text(ref text) if text.is_empty() => {
                                debug!("File {} has empty content, skipping", user_file.file.name);
                                return (1, 0);
                            }
                            FileContent::Text(text) => ctx.store_content(&text).await,
                            FileContent::Binary {
                                data,
                                mime_type,
                                filename,
                            } => {
                                ctx.extract_and_store_content(data, &mime_type, Some(&filename))
                                    .await
                            }
                        };
                        match store_result {
                            Ok(content_id) => {
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
                                        warn!(
                                            "Failed to resolve path for file {}: {}",
                                            user_file.file.name, e
                                        );
                                        Some(format!("/{}", user_file.file.name))
                                    }
                                };

                                let event = user_file.file.to_connector_event(
                                    &sync_run_id,
                                    &source_id,
                                    &content_id,
                                    file_path,
                                    Some(&user_file.user_email),
                                );

                                match ctx.emit_event(event).await {
                                    Ok(_) => (1, 1),
                                    Err(e) => {
                                        error!(
                                            "Failed to queue event for file {}: {:?}",
                                            user_file.file.name, e
                                        );
                                        (1, 0)
                                    }
                                }
                            }
                            Err(e) => {
                                error!(
                                    "Failed to store content for file {}: {}",
                                    user_file.file.name, e
                                );
                                (1, 0)
                            }
                        }
                    }
                    Err(e) => {
                        warn!(
                            "Failed to get content for file {} ({}): {:?}",
                            user_file.file.name, user_file.file.id, e
                        );
                        (1, 0)
                    }
                }
            }
        });

        let results = futures::future::join_all(tasks).await;
        for (s, u) in results {
            scanned += s;
            updated += u;
        }

        // Push counts to the manager. Note: counts can over-count on resume
        // since save_connector_state only fires per-user; an in-flight batch
        // re-runs after crash. Counts are advisory progress, not exact.
        if scanned > 0 {
            ctx.increment_scanned(scanned as i32).await?;
        }
        if updated > 0 {
            ctx.increment_updated(updated as i32).await?;
        }

        info!(
            "Batch processing complete: {} scanned, {} updated",
            scanned, updated
        );
        Ok((scanned, updated))
    }

    async fn sync_drive_source_internal(
        &self,
        source: &Source,
        service_creds: &ServiceCredential,
        sync_type: SyncType,
        existing_state: GoogleConnectorState,
        ctx: &SyncContext,
    ) -> Result<GoogleConnectorState> {
        let sync_run_id = ctx.sync_run_id();

        let service_auth = Arc::new(self.create_auth(service_creds, source.source_type).await?);

        // Calculate cutoff date for filtering
        let (drive_cutoff_date, _gmail_cutoff_date) = self.get_cutoff_date()?;
        info!("Using Drive cutoff date: {}", drive_cutoff_date);

        // Build user list: single OAuth user or all domain users
        let user_emails: Vec<String> = if service_auth.is_oauth() {
            let email = service_auth
                .oauth_user_email()
                .ok_or_else(|| anyhow::anyhow!("OAuth auth missing user_email"))?
                .to_string();
            info!("OAuth Drive sync for single user: {}", email);
            vec![email]
        } else {
            let domain = crate::auth::get_domain_from_credentials(service_creds)?;
            let user_email = ctx.get_user_email_for_source().await
                .map_err(|e| anyhow::anyhow!("Failed to get user email for source {}: {}. Make sure the source has a valid creator.", source.id, e))?;

            info!("Listing all users in domain: {}", domain);
            info!("Using user email: {}", user_email);
            let admin_access_token = service_auth.get_access_token(&user_email).await
                .map_err(|e| anyhow::anyhow!("Failed to get access token for user {}: {}. Make sure the user is a super-admin and the service account has domain-wide delegation enabled.", user_email, e))?;
            let all_users = self
                .admin_client
                .list_all_users(&admin_access_token, &domain)
                .await?;
            info!("Found {} users in domain {}", all_users.len(), domain);

            let filtered: Vec<String> = all_users
                .into_iter()
                .filter(|user| source.should_index_user(&user.primary_email))
                .map(|user| user.primary_email)
                .collect();
            info!("After filtering: {} users will be indexed", filtered.len());
            filtered
        };

        let is_incremental = matches!(sync_type, SyncType::Incremental);

        let webhook_channel_id = existing_state.webhook_channel_id.clone();
        let webhook_resource_id = existing_state.webhook_resource_id.clone();
        let webhook_expires_at = existing_state.webhook_expires_at;
        let gmail_history_ids = existing_state.gmail_history_ids.clone();
        let old_page_tokens = existing_state.drive_page_tokens.unwrap_or_default();
        let mut new_page_tokens: HashMap<String, String> = HashMap::new();

        info!(
            "Starting user processing for {} users (Drive, incremental={})",
            user_emails.len(),
            is_incremental
        );

        let mut total_scanned = 0;
        let mut total_updated = 0;
        let mut errors = 0;

        for cur_user_email in &user_emails {
            if ctx.is_cancelled() {
                info!("Sync {} cancelled, stopping Drive sync early", sync_run_id);
                break;
            }

            match service_auth.get_access_token(cur_user_email).await {
                Ok(access_token) => {
                    info!("Processing user: {}", cur_user_email);

                    let stored_page_token = old_page_tokens.get(cur_user_email.as_str());
                    let use_incremental = is_incremental && stored_page_token.is_some();

                    let result = if use_incremental {
                        let start_token = stored_page_token.unwrap();
                        info!(
                            "Using incremental Drive sync for user {} from pageToken {}",
                            cur_user_email, start_token
                        );
                        match self
                            .sync_drive_for_user_incremental(
                                &cur_user_email,
                                service_auth.clone(),
                                &source.id,
                                sync_run_id,
                                ctx,
                                start_token,
                            )
                            .await
                        {
                            Ok(result) => Ok(result),
                            Err(e) => {
                                warn!(
                                    error = ?e,
                                    user = %cur_user_email,
                                    "Incremental drive sync failed."
                                );
                                Err(e).with_context(|| {
                                    format!(
                                        "Incremental drive sync failed for {} at pageToken {}",
                                        cur_user_email, start_token
                                    )
                                })
                            }
                        }
                    } else {
                        self.sync_drive_for_user(
                            &cur_user_email,
                            service_auth.clone(),
                            &source.id,
                            sync_run_id,
                            ctx,
                            Some(&drive_cutoff_date),
                        )
                        .await
                    };

                    let user_succeeded = match result {
                        Ok((scanned, updated)) => {
                            total_scanned += scanned;
                            total_updated += updated;
                            info!(
                                "User {} Drive sync completed: {} scanned, {} updated",
                                cur_user_email, scanned, updated
                            );
                            true
                        }
                        Err(e) => {
                            error!("Failed to process Drive for user {}: {}", cur_user_email, e);
                            errors += 1;
                            false
                        }
                    };

                    // Capture the watermark AFTER the user's files are fully
                    // processed. If we captured before and crashed mid-user,
                    // resume would advance past the user's unprocessed files.
                    if user_succeeded {
                        match self.drive_client.get_start_page_token(&access_token).await {
                            Ok(token) => {
                                new_page_tokens.insert(cur_user_email.clone(), token);
                            }
                            Err(e) => {
                                warn!(
                                    "Failed to get start page token for user {}: {}",
                                    cur_user_email, e
                                );
                            }
                        }

                        // Per-user checkpoint: persist progress so a crash
                        // before later users finish doesn't lose this user's
                        // work. State-save errors are fatal (silent loss is
                        // worse than a failed sync).
                        let checkpoint_state = GoogleConnectorState {
                            webhook_channel_id: webhook_channel_id.clone(),
                            webhook_resource_id: webhook_resource_id.clone(),
                            webhook_expires_at,
                            gmail_history_ids: gmail_history_ids.clone(),
                            drive_page_tokens: if new_page_tokens.is_empty() {
                                None
                            } else {
                                Some(new_page_tokens.clone())
                            },
                        };
                        ctx.save_connector_state(serde_json::to_value(&checkpoint_state)?)
                            .await
                            .with_context(|| {
                                format!(
                                    "Failed to checkpoint Drive state after user {}",
                                    cur_user_email
                                )
                            })?;
                    }
                }
                Err(e) => {
                    warn!("Failed to get access token for user {}: {}. This user may not have Drive access.", cur_user_email, e);
                    errors += 1;
                }
            }
        }

        info!(
            "User processing complete. Total: {} scanned, {} updated, {} errors",
            total_scanned, total_updated, errors
        );

        info!(
            "Sync completed for source {}: {} scanned, {} updated",
            source.id, total_scanned, total_updated
        );

        // Clear folder cache to free memory after sync
        self.folder_cache.clear();

        info!("Completed sync for source: {}", source.id);

        Ok(GoogleConnectorState {
            webhook_channel_id,
            webhook_resource_id,
            webhook_expires_at,
            gmail_history_ids,
            drive_page_tokens: if new_page_tokens.is_empty() {
                None
            } else {
                Some(new_page_tokens)
            },
        })
    }

    async fn sync_gmail_source_internal(
        &self,
        source: &Source,
        service_creds: &ServiceCredential,
        sync_type: SyncType,
        existing_state: GoogleConnectorState,
        known_groups: HashSet<String>,
        ctx: &SyncContext,
    ) -> Result<GoogleConnectorState> {
        let sync_run_id = ctx.sync_run_id();

        let service_auth = Arc::new(self.create_auth(service_creds, source.source_type).await?);

        let (_drive_cutoff_date, gmail_cutoff_date) = self.get_cutoff_date()?;
        info!("Using Gmail cutoff date: {}", gmail_cutoff_date);

        // Build user list: single OAuth user or all domain users
        let user_emails: Vec<String> = if service_auth.is_oauth() {
            let email = service_auth
                .oauth_user_email()
                .ok_or_else(|| anyhow::anyhow!("OAuth auth missing user_email"))?
                .to_string();
            info!("OAuth Gmail sync for single user: {}", email);
            vec![email]
        } else {
            let domain = crate::auth::get_domain_from_credentials(service_creds)?;
            let user_email = ctx.get_user_email_for_source().await
                .map_err(|e| anyhow::anyhow!("Failed to get user email for source {}: {}. Make sure the source has a valid creator.", source.id, e))?;

            info!("Listing all users in domain: {}", domain);
            info!("Using user email: {}", user_email);
            let admin_access_token = service_auth.get_access_token(&user_email).await
                .map_err(|e| anyhow::anyhow!("Failed to get access token for user {}: {}. Make sure the user is a super-admin and the service account has domain-wide delegation enabled.", user_email, e))?;
            let all_users = self
                .admin_client
                .list_all_users(&admin_access_token, &domain)
                .await?;
            info!("Found {} users in domain {}", all_users.len(), domain);

            let filtered: Vec<String> = all_users
                .into_iter()
                .filter(|user| source.should_index_user(&user.primary_email))
                .map(|user| user.primary_email)
                .collect();
            info!("After filtering: {} users will be indexed", filtered.len());
            filtered
        };

        let is_incremental = matches!(sync_type, SyncType::Incremental);

        let webhook_channel_id = existing_state.webhook_channel_id.clone();
        let webhook_resource_id = existing_state.webhook_resource_id.clone();
        let webhook_expires_at = existing_state.webhook_expires_at;
        let drive_page_tokens = existing_state.drive_page_tokens.clone();
        let old_history_ids = existing_state.gmail_history_ids.unwrap_or_default();
        let mut new_history_ids: HashMap<String, String> = HashMap::new();

        let processed_threads = Arc::new(std::sync::Mutex::new(HashSet::<String>::new()));
        let known_groups = Arc::new(known_groups);

        info!(
            "Starting sequential user processing for {} users (Gmail, incremental={})",
            user_emails.len(),
            is_incremental
        );

        let mut total_processed = 0;
        let mut total_updated = 0;

        for cur_user_email in &user_emails {
            if ctx.is_cancelled() {
                info!("Sync {} cancelled, stopping Gmail sync early", sync_run_id);
                break;
            }

            match service_auth.get_access_token(cur_user_email).await {
                Ok(_token) => {
                    info!("Processing user: {}", cur_user_email);

                    let stored_history_id = old_history_ids.get(cur_user_email.as_str());
                    let use_incremental = is_incremental && stored_history_id.is_some();

                    let result = if use_incremental {
                        let start_id = stored_history_id.unwrap();
                        info!(
                            "Using incremental Gmail sync for user {} from historyId {}",
                            cur_user_email, start_id
                        );
                        match self
                            .sync_gmail_for_user_incremental(
                                &cur_user_email,
                                service_auth.clone(),
                                &source.id,
                                sync_run_id,
                                ctx,
                                start_id,
                                processed_threads.clone(),
                                known_groups.clone(),
                            )
                            .await
                        {
                            Ok(result) => Ok(result),
                            Err(e) => {
                                let err_str = format!("{}", e);
                                if err_str.contains("HTTP 404") {
                                    warn!(
                                        "History expired for user {}, falling back to full sync",
                                        cur_user_email
                                    );
                                    self.sync_gmail_for_user(
                                        &cur_user_email,
                                        service_auth.clone(),
                                        ctx,
                                        processed_threads.clone(),
                                        Some(&gmail_cutoff_date),
                                        known_groups.clone(),
                                    )
                                    .await
                                } else {
                                    Err(e)
                                }
                            }
                        }
                    } else {
                        self.sync_gmail_for_user(
                            &cur_user_email,
                            service_auth.clone(),
                            ctx,
                            processed_threads.clone(),
                            Some(&gmail_cutoff_date),
                            known_groups.clone(),
                        )
                        .await
                    };

                    let user_succeeded = match result {
                        Ok((processed, updated)) => {
                            total_processed += processed;
                            total_updated += updated;
                            info!(
                                "User {} Gmail sync completed: {} processed, {} updated",
                                cur_user_email, processed, updated
                            );
                            true
                        }
                        Err(e) => {
                            error!("Failed to process Gmail for user {}: {}", cur_user_email, e);
                            false
                        }
                    };

                    // Capture the historyId watermark AFTER the user finishes
                    // and checkpoint immediately. Capturing before processing
                    // would let resume skip past unprocessed history on crash.
                    if user_succeeded {
                        match self
                            .gmail_client
                            .get_profile(&service_auth, &cur_user_email)
                            .await
                        {
                            Ok(profile) => {
                                new_history_ids.insert(cur_user_email.clone(), profile.history_id);
                            }
                            Err(e) => {
                                warn!(
                                    "Failed to get Gmail profile for user {}: {}",
                                    cur_user_email, e
                                );
                            }
                        }

                        let checkpoint_state = GoogleConnectorState {
                            webhook_channel_id: webhook_channel_id.clone(),
                            webhook_resource_id: webhook_resource_id.clone(),
                            webhook_expires_at,
                            gmail_history_ids: if new_history_ids.is_empty() {
                                None
                            } else {
                                Some(new_history_ids.clone())
                            },
                            drive_page_tokens: drive_page_tokens.clone(),
                        };
                        ctx.save_connector_state(serde_json::to_value(&checkpoint_state)?)
                            .await
                            .with_context(|| {
                                format!(
                                    "Failed to checkpoint Gmail state after user {}",
                                    cur_user_email
                                )
                            })?;
                    }
                }
                Err(e) => {
                    warn!("Failed to get access token for user {}: {}. This user may not have Gmail access.", cur_user_email, e);
                }
            }
        }

        info!(
            "Gmail sync completed for source {}: {} total processed, {} total updated",
            source.id, total_processed, total_updated
        );

        info!("Completed Gmail sync for source: {}", source.id);

        Ok(GoogleConnectorState {
            webhook_channel_id,
            webhook_resource_id,
            webhook_expires_at,
            gmail_history_ids: if new_history_ids.is_empty() {
                None
            } else {
                Some(new_history_ids)
            },
            drive_page_tokens,
        })
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

    async fn publish_deletion_event(&self, ctx: &SyncContext, document_id: &str) -> Result<()> {
        let event = ConnectorEvent::DocumentDeleted {
            sync_run_id: ctx.sync_run_id().to_string(),
            source_id: ctx.source_id().to_string(),
            document_id: document_id.to_string(),
        };
        ctx.emit_event(event).await
    }

    async fn get_service_credentials(&self, source_id: &str) -> Result<ServiceCredential> {
        let creds = self
            .sdk_client
            .get_credentials(source_id)
            .await
            .context("Failed to fetch credentials via SDK")?;

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

    /// Create GoogleAuth from credentials, branching on auth_type (JWT vs OAuth)
    pub async fn create_auth(
        &self,
        creds: &ServiceCredential,
        source_type: SourceType,
    ) -> Result<GoogleAuth> {
        match creds.auth_type {
            AuthType::OAuth => {
                let access_token = creds
                    .credentials
                    .get("access_token")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string();

                let refresh_token = creds
                    .credentials
                    .get("refresh_token")
                    .and_then(|v| v.as_str())
                    .ok_or_else(|| anyhow::anyhow!("Missing refresh_token in OAuth credentials"))?
                    .to_string();

                let expires_at = creds
                    .credentials
                    .get("expires_at")
                    .and_then(|v| v.as_i64())
                    .unwrap_or(0);

                let user_email = creds
                    .credentials
                    .get("user_email")
                    .and_then(|v| v.as_str())
                    .or(creds.principal_email.as_deref())
                    .ok_or_else(|| anyhow::anyhow!("Missing user_email in OAuth credentials"))?
                    .to_string();

                // Fetch connector config for OAuth client_id/secret
                let connector_config = self
                    .sdk_client
                    .get_connector_config("google")
                    .await
                    .context("Failed to fetch Google connector config for OAuth")?;

                let client_id = connector_config
                    .get("oauth_client_id")
                    .and_then(|v| v.as_str())
                    .ok_or_else(|| {
                        anyhow::anyhow!("Missing oauth_client_id in Google connector config")
                    })?
                    .to_string();

                let client_secret = connector_config
                    .get("oauth_client_secret")
                    .and_then(|v| v.as_str())
                    .ok_or_else(|| {
                        anyhow::anyhow!("Missing oauth_client_secret in Google connector config")
                    })?
                    .to_string();

                let oauth_auth = OAuthAuth::new(
                    access_token,
                    refresh_token,
                    expires_at,
                    user_email,
                    client_id,
                    client_secret,
                )?;

                Ok(GoogleAuth::OAuth(oauth_auth))
            }
            _ => {
                // Default: JWT / service account
                let sa = crate::auth::create_service_auth(creds, source_type)?;
                Ok(GoogleAuth::ServiceAccount(sa))
            }
        }
    }

    async fn get_user_email_from_source(&self, source_id: &str) -> Result<String> {
        self.sdk_client
            .get_user_email_for_source(source_id)
            .await
            .context("Failed to get user email via SDK")
    }

    pub async fn handle_webhook_notification(
        &self,
        notification: WebhookNotification,
    ) -> Result<()> {
        info!(
            "Handling webhook notification for channel {}, state: {}",
            notification.channel_id, notification.resource_state
        );

        let source_id = match &notification.source_id {
            Some(id) => id.clone(),
            None => {
                warn!(
                    "Received webhook notification without source_id token for channel {}",
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
            "add" | "update" | "remove" | "trash" | "untrash" | "change" => {
                let now = Instant::now();
                let mut entry = self
                    .webhook_debounce
                    .entry(source_id.clone())
                    .or_insert_with(|| WebhookDebounce {
                        last_received: now,
                        last_event_type: notification.resource_state.clone(),
                        count: 0,
                    });
                entry.last_received = now;
                entry.last_event_type = notification.resource_state.clone();
                entry.count += 1;

                info!(
                    "Buffered webhook event for source {} (state: {}, buffered_count: {})",
                    source_id, notification.resource_state, entry.count
                );

                self.webhook_notify.notify_one();
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

    /// Background loop that coalesces rapid webhook notifications.
    /// Waits until 10 minutes of quiet time per source, then fires one
    /// `notify_webhook` call for all buffered events.
    pub async fn run_webhook_processor(self: &Arc<Self>) {
        const POLL_INTERVAL: Duration = Duration::from_secs(30);
        let debounce_duration =
            Duration::from_millis(self.debounce_duration_ms.load(Ordering::Relaxed));

        loop {
            tokio::select! {
                _ = self.webhook_notify.notified() => {}
                _ = tokio::time::sleep(POLL_INTERVAL) => {}
            }

            let now = Instant::now();
            let mut expired: Vec<(String, String, u32)> = Vec::new();

            // Collect expired entries
            for entry in self.webhook_debounce.iter() {
                if now.duration_since(entry.last_received) >= debounce_duration {
                    expired.push((
                        entry.key().clone(),
                        entry.last_event_type.clone(),
                        entry.count,
                    ));
                }
            }

            // Notify first, only remove on success
            for (source_id, event_type, count) in expired {
                info!(
                    "Debounce expired for source {} ({} buffered events), notifying connector-manager",
                    source_id, count
                );

                match self
                    .sdk_client
                    .notify_webhook(&source_id, &event_type)
                    .await
                {
                    Ok(sync_run_id) => {
                        self.webhook_debounce.remove(&source_id);
                        info!(
                            "Connector-manager created sync run {} for debounced webhook (source: {})",
                            sync_run_id, source_id
                        );
                    }
                    Err(e) => {
                        error!(
                            "Failed to notify connector-manager for debounced webhook (source: {}): {}",
                            source_id, e
                        );
                    }
                }
            }
        }
    }

    /// Ensure a webhook is registered for a source.
    /// No-op if webhook_url is None. Logs but never propagates errors.
    pub async fn ensure_webhook_registered(&self, source_id: &str) {
        let Some(ref webhook_url) = self.webhook_url else {
            return;
        };

        info!("Ensuring webhook registered for source {}", source_id);
        if let Err(e) = self
            .register_webhook_for_source(source_id, webhook_url.clone())
            .await
        {
            error!("Failed to register webhook for source {}: {}", source_id, e);
        }
    }

    pub async fn register_webhook_for_source(
        &self,
        source_id: &str,
        webhook_url: String,
    ) -> Result<WebhookChannelResponse> {
        // Capture old channel info before registering the new one
        let old_channel =
            if let Ok(Some(raw_state)) = self.sdk_client.get_connector_state(source_id).await {
                let state: GoogleConnectorState =
                    serde_json::from_value(raw_state).unwrap_or_else(|e| {
                        warn!(
                            "Failed to parse connector state for source {}: {}",
                            source_id, e
                        );
                        GoogleConnectorState::default()
                    });
                match (&state.webhook_channel_id, &state.webhook_resource_id) {
                    (Some(ch), Some(res)) => Some((ch.clone(), res.clone())),
                    _ => None,
                }
            } else {
                None
            };

        let service_creds = self.get_service_credentials(source_id).await?;
        let service_auth =
            crate::auth::create_service_auth(&service_creds, SourceType::GoogleDrive)?;
        let user_email = self.get_user_email_from_source(source_id).await
            .map_err(|e| anyhow::anyhow!("Failed to get user email for source {}: {}. Make sure the source has a valid creator.", source_id, e))?;
        let access_token = service_auth.get_access_token(&user_email).await?;

        let start_page_token = self
            .drive_client
            .get_start_page_token(&access_token)
            .await?;

        let webhook_channel = WebhookChannel::new(webhook_url.clone(), source_id);

        let webhook_response = self
            .drive_client
            .register_changes_webhook(&access_token, &webhook_channel, &start_page_token)
            .await?;

        let expires_at = webhook_response
            .expiration
            .as_ref()
            .and_then(|exp| exp.parse::<i64>().ok());

        // Store new channel info in connector_state, preserving existing gmail_history_ids
        let existing_state: GoogleConnectorState =
            if let Ok(Some(raw)) = self.sdk_client.get_connector_state(source_id).await {
                serde_json::from_value(raw).unwrap_or_default()
            } else {
                GoogleConnectorState::default()
            };
        let webhook_state = GoogleConnectorState {
            webhook_channel_id: Some(webhook_response.id.clone()),
            webhook_resource_id: Some(webhook_response.resource_id.clone()),
            webhook_expires_at: expires_at,
            gmail_history_ids: existing_state.gmail_history_ids,
            drive_page_tokens: existing_state.drive_page_tokens,
        };
        self.sdk_client
            .save_connector_state(source_id, serde_json::to_value(&webhook_state)?)
            .await?;

        info!(
            "Successfully registered webhook for source {}: channel_id={}, resource_id={}",
            source_id, webhook_response.id, webhook_response.resource_id
        );

        // Stop old channel after the new one is active to avoid gaps in coverage
        if let Some((old_channel_id, old_resource_id)) = old_channel {
            info!(
                "Stopping old webhook channel {} for source {}",
                old_channel_id, source_id
            );
            if let Err(e) = self
                .stop_webhook_for_source(source_id, &old_channel_id, &old_resource_id)
                .await
            {
                warn!("Failed to stop old webhook channel: {}", e);
            }
        }

        Ok(webhook_response)
    }

    pub async fn stop_webhook_for_source(
        &self,
        source_id: &str,
        channel_id: &str,
        resource_id: &str,
    ) -> Result<()> {
        let service_creds = self.get_service_credentials(source_id).await?;
        let service_auth =
            crate::auth::create_service_auth(&service_creds, SourceType::GoogleDrive)?;
        let user_email = self.get_user_email_from_source(source_id).await
            .map_err(|e| anyhow::anyhow!("Failed to get user email for source {}: {}. Make sure the source has a valid creator.", source_id, e))?;
        let access_token = service_auth.get_access_token(&user_email).await?;

        self.drive_client
            .stop_webhook_channel(&access_token, channel_id, resource_id)
            .await?;

        info!(
            "Successfully stopped webhook for source {}: channel_id={}",
            source_id, channel_id
        );
        Ok(())
    }

    async fn resolve_file_path(
        &self,
        auth: &GoogleAuth,
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
        auth: &GoogleAuth,
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
        service_auth: Arc<GoogleAuth>,
        ctx: &SyncContext,
        processed_threads: Arc<std::sync::Mutex<HashSet<String>>>,
        created_after: Option<&str>,
        known_groups: Arc<HashSet<String>>,
    ) -> Result<(usize, usize)> {
        info!("Processing Gmail for user: {}", user_email);

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
                    Some("-in:chats"),
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
                let page_thread_count = threads.len();
                debug!(
                    "Got {} threads in this page for user {}",
                    page_thread_count, user_email
                );

                for thread_info in threads {
                    user_threads.push(thread_info.id);
                }

                // Update scanned count for this page via SDK
                ctx.increment_scanned(page_thread_count as i32).await?;
            }

            // Check for cancellation
            if ctx.is_cancelled() {
                info!(
                    "Sync {} cancelled, stopping Gmail thread listing for user {}",
                    ctx.sync_run_id(),
                    user_email
                );
                break;
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

        self.process_gmail_threads(
            user_threads,
            user_email,
            service_auth,
            ctx,
            processed_threads,
            known_groups,
        )
        .await
    }

    async fn sync_gmail_for_user_incremental(
        &self,
        user_email: &str,
        service_auth: Arc<GoogleAuth>,
        source_id: &str,
        sync_run_id: &str,
        ctx: &SyncContext,
        start_history_id: &str,
        processed_threads: Arc<std::sync::Mutex<HashSet<String>>>,
        known_groups: Arc<HashSet<String>>,
    ) -> Result<(usize, usize)> {
        info!(
            "Processing incremental Gmail sync for user {} from historyId {}",
            user_email, start_history_id
        );

        let mut changed_thread_ids = HashSet::new();
        let mut page_token: Option<String> = None;

        loop {
            let response = self
                .gmail_client
                .list_history(
                    &service_auth,
                    user_email,
                    start_history_id,
                    Some(500),
                    page_token.as_deref(),
                )
                .await?;

            if let Some(history_records) = response.history {
                for record in history_records {
                    if let Some(messages) = record.messages {
                        for msg in messages {
                            changed_thread_ids.insert(msg.thread_id);
                        }
                    }
                    if let Some(added) = record.messages_added {
                        for item in added {
                            changed_thread_ids.insert(item.message.thread_id);
                        }
                    }
                    if let Some(deleted) = record.messages_deleted {
                        for item in deleted {
                            changed_thread_ids.insert(item.message.thread_id);
                        }
                    }
                    if let Some(label_added) = record.labels_added {
                        for item in label_added {
                            changed_thread_ids.insert(item.message.thread_id);
                        }
                    }
                    if let Some(label_removed) = record.labels_removed {
                        for item in label_removed {
                            changed_thread_ids.insert(item.message.thread_id);
                        }
                    }
                }
            }

            if ctx.is_cancelled() {
                info!(
                    "Sync {} cancelled during history listing for user {}",
                    sync_run_id, user_email
                );
                break;
            }

            page_token = response.next_page_token;
            if page_token.is_none() {
                break;
            }
        }

        let thread_ids: Vec<String> = changed_thread_ids.into_iter().collect();
        info!(
            "Incremental sync found {} changed threads for user {}",
            thread_ids.len(),
            user_email
        );

        ctx.increment_scanned(thread_ids.len() as i32).await?;

        self.process_gmail_threads(
            thread_ids,
            user_email,
            service_auth,
            ctx,
            processed_threads,
            known_groups,
        )
        .await
    }

    async fn process_gmail_threads(
        &self,
        thread_ids: Vec<String>,
        user_email: &str,
        service_auth: Arc<GoogleAuth>,
        ctx: &SyncContext,
        processed_threads: Arc<std::sync::Mutex<HashSet<String>>>,
        known_groups: Arc<HashSet<String>>,
    ) -> Result<(usize, usize)> {
        let mut total_processed = 0;
        let mut total_updated = 0;
        let mut total_deduped = 0usize;
        let mut total_failed = 0usize;
        let total_listed = thread_ids.len();
        const THREAD_BATCH_SIZE: usize = 50;

        for chunk in thread_ids.chunks(THREAD_BATCH_SIZE) {
            if ctx.is_cancelled() {
                info!(
                    "Sync {} cancelled, stopping Gmail thread processing for user {}",
                    ctx.sync_run_id(),
                    user_email
                );
                break;
            }

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
                    total_deduped += 1;
                    continue;
                }

                unprocessed_threads.push(thread_id.clone());
            }

            if unprocessed_threads.is_empty() {
                continue;
            }

            {
                let mut processed_guard = processed_threads.lock().unwrap();
                for thread_id in &unprocessed_threads {
                    processed_guard.insert(thread_id.clone());
                }
            }

            debug!("Processing batch of {} threads", unprocessed_threads.len());

            // Fetch batch with retry on 429 (up to 3 attempts with exponential backoff).
            // Each attempt drains its successes by-value into the per-thread processor
            // immediately — never accumulates GmailThreadResponse bodies across
            // attempts, which previously caused the OOM during full sync.
            let mut threads_to_fetch = unprocessed_threads;
            let max_retries = 3;
            let mut saw_rate_limit = false;
            let mut chunk_updated: usize = 0;

            for attempt in 0..=max_retries {
                if threads_to_fetch.is_empty() {
                    break;
                }

                if attempt > 0 {
                    let delay = Duration::from_secs(2u64.pow(attempt as u32));
                    warn!(
                        "Retrying {} rate-limited threads (attempt {}/{}, waiting {:?})",
                        threads_to_fetch.len(),
                        attempt,
                        max_retries,
                        delay
                    );
                    tokio::time::sleep(delay).await;
                }

                let batch_results = match self
                    .gmail_client
                    .batch_get_threads(
                        &service_auth,
                        user_email,
                        &threads_to_fetch,
                        MessageFormat::Full,
                    )
                    .await
                    .with_context(|| {
                        format!("Failed to get Gmail threads batch for user {}", user_email)
                    }) {
                    Ok(results) => results,
                    Err(e) => {
                        warn!("Failed to fetch thread batch: {}", e);
                        break;
                    }
                };

                let mut rate_limited_ids = Vec::new();
                let fetched_ids = std::mem::take(&mut threads_to_fetch);
                for (i, result) in batch_results.into_iter().enumerate() {
                    let thread_id = fetched_ids[i].clone();
                    match result {
                        BatchThreadResult::Success(response) => {
                            total_processed += 1;
                            let updated = self
                                .process_gmail_thread(
                                    &thread_id,
                                    response,
                                    user_email,
                                    &service_auth,
                                    ctx,
                                    &known_groups,
                                )
                                .await;
                            if updated {
                                total_updated += 1;
                                chunk_updated += 1;
                            }
                        }
                        BatchThreadResult::RateLimited => {
                            rate_limited_ids.push(thread_id);
                        }
                        BatchThreadResult::Failed(e) => {
                            total_failed += 1;
                            warn!("Failed to fetch thread {}: {}", thread_id, e);
                        }
                    }
                }

                if !rate_limited_ids.is_empty() {
                    saw_rate_limit = true;
                }
                threads_to_fetch = rate_limited_ids;
            }

            if !threads_to_fetch.is_empty() {
                warn!(
                    "Gave up on {} threads after {} retries for user {}",
                    threads_to_fetch.len(),
                    max_retries,
                    user_email
                );
            }

            // Push the chunk's contribution to documents_updated to the manager
            // so a mid-sync crash doesn't lose it. Per-page increment_scanned
            // already covers the scanned counter.
            if chunk_updated > 0 {
                ctx.increment_updated(chunk_updated as i32).await?;
            }

            // Adaptive backpressure: if this batch had 429s, pause before next batch
            if saw_rate_limit {
                debug!("Rate limit hit — pausing 3s before next batch");
                tokio::time::sleep(Duration::from_secs(3)).await;
            }
        }

        info!(
            "Completed Gmail processing for user {}: {} listed, {} indexed, {} updated \
            (skipped: {} deduped across users, {} failed/inaccessible)",
            user_email, total_listed, total_processed, total_updated, total_deduped, total_failed
        );

        Ok((total_processed, total_updated))
    }

    /// Process a single Gmail thread response by-value: build the GmailThread,
    /// emit the thread document and its attachments. Returns true if the
    /// thread was emitted as an update. Consumes `response` so the response
    /// body can drop as soon as the messages are moved into `gmail_thread`.
    async fn process_gmail_thread(
        &self,
        thread_id: &str,
        response: crate::gmail::GmailThreadResponse,
        user_email: &str,
        service_auth: &Arc<GoogleAuth>,
        ctx: &SyncContext,
        known_groups: &HashSet<String>,
    ) -> bool {
        let mut gmail_thread = GmailThread::new(thread_id.to_string());
        for message in response.messages {
            gmail_thread.add_message(message);
        }

        if gmail_thread.total_messages == 0 {
            debug!("Gmail thread {} has no messages, skipping", thread_id);
            return false;
        }

        let thread_url = gmail_thread.message_id.as_ref().map(|mid| {
            let clean_id = mid.trim_start_matches('<').trim_end_matches('>');
            let encoded = urlencoding::encode(clean_id);
            format!(
                "https://mail.google.com/mail/#search/rfc822msgid%3A{}",
                encoded
            )
        });

        // Extract attachments and store their content first, so the thread
        // document can carry pointers to its attachments in metadata.extra.
        //
        // Within a thread, dedup by (filename, size): the same file forwarded
        // across multiple replies would otherwise produce one document per
        // occurrence, flooding the BM25 index with copies of identical content.
        //
        // We persist the canonical RFC 822 Message-ID (not Gmail's per-mailbox
        // messageId) so the attachment can be fetched from any participating
        // user's mailbox via `messages.list?q=rfc822msgid:<id>`.
        let mut stored_attachments: Vec<(ExtractedAttachment, String, String)> = Vec::new();
        let mut seen: HashSet<(String, u64)> = HashSet::new();
        for message in &gmail_thread.messages {
            let rfc822_msgid = match self
                .gmail_client
                .get_header_value(message, "Message-ID")
                .map(|raw| {
                    raw.trim()
                        .trim_start_matches('<')
                        .trim_end_matches('>')
                        .to_string()
                })
                .filter(|s| !s.is_empty())
            {
                Some(id) => id,
                None => {
                    warn!(
                        "Gmail message {} in thread {} has no Message-ID header; \
                         skipping its attachments (cannot be fetched without canonical id)",
                        message.id, thread_id
                    );
                    continue;
                }
            };

            let attachments = self
                .gmail_client
                .extract_attachments(
                    message,
                    service_auth,
                    user_email,
                    ctx.sdk_client(),
                    ctx.sync_run_id(),
                )
                .await;

            for att in attachments {
                if att.extracted_text.trim().is_empty() {
                    continue;
                }

                if !seen.insert((att.filename.clone(), att.size)) {
                    debug!(
                        "Skipping duplicate attachment {} (size {}) in thread {}",
                        att.filename, att.size, thread_id
                    );
                    continue;
                }

                let att_content_id = match ctx.store_content(&att.extracted_text).await {
                    Ok(id) => id,
                    Err(e) => {
                        error!(
                            "Failed to store attachment content for {}: {}",
                            att.filename, e
                        );
                        continue;
                    }
                };

                stored_attachments.push((att, att_content_id, rfc822_msgid.clone()));
            }
        }

        let attachment_pointers: Vec<AttachmentPointer> = stored_attachments
            .iter()
            .map(|(att, _, rfc822_msgid)| AttachmentPointer {
                id: build_attachment_doc_id(rfc822_msgid, &att.filename, att.size),
                filename: att.filename.clone(),
                mime_type: att.mime_type.clone(),
                size: att.size,
            })
            .collect();

        let emit_result: Result<bool> = async {
            let content = gmail_thread
                .aggregate_content(&self.gmail_client, ctx.sdk_client(), ctx.sync_run_id())
                .await
                .context("aggregate content")?;
            if content.trim().is_empty() {
                debug!("Gmail thread {} has empty content, skipping", thread_id);
                return Ok(false);
            }
            let content_id = ctx.store_content(&content).await.context("store content")?;
            let event = gmail_thread
                .to_connector_event(
                    ctx.sync_run_id(),
                    ctx.source_id(),
                    &content_id,
                    known_groups,
                    &attachment_pointers,
                )
                .context("build connector event")?;
            ctx.emit_event(event).await.context("emit event")?;
            Ok(true)
        }
        .await;

        let updated = match emit_result {
            Ok(true) => {
                info!("Successfully queued Gmail thread {}", thread_id);
                true
            }
            Ok(false) => false,
            Err(e) => {
                error!("Failed to process Gmail thread {}: {:#}", thread_id, e);
                false
            }
        };

        let mut att_users = Vec::new();
        let mut att_groups = Vec::new();
        for participant in &gmail_thread.participants {
            if known_groups.contains(participant) {
                att_groups.push(participant.clone());
            } else {
                att_users.push(participant.clone());
            }
        }
        let att_permissions = DocumentPermissions {
            public: false,
            users: att_users,
            groups: att_groups,
        };

        for (att, att_content_id, rfc822_msgid) in stored_attachments {
            let att_doc_id = build_attachment_doc_id(&rfc822_msgid, &att.filename, att.size);

            let mut att_extra = HashMap::new();
            att_extra.insert("parent_thread_id".to_string(), json!(thread_id));

            let att_metadata = DocumentMetadata {
                title: Some(att.filename.clone()),
                author: None,
                created_at: None,
                updated_at: None,
                content_type: mime_type_to_content_type(&att.mime_type),
                mime_type: Some(att.mime_type.clone()),
                size: Some(att.size.to_string()),
                url: thread_url.clone(),
                path: Some(format!("/Gmail/{}/{}", gmail_thread.subject, att.filename)),
                extra: Some(att_extra),
            };

            let att_event = ConnectorEvent::DocumentCreated {
                sync_run_id: ctx.sync_run_id().to_string(),
                source_id: ctx.source_id().to_string(),
                document_id: att_doc_id.clone(),
                content_id: att_content_id,
                metadata: att_metadata,
                permissions: att_permissions.clone(),
                attributes: Some(HashMap::new()),
            };

            match ctx.emit_event(att_event).await {
                Ok(_) => debug!(
                    "Queued attachment {} for thread {}",
                    att.filename, thread_id
                ),
                Err(e) => error!(
                    "Failed to queue attachment {} for thread {}: {}",
                    att.filename, thread_id, e
                ),
            }
        }

        updated
    }

    /// Sync group memberships if this is a service-account (domain-wide) source.
    /// OAuth single-user sources don't have Admin API access, so we skip them.
    async fn maybe_sync_groups(
        &self,
        source: &Source,
        service_creds: &ServiceCredential,
        ctx: &SyncContext,
    ) -> HashSet<String> {
        let service_auth = match self.create_auth(service_creds, source.source_type).await {
            Ok(auth) => auth,
            Err(e) => {
                warn!("Failed to create auth for group sync: {}", e);
                return HashSet::new();
            }
        };

        // Only service-account (domain-wide) setups have Admin API access
        if service_auth.is_oauth() {
            debug!("Skipping group sync for OAuth source {}", source.id);
            return HashSet::new();
        }

        let domain = match crate::auth::get_domain_from_credentials(service_creds) {
            Ok(d) => d,
            Err(e) => {
                warn!("Failed to get domain for group sync: {}", e);
                return HashSet::new();
            }
        };

        let user_email = match ctx.get_user_email_for_source().await {
            Ok(email) => email,
            Err(e) => {
                warn!("Failed to get user email for group sync: {}", e);
                return HashSet::new();
            }
        };

        let access_token = match service_auth.get_access_token(&user_email).await {
            Ok(token) => token,
            Err(e) => {
                warn!("Failed to get access token for group sync: {}", e);
                return HashSet::new();
            }
        };

        match self
            .sync_groups(&source.id, ctx.sync_run_id(), &domain, &access_token)
            .await
        {
            Ok(group_emails) => group_emails,
            Err(e) => {
                warn!(
                    "Failed to sync group memberships: {}. Continuing with document sync.",
                    e
                );
                HashSet::new()
            }
        }
    }

    async fn sync_groups(
        &self,
        source_id: &str,
        sync_run_id: &str,
        domain: &str,
        access_token: &str,
    ) -> Result<HashSet<String>> {
        info!("Syncing group memberships for domain: {}", domain);

        let groups = self
            .admin_client
            .list_all_groups(access_token, domain)
            .await?;
        info!("Found {} groups in domain {}", groups.len(), domain);

        let mut group_emails: HashSet<String> = HashSet::new();
        let mut total_members = 0;
        for group in &groups {
            group_emails.insert(group.email.to_lowercase());

            let members = self
                .admin_client
                .list_all_group_members(access_token, &group.email)
                .await
                .unwrap_or_else(|e| {
                    warn!("Failed to list members for group {}: {}", group.email, e);
                    vec![]
                });

            let member_emails: Vec<String> = members
                .into_iter()
                .filter_map(|m| m.email)
                .map(|e| e.to_lowercase())
                .collect();

            total_members += member_emails.len();

            let event = ConnectorEvent::GroupMembershipSync {
                sync_run_id: sync_run_id.to_string(),
                source_id: source_id.to_string(),
                group_email: group.email.clone(),
                group_name: group.name.clone(),
                member_emails,
            };

            if let Err(e) = self
                .sdk_client
                .emit_event(sync_run_id, source_id, event)
                .await
            {
                warn!(
                    "Failed to emit group membership event for {}: {}",
                    group.email, e
                );
            }
        }

        info!(
            "Group sync complete: {} groups, {} total memberships",
            groups.len(),
            total_members
        );
        Ok(group_emails)
    }
}

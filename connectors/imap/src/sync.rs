use anyhow::{anyhow, Context, Result};
use dashmap::DashMap;
use shared::models::{ConnectorEvent, ServiceProvider, SourceType, SyncRequest};
use std::collections::{HashMap, HashSet};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use tracing::{error, info, warn};

use crate::client::ImapSession;
use crate::config::ImapAccountConfig;
use crate::models::{
    build_thread_connector_event, generate_thread_content, make_thread_document_id,
    parse_raw_email, resolve_new_email_thread_root, resolve_thread_root, FolderSyncState,
    ImapConnectorState, ParsedEmail,
};
use shared::SdkClient;

const FETCH_BATCH_SIZE: usize = 50;

pub struct SyncManager {
    sdk_client: SdkClient,
    active_syncs: DashMap<String, Arc<AtomicBool>>,
}

impl SyncManager {
    pub fn new(sdk_client: SdkClient) -> Self {
        Self {
            sdk_client,
            active_syncs: DashMap::new(),
        }
    }

    pub fn cancel_sync(&self, sync_run_id: &str) -> bool {
        if let Some(cancelled) = self.active_syncs.get(sync_run_id) {
            cancelled.store(true, Ordering::SeqCst);
            true
        } else {
            false
        }
    }

    pub async fn sync_source(&self, request: SyncRequest) -> Result<()> {
        let sync_run_id = &request.sync_run_id;
        let source_id = &request.source_id;

        info!(
            "Starting IMAP sync for source: {} (sync_run_id: {})",
            source_id, sync_run_id
        );

        if let Err(e) = self.do_sync(&request).await {
            let _ = self.sdk_client.fail(sync_run_id, &e.to_string()).await;
            return Err(e);
        }
        Ok(())
    }

    async fn do_sync(&self, request: &SyncRequest) -> Result<()> {
        let sync_run_id = &request.sync_run_id;
        let source_id = &request.source_id;

        let source = self.sdk_client.get_source(source_id).await?;
        if !source.is_active {
            return Err(anyhow!("Source is not active: {}", source_id));
        }
        if source.source_type != SourceType::Imap {
            return Err(anyhow!(
                "Invalid source type for IMAP connector: {:?}",
                source.source_type
            ));
        }

        let creds = self.sdk_client.get_credentials(source_id).await?;
        if creds.provider != ServiceProvider::Imap {
            return Err(anyhow!(
                "Expected IMAP credentials, found {:?}",
                creds.provider
            ));
        }

        let username = creds
            .credentials
            .get("username")
            .and_then(|v| v.as_str())
            .ok_or_else(|| anyhow!("Missing 'username' in credentials"))?
            .to_string();
        let password = creds
            .credentials
            .get("password")
            .and_then(|v| v.as_str())
            .ok_or_else(|| anyhow!("Missing 'password' in credentials"))?
            .to_string();

        let account_config = ImapAccountConfig::from_source_config(&source.config)?;

        if !account_config.sync_enabled {
            info!("Sync disabled for source {}, skipping", source_id);
            let _ = self.sdk_client.complete(sync_run_id, 0, 0, None).await;
            return Ok(());
        }

        let display_name = account_config
            .display_name
            .clone()
            .unwrap_or_else(|| source.name.clone());

        let mut connector_state =
            ImapConnectorState::from_connector_state(&source.connector_state);
        if request.sync_mode == "full" {
            info!(
                "Full sync requested, resetting connector state for source {}",
                source_id
            );
            connector_state = ImapConnectorState::default();
        }

        let cancelled = Arc::new(AtomicBool::new(false));
        self.active_syncs
            .insert(sync_run_id.to_string(), cancelled.clone());

        let result = self
            .execute_sync(
                &account_config,
                &username,
                &password,
                source_id,
                sync_run_id,
                &display_name,
                &mut connector_state,
                &cancelled,
            )
            .await;

        if cancelled.load(Ordering::SeqCst) {
            info!("IMAP sync {} was cancelled", sync_run_id);
            let _ = self
                .sdk_client
                .save_connector_state(source_id, connector_state.to_json())
                .await;
            let _ = self.sdk_client.cancel(sync_run_id).await;
            self.active_syncs.remove(sync_run_id);
            return Ok(());
        }

        self.active_syncs.remove(sync_run_id);

        match result {
            Ok((total_scanned, total_processed)) => {
                info!(
                    "IMAP sync completed for source {}: {} scanned, {} processed",
                    source.name, total_scanned, total_processed
                );
                self.sdk_client
                    .complete(
                        sync_run_id,
                        total_scanned as i32,
                        total_processed as i32,
                        Some(connector_state.to_json()),
                    )
                    .await?;
                Ok(())
            }
            Err(e) => {
                let _ = self
                    .sdk_client
                    .save_connector_state(source_id, connector_state.to_json())
                    .await;
                error!("IMAP sync failed for source {}: {}", source.name, e);
                Err(e)
            }
        }
    }

    #[allow(clippy::too_many_arguments)]
    async fn execute_sync(
        &self,
        config: &ImapAccountConfig,
        username: &str,
        password: &str,
        source_id: &str,
        sync_run_id: &str,
        display_name: &str,
        state: &mut ImapConnectorState,
        cancelled: &AtomicBool,
    ) -> Result<(usize, usize)> {
        let mut session = ImapSession::connect(config, username, password)
            .await
            .context("Failed to connect to IMAP server")?;

        let folders = session
            .list_folders()
            .await
            .context("Failed to list IMAP folders")?;

        let folders_to_sync: Vec<String> = folders
            .into_iter()
            .filter(|f| config.should_index_folder(f))
            .collect();

        info!(
            "Syncing {} folders for source {}",
            folders_to_sync.len(),
            source_id
        );

        let mut total_scanned = 0usize;
        let mut total_processed = 0usize;

        for folder in &folders_to_sync {
            if cancelled.load(Ordering::SeqCst) {
                info!("Sync cancelled during folder enumeration");
                break;
            }

            match self
                .sync_folder(
                    &mut session,
                    config,
                    folder,
                    source_id,
                    sync_run_id,
                    display_name,
                    state,
                    cancelled,
                )
                .await
            {
                Ok((scanned, processed)) => {
                    total_scanned += scanned;
                    total_processed += processed;
                }
                Err(e) => {
                    warn!(
                        "Failed to sync folder '{}' for source {}: {}",
                        folder, source_id, e
                    );
                    // Continue with remaining folders.
                }
            }
        }

        session.logout().await;

        Ok((total_scanned, total_processed))
    }

    #[allow(clippy::too_many_arguments)]
    async fn sync_folder(
        &self,
        session: &mut ImapSession,
        config: &ImapAccountConfig,
        folder: &str,
        source_id: &str,
        sync_run_id: &str,
        display_name: &str,
        state: &mut ImapConnectorState,
        cancelled: &AtomicBool,
    ) -> Result<(usize, usize)> {
        let (uid_validity, _exists) = session
            .examine_folder(folder)
            .await
            .with_context(|| format!("Failed to examine folder '{}'", folder))?;

        let folder_state = state.folders.entry(folder.to_string()).or_insert_with(|| {
            FolderSyncState {
                uid_validity,
                indexed_uids: vec![],
                messages: HashMap::new(),
                skipped_uids: HashSet::new(),
            }
        });

        // If UIDVALIDITY changed, all previously stored UIDs are invalid; the
        // indexed_uids set must be cleared so every message is re-fetched.
        if uid_validity != 0 && folder_state.uid_validity != 0 && folder_state.uid_validity != uid_validity {
            warn!(
                "UIDVALIDITY changed for folder '{}' (was {}, now {}), performing full resync",
                folder, folder_state.uid_validity, uid_validity
            );
            folder_state.indexed_uids.clear();
            folder_state.messages.clear();
            folder_state.skipped_uids.clear();
        }
        // Only persist the server's UIDVALIDITY when it is non-zero.  If the
        // server stops advertising UIDVALIDITY (returns 0), preserving the
        // last-known good value means we can still detect a genuine change the
        // next time the server starts advertising again.
        if uid_validity != 0 {
            folder_state.uid_validity = uid_validity;
        }

        // Fetch all current server UIDs in a single round trip.  This result
        // is used for BOTH new-message detection and deletion detection,
        // avoiding a separate UID SEARCH call for each purpose.
        let server_uids = session
            .fetch_all_uids()
            .await
            .with_context(|| format!("Failed to fetch UIDs for folder '{}'", folder))?;
        let server_uid_set: HashSet<u32> = server_uids.iter().copied().collect();

        // Remove skipped UIDs that no longer exist on the server so the set
        // does not grow unboundedly on active mailboxes.
        folder_state.skipped_uids.retain(|uid| server_uid_set.contains(uid));

        let deleted_uids: Vec<u32> = folder_state
            .indexed_uids
            .iter()
            .copied()
            .filter(|uid| !server_uid_set.contains(uid))
            .collect();

        // --- New-message detection --------------------------------------------
        // Compute new_uids = server_uids − indexed_uids_set.
        //
        // Using set subtraction (rather than a `last_uid` high-water-mark)
        // guarantees that messages skipped in a previous sync due to parse
        // errors, store failures, or size limits are retried on every
        // subsequent sync instead of being permanently lost.
        let indexed_uid_set: HashSet<u32> = folder_state.indexed_uids.iter().copied().collect();
        let mut new_uids: Vec<u32> = server_uids
            .into_iter()
            .filter(|uid| !indexed_uid_set.contains(uid) && !folder_state.skipped_uids.contains(uid))
            .collect();
        new_uids.sort_unstable();

        let count_new = new_uids.len();
        info!("Folder '{}': {} new messages to index", folder, count_new);

        // --- Batch fetch and index --------------------------------------------
        let mut scanned = 0usize;
        let mut processed = 0usize;

        // Build message_id → UID index for thread root chain-walking.
        // Maintained incrementally as new messages are indexed.
        let mut by_message_id: HashMap<String, u32> = folder_state
            .messages
            .values()
            .filter_map(|m| m.message_id.as_ref().map(|id| (id.clone(), m.imap_uid)))
            .collect();

        // Precompute canonical thread root per stored UID.
        // Updated as new messages are indexed within this sync run.
        let mut thread_root_map: HashMap<u32, String> = folder_state
            .messages
            .keys()
            .map(|&uid| (uid, resolve_thread_root(uid, &folder_state.messages, &by_message_id)))
            .collect();

        for chunk in new_uids.chunks(FETCH_BATCH_SIZE) {
            if cancelled.load(Ordering::SeqCst) {
                info!("Sync cancelled during message fetch in folder '{}'", folder);
                break;
            }

            let raw_messages = match session.fetch_messages(chunk).await {
                Ok(msgs) => msgs,
                Err(e) => {
                    warn!("Failed to fetch batch in folder '{}': {}", folder, e);
                    continue;
                }
            };

            for raw in &raw_messages {
                scanned += 1;

                // Enforce optional message size limit.
                if config.max_message_size > 0
                    && raw.data.len() as u64 > config.max_message_size
                {
                    warn!(
                        "Skipping message UID {} in '{}': size {} exceeds limit {}",
                        raw.uid,
                        folder,
                        raw.data.len(),
                        config.max_message_size
                    );
                    // Record so we don't re-download the body on every subsequent sync.
                    folder_state.skipped_uids.insert(raw.uid);
                    continue;
                }

                let mut email = match parse_raw_email(&raw.data, raw.uid, folder) {
                    Ok(e) => e,
                    Err(e) => {
                        warn!(
                            "Failed to parse message UID {} in '{}': {}",
                            raw.uid, folder, e
                        );
                        continue;
                    }
                };
                email.flags = raw.flags.clone();

                // Resolve canonical thread root with full chain-walking so that
                // replies-to-replies without a References header are grouped
                // correctly under the thread's original root message.
                let thread_root = resolve_new_email_thread_root(
                    &email,
                    &folder_state.messages,
                    &by_message_id,
                );
                let thread_existed =
                    thread_root_map.values().any(|r| r == &thread_root);

                let mut thread_messages: Vec<ParsedEmail> = folder_state
                    .messages
                    .values()
                    .filter(|m| thread_root_map.get(&m.imap_uid) == Some(&thread_root))
                    .cloned()
                    .collect();
                // Defensive: ensure the new email's UID is not already in the slice.
                thread_messages.retain(|m| m.imap_uid != email.imap_uid);
                thread_messages.push(email.clone());

                let content = generate_thread_content(&thread_messages);
                let content_id = match self
                    .sdk_client
                    .store_content(sync_run_id, &content)
                    .await
                {
                    Ok(id) => id,
                    Err(e) => {
                        warn!(
                            "Failed to store content for UID {} in '{}': {}",
                            raw.uid, folder, e
                        );
                        continue;
                    }
                };

                let event = build_thread_connector_event(
                    &thread_messages,
                    sync_run_id.to_string(),
                    source_id.to_string(),
                    content_id,
                    display_name,
                    config.webmail_url_template.as_deref(),
                    thread_existed,
                );

                if let Err(e) = self
                    .sdk_client
                    .emit_event(sync_run_id, source_id, event)
                    .await
                {
                    warn!(
                        "Failed to emit event for UID {} in '{}': {}",
                        raw.uid, folder, e
                    );
                    continue;
                }

                // Update lookup structures before inserting into the messages map
                // so that the next message in this batch can resolve its thread root.
                if let Some(mid) = &email.message_id {
                    by_message_id.insert(mid.clone(), raw.uid);
                }
                thread_root_map.insert(raw.uid, thread_root.clone());
                // Guaranteed not in indexed_uids (new_uids = server_uids − indexed_uid_set − skipped_uids).
                folder_state.indexed_uids.push(raw.uid);
                folder_state.messages.insert(raw.uid, email);
                processed += 1;
            }

            // Heartbeat / progress update.
            let batch_size = raw_messages.len() as i32;
            let _ = self
                .sdk_client
                .increment_scanned(sync_run_id, batch_size)
                .await;
        }

        let deleted_count = deleted_uids.len();

        // --- Flag-change detection -------------------------------------------
        // Fetch FLAGS (no body) for all already-indexed messages still present
        // on the server.  Emit DocumentUpdated for threads where any message's
        // flags changed since it was indexed.  Threads already updated in the
        // new-message pass above are skipped to avoid redundant events.
        {
            let live_indexed: Vec<u32> = folder_state
                .indexed_uids
                .iter()
                .copied()
                .filter(|uid| server_uid_set.contains(uid))
                .collect();

            if !live_indexed.is_empty() {
                // Phase 1: collect all (uid, new_flags) pairs where flags changed.
                let mut flag_changes: Vec<(u32, Vec<String>)> = Vec::new();
                'flag_chunks: for chunk in live_indexed.chunks(FETCH_BATCH_SIZE) {
                    if cancelled.load(Ordering::SeqCst) {
                        break 'flag_chunks;
                    }
                    match session.fetch_flags_only(chunk).await {
                        Ok(updates) => {
                            for (uid, mut new_flags) in updates {
                                if let Some(msg) = folder_state.messages.get(&uid) {
                                    // Sort both sides before comparing: the IMAP server
                                    // may return flags in any order, so a plain Vec !=
                                    // check would produce spurious updates every sync.
                                    let mut stored_flags = msg.flags.clone();
                                    stored_flags.sort_unstable();
                                    new_flags.sort_unstable();
                                    if stored_flags != new_flags {
                                        flag_changes.push((uid, new_flags));
                                    }
                                }
                            }
                        }
                        Err(e) => warn!("Failed to fetch flags in '{}': {}", folder, e),
                    }
                }

                if !flag_changes.is_empty() {
                    // Identify dirty thread roots — all of them, regardless of
                    // whether the thread was touched in the new-message pass.
                    // Skipping threads from the new-message pass would suppress
                    // legitimate flag-change events on already-indexed messages.
                    let dirty_threads: HashSet<String> = flag_changes
                        .iter()
                        .filter_map(|(uid, _)| thread_root_map.get(uid).cloned())
                        .collect();

                    // Phase 2: apply flag updates to stored snapshots.
                    for (uid, new_flags) in flag_changes {
                        if let Some(msg) = folder_state.messages.get_mut(&uid) {
                            msg.flags = new_flags;
                        }
                    }

                    // Phase 3: one DocumentUpdated per dirty thread.
                    for thread_root in dirty_threads {
                        if cancelled.load(Ordering::SeqCst) {
                            break;
                        }
                        let thread_messages: Vec<ParsedEmail> = folder_state
                            .messages
                            .values()
                            .filter(|m| {
                                thread_root_map
                                    .get(&m.imap_uid)
                                    .map(String::as_str)
                                    == Some(&thread_root)
                            })
                            .cloned()
                            .collect();
                        if thread_messages.is_empty() {
                            continue;
                        }
                        let content = generate_thread_content(&thread_messages);
                        let content_id =
                            match self.sdk_client.store_content(sync_run_id, &content).await {
                                Ok(id) => id,
                                Err(e) => {
                                    warn!(
                                        "Failed to store content for flag-updated thread \
                                         '{}' in '{}': {}",
                                        thread_root, folder, e
                                    );
                                    continue;
                                }
                            };
                        let event = build_thread_connector_event(
                            &thread_messages,
                            sync_run_id.to_string(),
                            source_id.to_string(),
                            content_id,
                            display_name,
                            config.webmail_url_template.as_deref(),
                            true, // always an update
                        );
                        if let Err(e) = self
                            .sdk_client
                            .emit_event(sync_run_id, source_id, event)
                            .await
                        {
                            warn!(
                                "Failed to emit DocumentUpdated for flag-changed thread \
                                 '{}' in '{}': {}",
                                thread_root, folder, e
                            );
                        } else {
                            processed += 1;
                        }
                    }
                }
            }
        }

        for uid in deleted_uids {
            let Some(deleted_message) = folder_state.messages.get(&uid).cloned() else {
                // The UID was tracked but we have no message snapshot, so we
                // cannot reconstruct the thread document ID to emit a
                // DocumentDeleted event.  Remove from indexed_uids to avoid
                // retrying on every sync, but warn so the orphaned document
                // is visible in logs.
                warn!(
                    "UID {} in folder '{}' is in indexed_uids but has no message snapshot; \
                     removing from state without emitting DocumentDeleted (orphaned index entry)",
                    uid, folder
                );
                folder_state.indexed_uids.retain(|indexed_uid| *indexed_uid != uid);
                continue;
            };

            let thread_root = match thread_root_map.get(&uid) {
                Some(r) => r.clone(),
                // Defensive fallback: reconstruct from the snapshot.
                None => {
                    warn!(
                        "UID {} in '{}' missing from thread_root_map; \
                         reconstructing thread root",
                        uid, folder
                    );
                    deleted_message.thread_id()
                }
            };
            let remaining_messages: Vec<ParsedEmail> = folder_state
                .messages
                .values()
                .filter(|m| {
                    thread_root_map.get(&m.imap_uid).map(String::as_str)
                        == Some(&thread_root)
                        && m.imap_uid != uid
                })
                .cloned()
                .collect();

            let event = if remaining_messages.is_empty() {
                ConnectorEvent::DocumentDeleted {
                    sync_run_id: sync_run_id.to_string(),
                    source_id: source_id.to_string(),
                    document_id: make_thread_document_id(source_id, folder, &thread_root),
                }
            } else {
                let content = generate_thread_content(&remaining_messages);
                let content_id = match self.sdk_client.store_content(sync_run_id, &content).await {
                    Ok(id) => id,
                    Err(e) => {
                        warn!(
                            "Failed to store updated thread content after deleting UID {} in '{}': {}",
                            uid, folder, e
                        );
                        continue;
                    }
                };

                build_thread_connector_event(
                    &remaining_messages,
                    sync_run_id.to_string(),
                    source_id.to_string(),
                    content_id,
                    display_name,
                    config.webmail_url_template.as_deref(),
                    true,
                )
            };

            if let Err(e) = self
                .sdk_client
                .emit_event(sync_run_id, source_id, event)
                .await
            {
                warn!(
                    "Failed to emit thread update for deleted UID {} in '{}': {}",
                    uid, folder, e
                );
                continue;
            }

            folder_state.messages.remove(&uid);
            folder_state.indexed_uids.retain(|indexed_uid| *indexed_uid != uid);
        }

        folder_state.indexed_uids.sort_unstable();

        info!(
            "Folder '{}': scanned {}, indexed {}, deleted {}",
            folder, scanned, processed, deleted_count
        );
        Ok((scanned, processed))
    }
}

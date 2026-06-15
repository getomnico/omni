use anyhow::{Context, Result};
use omni_connector_sdk::SyncContext;
use omni_connector_sdk::{ConnectorEvent, SyncType};
use std::collections::{HashMap, HashSet};
use std::io;
use tracing::{error, info, warn};

use crate::client::ImapSession;
use crate::config::{ImapAccountConfig, ImapCredentials};
use crate::models::{
    FolderSyncState, ImapConnectorState, ParsedEmail, build_thread_connector_event,
    generate_thread_content, make_thread_document_id, parse_raw_email,
    resolve_new_email_thread_root, resolve_thread_root,
};

const FETCH_BATCH_SIZE: usize = 50;

/// Returns `true` when the error chain indicates the underlying TCP
/// connection was lost (broken pipe, connection reset, unexpected EOF,
/// etc.).  Used to decide whether an IMAP session should be
/// re-established after a folder sync failure.
fn is_connection_error(e: &anyhow::Error) -> bool {
    e.chain().any(|cause| {
        if let Some(io_err) = cause.downcast_ref::<io::Error>() {
            return matches!(
                io_err.kind(),
                io::ErrorKind::BrokenPipe
                    | io::ErrorKind::ConnectionReset
                    | io::ErrorKind::ConnectionAborted
                    | io::ErrorKind::NotConnected
                    | io::ErrorKind::UnexpectedEof
            );
        }
        false
    })
}

pub struct SyncManager;

impl SyncManager {
    pub fn new() -> Self {
        Self
    }

    pub async fn run_sync(
        &self,
        config: ImapAccountConfig,
        credentials: ImapCredentials,
        state: Option<ImapConnectorState>,
        ctx: SyncContext,
    ) -> Result<()> {
        let sync_run_id = ctx.sync_run_id().to_string();
        let source_id = ctx.source_id().to_string();

        info!(
            "Starting IMAP sync for source: {} (sync_run_id: {})",
            source_id, sync_run_id
        );

        if !config.sync_enabled {
            info!("Sync disabled for source {}, skipping", source_id);
            let _ = ctx.complete().await;
            return Ok(());
        }

        let mut connector_state = if ctx.sync_mode() == SyncType::Full {
            // On a full sync we want to re-fetch all messages from scratch, but
            // deletion detection requires the previous state:
            // - indexed_uids: so deleted_uids = old_indexed_uids − server_uids can be computed
            // - messages: so the deletion loop can find thread snapshots to emit DocumentDeleted
            // indexed_uids is cleared immediately after deleted_uids is computed in
            // sync_folder, before new messages are pushed, to avoid duplicates.
            // Clear skipped_uids so oversized messages are retried on a full sync.
            info!(
                "Full sync requested, preserving state for deletion detection for source {}",
                source_id
            );
            let mut old_state = state.unwrap_or_default();
            for folder_state in old_state.folders.values_mut() {
                folder_state.skipped_uids.clear();
            }
            old_state
        } else {
            state.unwrap_or_default()
        };

        let display_name = config
            .display_name
            .clone()
            .unwrap_or_else(|| "imap".to_string());

        let user_email = ctx.get_user_email_for_source().await.ok();

        let result = self
            .execute_sync(
                &config,
                &credentials.username,
                &credentials.password,
                &display_name,
                user_email.as_deref(),
                &mut connector_state,
                &ctx,
            )
            .await;

        if ctx.is_cancelled() {
            info!("IMAP sync {} was cancelled", sync_run_id);
            let _ = ctx.save_checkpoint(connector_state.to_json()).await;
            ctx.cancel().await?;
            return Ok(());
        }

        match result {
            Ok((total_scanned, total_processed)) => {
                info!(
                    "IMAP sync completed for source {}: {} scanned, {} processed",
                    source_id, total_scanned, total_processed
                );
                ctx.increment_updated(total_processed as i32).await?;
                ctx.save_checkpoint(connector_state.to_json()).await?;
                ctx.complete().await?;
                Ok(())
            }
            Err(e) => {
                let _ = ctx.save_checkpoint(connector_state.to_json()).await;
                error!("IMAP sync failed for source {}: {}", source_id, e);
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
        display_name: &str,
        user_email: Option<&str>,
        state: &mut ImapConnectorState,
        ctx: &SyncContext,
    ) -> Result<(usize, usize)> {
        let source_id = ctx.source_id();
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
            if ctx.is_cancelled() {
                info!("Sync cancelled during folder enumeration");
                break;
            }

            match self
                .sync_folder(
                    &mut session,
                    config,
                    folder,
                    display_name,
                    user_email,
                    state,
                    ctx,
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
                    // If the error is connection-level, attempt to re-establish
                    // the IMAP session so subsequent folders can still be synced.
                    if is_connection_error(&e) {
                        match ImapSession::connect(config, username, password).await {
                            Ok(new_session) => {
                                info!(
                                    "Reconnected IMAP session for source {} after connection loss",
                                    source_id
                                );
                                session = new_session;
                            }
                            Err(reconnect_err) => {
                                warn!(
                                    "Failed to reconnect IMAP session for source {}: {}",
                                    source_id, reconnect_err
                                );
                                break;
                            }
                        }
                    }
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
        display_name: &str,
        user_email: Option<&str>,
        state: &mut ImapConnectorState,
        ctx: &SyncContext,
    ) -> Result<(usize, usize)> {
        let source_id = ctx.source_id();
        let sync_run_id = ctx.sync_run_id();
        let (uid_validity, _exists) = session
            .examine_folder(folder)
            .await
            .with_context(|| format!("Failed to examine folder '{}'", folder))?;

        let folder_state =
            state
                .folders
                .entry(folder.to_string())
                .or_insert_with(|| FolderSyncState {
                    uid_validity,
                    indexed_uids: vec![],
                    messages: HashMap::new(),
                    skipped_uids: HashSet::new(),
                });

        // If UIDVALIDITY changed, all previously stored UIDs are invalid; the
        // indexed_uids set must be cleared so every message is re-fetched.
        if uid_validity != 0
            && folder_state.uid_validity != 0
            && folder_state.uid_validity != uid_validity
        {
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
        folder_state
            .skipped_uids
            .retain(|uid| server_uid_set.contains(uid));

        let deleted_uids: Vec<u32> = folder_state
            .indexed_uids
            .iter()
            .copied()
            .filter(|uid| !server_uid_set.contains(uid))
            .collect();

        // On a full sync, clear indexed_uids now that the deletion list has
        // been captured.  Every surviving UID will be re-pushed in the
        // new-message pass below, keeping the set clean with no duplicates.
        if ctx.sync_mode() == SyncType::Full {
            folder_state.indexed_uids.clear();
        }

        // Pre-compute a set of deleted UIDs so the new-message pass can exclude
        // their stale snapshots from thread documents.  Without this, a thread
        // update emitted for a surviving thread member would include the deleted
        // member's old snapshot.  If that event and the deletion loop's
        // corrective event land in the same indexer batch, the indexer's
        // first-write-wins dedup would silently discard the correction.
        let deleted_uid_set: HashSet<u32> = deleted_uids.iter().copied().collect();

        // --- New-message detection --------------------------------------------
        // On incremental syncs: new_uids = server_uids − indexed_uids_set, so
        // messages already indexed are not re-fetched.
        // On full syncs: new_uids = all server_uids, so every message body is
        // re-downloaded.  deleted_uids was captured before indexed_uids was
        // cleared, so deletion detection still fires for removed messages.
        let mut new_uids: Vec<u32> = if ctx.sync_mode() == SyncType::Full {
            server_uids
        } else {
            let indexed_uid_set: HashSet<u32> = folder_state.indexed_uids.iter().copied().collect();
            server_uids
                .into_iter()
                .filter(|uid| {
                    !indexed_uid_set.contains(uid) && !folder_state.skipped_uids.contains(uid)
                })
                .collect()
        };
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
            .map(|&uid| {
                (
                    uid,
                    resolve_thread_root(uid, &folder_state.messages, &by_message_id),
                )
            })
            .collect();

        for chunk in new_uids.chunks(FETCH_BATCH_SIZE) {
            if ctx.is_cancelled() {
                info!("Sync cancelled during message fetch in folder '{}'", folder);
                break;
            }

            let raw_messages = match session.fetch_messages(chunk).await {
                Ok(msgs) => msgs,
                Err(e) => {
                    warn!("Failed to fetch batch in folder '{}': {}", folder, e);
                    // Attempt to retrieve each message individually so a single
                    // oversized, malformed, or server-rejected message does not
                    // cause the entire batch to be silently skipped.
                    let mut recovered = Vec::new();
                    for &uid in chunk {
                        match session.fetch_messages(&[uid]).await {
                            Ok(msgs) => recovered.extend(msgs),
                            Err(single_err) => {
                                if is_connection_error(&single_err) {
                                    // Propagate immediately so execute_sync can reconnect
                                    // rather than retrying every UID on a dead connection.
                                    return Err(single_err.context(format!(
                                        "Connection lost during UID fetch in folder '{}'",
                                        folder
                                    )));
                                }
                                warn!("Skipping UID {} in '{}': {}", uid, folder, single_err);
                            }
                        }
                    }
                    recovered
                }
            };

            for raw in &raw_messages {
                scanned += 1;

                // Enforce optional message size limit.
                if config.max_message_size > 0 && raw.data.len() as u64 > config.max_message_size {
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

                let (mut email, raw_attachments) = match parse_raw_email(&raw.data, raw.uid, folder)
                {
                    Ok(result) => result,
                    Err(e) => {
                        warn!(
                            "Failed to parse message UID {} in '{}': {}",
                            raw.uid, folder, e
                        );
                        continue;
                    }
                };
                email.flags = raw.flags.clone();

                // If the email body is HTML (no plain-text alternative), convert
                // it via the connector manager so Docling is used when enabled.
                // On failure, fall back to the built-in HTML-to-text extractor
                // so we never index raw HTML tags.
                if email.body_is_html && !email.body_text.is_empty() {
                    match ctx
                        .sdk_client()
                        .extract_text(
                            sync_run_id,
                            email.body_text.as_bytes().to_vec(),
                            "text/html",
                            None,
                        )
                        .await
                    {
                        Ok(text) => {
                            email.body_text = text;
                            email.body_is_html = false;
                        }
                        Err(e) => {
                            warn!(
                                "Failed to convert HTML body for UID {}: {}, using built-in fallback",
                                raw.uid, e
                            );
                            let html_bytes = email.body_text.as_bytes();
                            email.body_text =
                                omni_connector_sdk::content_extractor::extract_content(
                                    html_bytes,
                                    "text/html",
                                    None,
                                )
                                .unwrap_or_default();
                            email.body_is_html = false;
                        }
                    }
                }

                // Extract attachment text via the connector manager (supports
                // Docling when enabled) and append to the email body.
                for att in raw_attachments {
                    match ctx
                        .sdk_client()
                        .extract_text(sync_run_id, att.data, &att.mime_type, Some(&att.filename))
                        .await
                    {
                        Ok(text) if !text.trim().is_empty() => {
                            email.body_text.push_str("\n\n");
                            email
                                .body_text
                                .push_str(&format!("[Attachment: {}]\n", att.filename));
                            email.body_text.push_str(&text);
                        }
                        Ok(_) => {}
                        Err(e) => {
                            warn!(
                                "Failed to extract attachment '{}' for UID {}: {}",
                                att.filename, raw.uid, e
                            );
                        }
                    }
                }

                // Resolve canonical thread root with full chain-walking so that
                // replies-to-replies without a References header are grouped
                // correctly under the thread's original root message.
                let thread_root =
                    resolve_new_email_thread_root(&email, &folder_state.messages, &by_message_id);
                let thread_existed = thread_root_map.values().any(|r| r == &thread_root);

                let mut thread_messages: Vec<ParsedEmail> = folder_state
                    .messages
                    .values()
                    .filter(|m| {
                        thread_root_map.get(&m.imap_uid) == Some(&thread_root)
                            // Exclude UIDs already known to be deleted on the
                            // server so the emitted document never includes
                            // stale content from messages about to be removed.
                            && !deleted_uid_set.contains(&m.imap_uid)
                    })
                    .cloned()
                    .collect();
                // Defensive: ensure the new email's UID is not already in the slice.
                thread_messages.retain(|m| m.imap_uid != email.imap_uid);
                thread_messages.push(email.clone());

                let content = generate_thread_content(&thread_messages);
                let content_id = match ctx.store_content(&content).await {
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
                    user_email,
                    thread_existed,
                );

                if let Err(e) = ctx.emit_event(event).await {
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
                // On incremental syncs, UID was not in indexed_uids.
                // On full syncs, indexed_uids was cleared before this pass; no duplicate can exist.
                folder_state.indexed_uids.push(raw.uid);
                folder_state.messages.insert(raw.uid, email);
                processed += 1;
            }

            // Heartbeat / progress update.
            let batch_size = raw_messages.len() as i32;
            let _ = ctx.increment_scanned(batch_size).await;
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
                    if ctx.is_cancelled() {
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
                        if ctx.is_cancelled() {
                            break;
                        }
                        let thread_messages: Vec<ParsedEmail> = folder_state
                            .messages
                            .values()
                            .filter(|m| {
                                thread_root_map.get(&m.imap_uid).map(String::as_str)
                                    == Some(&thread_root)
                            })
                            .cloned()
                            .collect();
                        if thread_messages.is_empty() {
                            continue;
                        }
                        let content = generate_thread_content(&thread_messages);
                        let content_id = match ctx.store_content(&content).await {
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
                            user_email,
                            true, // always an update
                        );
                        match ctx.emit_event(event).await {
                            Err(e) => {
                                warn!(
                                    "Failed to emit DocumentUpdated for flag-changed thread \
                                 '{}' in '{}': {}",
                                    thread_root, folder, e
                                );
                            }
                            _ => {
                                processed += 1;
                            }
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
                folder_state
                    .indexed_uids
                    .retain(|indexed_uid| *indexed_uid != uid);
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
                    thread_root_map.get(&m.imap_uid).map(String::as_str) == Some(&thread_root)
                        && m.imap_uid != uid
                })
                .cloned()
                .collect();

            let event = if remaining_messages.is_empty() {
                ConnectorEvent::DocumentDeleted {
                    sync_run_id: sync_run_id.to_string(),
                    source_id: source_id.to_string(),
                    document_id: make_thread_document_id(folder, &thread_root),
                }
            } else {
                let content = generate_thread_content(&remaining_messages);
                let content_id = match ctx.store_content(&content).await {
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
                    user_email,
                    true,
                )
            };

            if let Err(e) = ctx.emit_event(event).await {
                warn!(
                    "Failed to emit thread update for deleted UID {} in '{}': {}",
                    uid, folder, e
                );
                continue;
            }

            folder_state.messages.remove(&uid);
            folder_state
                .indexed_uids
                .retain(|indexed_uid| *indexed_uid != uid);
        }

        folder_state.indexed_uids.sort_unstable();

        info!(
            "Folder '{}': scanned {}, indexed {}, deleted {}",
            folder, scanned, processed, deleted_count
        );
        Ok((scanned, processed))
    }
}

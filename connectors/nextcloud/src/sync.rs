use anyhow::{anyhow, Result};
use dashmap::DashMap;
use shared::models::{
    ConnectorEvent, DocumentMetadata, DocumentPermissions, ServiceProvider, SourceType,
    SyncRequest, SyncType,
};
use std::collections::{HashMap, HashSet};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use time::format_description::well_known::{Rfc2822, Rfc3339};
use tracing::{error, info, warn};

use crate::client::NextcloudClient;
use crate::config::NextcloudConfig;
use crate::models::{DavEntry, NextcloudConnectorState};
use shared::SdkClient;

const BATCH_SIZE: usize = 20;

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
        info!(
            "Starting Nextcloud sync for source: {} (sync_run_id: {})",
            request.source_id, sync_run_id
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
        if source.source_type != SourceType::Nextcloud {
            return Err(anyhow!(
                "Invalid source type for Nextcloud connector: {:?}",
                source.source_type
            ));
        }

        let creds = self.sdk_client.get_credentials(source_id).await?;
        if creds.provider != ServiceProvider::Nextcloud {
            return Err(anyhow!(
                "Expected Nextcloud credentials, found {:?}",
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

        let config = NextcloudConfig::from_source_config(&source.config)?;

        if !config.sync_enabled {
            info!("Sync disabled for source {}, skipping", source_id);
            let _ = self.sdk_client.complete(sync_run_id, 0, 0, None).await;
            return Ok(());
        }

        let mut state = NextcloudConnectorState::from_connector_state(&source.connector_state);
        if request.sync_mode == SyncType::Full {
            info!(
                "Full sync requested, resetting connector state for source {}",
                source_id
            );
            state = NextcloudConnectorState::default();
        }

        let cancelled = Arc::new(AtomicBool::new(false));
        self.active_syncs
            .insert(sync_run_id.to_string(), cancelled.clone());

        let user_email = self
            .sdk_client
            .get_user_email_for_source(source_id)
            .await
            .ok();

        let result = self
            .execute_sync(
                &config,
                &username,
                &password,
                source_id,
                sync_run_id,
                user_email.as_deref(),
                &mut state,
                &cancelled,
            )
            .await;

        if cancelled.load(Ordering::SeqCst) {
            info!("Nextcloud sync {} was cancelled", sync_run_id);
            let _ = self
                .sdk_client
                .save_connector_state(source_id, state.to_json())
                .await;
            let _ = self.sdk_client.cancel(sync_run_id).await;
            self.active_syncs.remove(sync_run_id);
            return Ok(());
        }

        self.active_syncs.remove(sync_run_id);

        match result {
            Ok((total_scanned, total_processed)) => {
                info!(
                    "Nextcloud sync completed for source {}: {} scanned, {} processed",
                    source.name, total_scanned, total_processed
                );
                self.sdk_client
                    .complete(
                        sync_run_id,
                        total_scanned as i32,
                        total_processed as i32,
                        Some(state.to_json()),
                    )
                    .await?;
                Ok(())
            }
            Err(e) => {
                let _ = self
                    .sdk_client
                    .save_connector_state(source_id, state.to_json())
                    .await;
                error!("Nextcloud sync failed for source {}: {}", source.name, e);
                Err(e)
            }
        }
    }

    #[allow(clippy::too_many_arguments)]
    async fn execute_sync(
        &self,
        config: &NextcloudConfig,
        username: &str,
        password: &str,
        source_id: &str,
        sync_run_id: &str,
        user_email: Option<&str>,
        state: &mut NextcloudConnectorState,
        cancelled: &AtomicBool,
    ) -> Result<(usize, usize)> {
        let client = NextcloudClient::new(username, password);
        let base_url = config.webdav_base_url(username);

        info!("Listing files from {}", base_url);

        let mut total_scanned = 0usize;
        let mut total_processed = 0usize;
        let mut current_keys = HashSet::<String>::new();

        // Try Depth: infinity first (loads all entries at once — fast for small instances).
        // Falls back to paginated BFS directory traversal when the server rejects it.
        match client.try_list_all(&base_url).await {
            Ok(entries) => {
                let file_entries: Vec<DavEntry> = entries
                    .into_iter()
                    .filter(|e| !e.is_collection && config.should_index_file(&e.filename()))
                    .collect();

                info!(
                    "Found {} files to process for source {}",
                    file_entries.len(),
                    source_id
                );

                for entry in &file_entries {
                    current_keys.insert(entry.file_key());
                }

                let (s, p) = self
                    .process_file_batch(
                        &file_entries,
                        &client,
                        config,
                        username,
                        source_id,
                        sync_run_id,
                        user_email,
                        state,
                        cancelled,
                    )
                    .await;
                total_scanned += s;
                total_processed += p;
            }
            Err(_) => {
                info!("Depth: infinity not supported, using paginated directory traversal");

                let mut dir_queue = std::collections::VecDeque::new();
                dir_queue.push_back(base_url.clone());
                let mut visited = HashSet::<String>::new();

                while let Some(dir_url) = dir_queue.pop_front() {
                    if cancelled.load(Ordering::SeqCst) {
                        break;
                    }

                    let canonical = crate::client::extract_path(&dir_url)
                        .trim_end_matches('/')
                        .to_string();
                    if !visited.insert(canonical.clone()) {
                        warn!("Cycle detected: already visited {}, skipping", dir_url);
                        continue;
                    }

                    let entries = match client.list_directory(&dir_url).await {
                        Ok(e) => e,
                        Err(e) => {
                            warn!("Failed to list directory {}: {}", dir_url, e);
                            continue;
                        }
                    };

                    let parent_path = canonical;
                    let mut page_files = Vec::new();

                    for entry in entries {
                        let entry_path = entry.href.trim_end_matches('/');
                        if entry_path == parent_path {
                            continue; // skip the parent itself
                        }
                        if entry.is_collection {
                            let child_url = crate::client::build_child_url(&dir_url, &entry.href);
                            dir_queue.push_back(child_url);
                        } else if config.should_index_file(&entry.filename()) {
                            current_keys.insert(entry.file_key());
                            page_files.push(entry);
                        }
                    }

                    let (s, p) = self
                        .process_file_batch(
                            &page_files,
                            &client,
                            config,
                            username,
                            source_id,
                            sync_run_id,
                            user_email,
                            state,
                            cancelled,
                        )
                        .await;
                    total_scanned += s;
                    total_processed += p;
                }
            }
        }

        // Detect and emit deletions
        if !cancelled.load(Ordering::SeqCst) {
            let known_set: HashSet<String> = state.known_files.iter().cloned().collect();
            let deleted_keys: Vec<String> = known_set.difference(&current_keys).cloned().collect();

            for key in &deleted_keys {
                if cancelled.load(Ordering::SeqCst) {
                    break;
                }
                let doc_id = format!("nextcloud:{}:{}", source_id, urlencoding::encode(key));
                let event = ConnectorEvent::DocumentDeleted {
                    sync_run_id: sync_run_id.to_string(),
                    source_id: source_id.to_string(),
                    document_id: doc_id,
                };
                if let Err(e) = self
                    .sdk_client
                    .emit_event(sync_run_id, source_id, event)
                    .await
                {
                    warn!("Failed to emit deletion event for {}: {}", key, e);
                }
            }

            // Remove stale etags
            state.etags.retain(|k, _| current_keys.contains(k));
        }

        // Persist current file key set
        state.known_files = current_keys.into_iter().collect();

        Ok((total_scanned, total_processed))
    }

    /// Process a batch of file entries: check etag, download, extract, store, emit.
    /// Returns (scanned, processed) counts.
    #[allow(clippy::too_many_arguments)]
    async fn process_file_batch(
        &self,
        entries: &[DavEntry],
        client: &NextcloudClient,
        config: &NextcloudConfig,
        username: &str,
        source_id: &str,
        sync_run_id: &str,
        user_email: Option<&str>,
        state: &mut NextcloudConnectorState,
        cancelled: &AtomicBool,
    ) -> (usize, usize) {
        let mut scanned = 0usize;
        let mut processed = 0usize;

        for batch in entries.chunks(BATCH_SIZE) {
            if cancelled.load(Ordering::SeqCst) {
                break;
            }

            for entry in batch {
                scanned += 1;
                let key = entry.file_key();

                // Skip unchanged files (compare effective etag — real or synthetic)
                let effective = entry.effective_etag();
                if let Some(ref stored) = state.etags.get(&key) {
                    if effective.as_deref() == Some(stored.as_str()) {
                        continue;
                    }
                }

                let is_update = state.etags.contains_key(&key);

                // Enforce file size limit
                let file_size = entry.content_length.or(entry.oc_size).unwrap_or(0);
                if config.max_file_size > 0 && file_size > config.max_file_size {
                    warn!(
                        "Skipping file '{}': size {} exceeds limit {}",
                        entry.filename(),
                        file_size,
                        config.max_file_size
                    );
                    continue;
                }

                let download_url = build_download_url(&config.server_url, &entry.href);

                let content_text = match download_and_extract(
                    client,
                    &download_url,
                    entry,
                    &self.sdk_client,
                    sync_run_id,
                )
                .await
                {
                    Ok(text) => text,
                    Err(e) => {
                        warn!("Failed to process file '{}': {}", entry.filename(), e);
                        continue;
                    }
                };

                let markdown = entry.to_markdown(username, &config.server_url, &content_text);

                let content_id = match self.sdk_client.store_content(sync_run_id, &markdown).await {
                    Ok(id) => id,
                    Err(e) => {
                        warn!("Failed to store content for '{}': {}", entry.filename(), e);
                        continue;
                    }
                };

                let event = build_file_event(
                    entry,
                    username,
                    &config.server_url,
                    sync_run_id,
                    source_id,
                    &content_id,
                    user_email,
                    is_update,
                );

                if let Err(e) = self
                    .sdk_client
                    .emit_event(sync_run_id, source_id, event)
                    .await
                {
                    warn!("Failed to emit event for '{}': {}", entry.filename(), e);
                    continue;
                }

                // Store effective etag (real or synthetic)
                if let Some(etag) = effective {
                    state.etags.insert(key, etag);
                }
                processed += 1;
            }

            let _ = self
                .sdk_client
                .increment_scanned(sync_run_id, batch.len() as i32)
                .await;
        }

        (scanned, processed)
    }
}

/// Try to parse a date string as RFC 3339 first, then RFC 2822.
fn parse_datetime(s: &str) -> Option<time::OffsetDateTime> {
    time::OffsetDateTime::parse(s, &Rfc3339)
        .or_else(|_| time::OffsetDateTime::parse(s, &Rfc2822))
        .ok()
}

/// Build an absolute download URL from server_url and the entry href.
fn build_download_url(server_url: &str, href: &str) -> String {
    if href.starts_with("http://") || href.starts_with("https://") {
        return href.to_string();
    }
    let base = server_url.trim_end_matches('/');
    format!("{}{}", base, href)
}

/// Download a file and extract its text content via the connector manager.
async fn download_and_extract(
    client: &NextcloudClient,
    url: &str,
    entry: &DavEntry,
    sdk_client: &SdkClient,
    sync_run_id: &str,
) -> Result<String> {
    let data = client.download_file(url).await?;

    let mime = entry
        .content_type
        .as_deref()
        .unwrap_or("application/octet-stream");
    let filename = entry.filename();

    let text = sdk_client
        .extract_text(sync_run_id, data, mime, Some(&filename))
        .await
        .unwrap_or_default();

    Ok(text)
}

/// Build a ConnectorEvent for a file.
pub fn build_file_event(
    entry: &DavEntry,
    username: &str,
    server_url: &str,
    sync_run_id: &str,
    source_id: &str,
    content_id: &str,
    user_email: Option<&str>,
    is_update: bool,
) -> ConnectorEvent {
    let doc_id = entry.document_id(source_id);
    let web_url = entry.web_url(server_url);
    let relative_path = entry.relative_path(username);
    let filename = entry.filename();

    let created_at = entry.creation_date.as_deref().and_then(parse_datetime);
    let updated_at = entry.last_modified.as_deref().and_then(parse_datetime);

    let mut extra = HashMap::new();
    if let Some(ref fid) = entry.file_id {
        extra.insert("file_id".to_string(), serde_json::json!(fid));
    }
    if let Some(ref perms) = entry.permissions {
        extra.insert("permissions".to_string(), serde_json::json!(perms));
    }
    if entry.favorite {
        extra.insert("favorite".to_string(), serde_json::json!(true));
    }
    if let Some(ref owner) = entry.owner_id {
        extra.insert("owner_id".to_string(), serde_json::json!(owner));
    }

    let metadata = DocumentMetadata {
        title: Some(filename.clone()),
        author: entry
            .owner_display_name
            .clone()
            .or_else(|| entry.owner_id.clone()),
        created_at,
        updated_at,
        content_type: entry.content_type.clone(),
        mime_type: entry.content_type.clone(),
        size: entry
            .content_length
            .or(entry.oc_size)
            .map(|s| s.to_string()),
        url: Some(web_url),
        path: Some(relative_path),
        extra: if extra.is_empty() { None } else { Some(extra) },
    };

    let permissions = DocumentPermissions {
        public: false,
        users: user_email.map(|e| vec![e.to_string()]).unwrap_or_default(),
        groups: vec![],
    };

    let mut attributes = HashMap::new();

    // Extract file extension for filtering
    if let Some(ext_pos) = filename.rfind('.') {
        let ext = &filename[ext_pos + 1..];
        if !ext.is_empty() {
            attributes.insert(
                "file_extension".to_string(),
                serde_json::json!(ext.to_lowercase()),
            );
        }
    }

    if is_update {
        ConnectorEvent::DocumentUpdated {
            sync_run_id: sync_run_id.to_string(),
            source_id: source_id.to_string(),
            document_id: doc_id,
            content_id: content_id.to_string(),
            metadata,
            permissions: Some(permissions),
            attributes: Some(attributes),
        }
    } else {
        ConnectorEvent::DocumentCreated {
            sync_run_id: sync_run_id.to_string(),
            source_id: source_id.to_string(),
            document_id: doc_id,
            content_id: content_id.to_string(),
            metadata,
            permissions,
            attributes: Some(attributes),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_build_download_url_relative() {
        assert_eq!(
            build_download_url(
                "https://cloud.example.com",
                "/remote.php/dav/files/alice/doc.pdf"
            ),
            "https://cloud.example.com/remote.php/dav/files/alice/doc.pdf"
        );
    }

    #[test]
    fn test_build_download_url_absolute() {
        let url = "https://cloud.example.com/remote.php/dav/files/alice/doc.pdf";
        assert_eq!(build_download_url("https://other.com", url), url);
    }

    #[test]
    fn test_build_file_event_created() {
        let entry = DavEntry {
            href: "/remote.php/dav/files/alice/doc.pdf".into(),
            file_id: Some("42".into()),
            content_type: Some("application/pdf".into()),
            content_length: Some(1024),
            owner_display_name: Some("Alice".into()),
            ..Default::default()
        };

        let event = build_file_event(
            &entry,
            "alice",
            "https://cloud.example.com",
            "run-1",
            "src-1",
            "cnt-1",
            Some("alice@example.com"),
            false,
        );

        match event {
            ConnectorEvent::DocumentCreated {
                document_id,
                metadata,
                permissions,
                attributes,
                ..
            } => {
                assert!(document_id.contains("src-1"));
                assert_eq!(metadata.title.as_deref(), Some("doc.pdf"));
                assert_eq!(metadata.author.as_deref(), Some("Alice"));
                assert_eq!(metadata.size.as_deref(), Some("1024"));
                assert!(metadata.url.as_ref().unwrap().contains("/f/42"));
                assert_eq!(permissions.users, vec!["alice@example.com"]);
                assert!(!permissions.public);
                let attrs = attributes.unwrap();
                assert_eq!(
                    attrs.get("file_extension").unwrap(),
                    &serde_json::json!("pdf")
                );
            }
            _ => panic!("Expected DocumentCreated"),
        }
    }

    #[test]
    fn test_build_file_event_updated() {
        let entry = DavEntry {
            href: "/remote.php/dav/files/bob/notes.txt".into(),
            ..Default::default()
        };

        let event = build_file_event(
            &entry,
            "bob",
            "https://nc.local",
            "run-2",
            "src-2",
            "cnt-2",
            None,
            true,
        );

        assert!(matches!(event, ConnectorEvent::DocumentUpdated { .. }));
    }

    #[test]
    fn test_parse_datetime_rfc3339() {
        let dt = parse_datetime("2024-01-01T00:00:00+00:00");
        assert!(dt.is_some());
        let dt = dt.unwrap();
        assert_eq!(dt.year(), 2024);
        assert_eq!(dt.month() as u8, 1);
        assert_eq!(dt.day(), 1);
    }

    #[test]
    fn test_parse_datetime_rfc2822() {
        let dt = parse_datetime("Wed, 20 Jul 2022 05:12:23 +0000");
        assert!(dt.is_some());
        let dt = dt.unwrap();
        assert_eq!(dt.year(), 2022);
        assert_eq!(dt.month() as u8, 7);
        assert_eq!(dt.day(), 20);
    }

    #[test]
    fn test_parse_datetime_rfc2822_gmt() {
        // Nextcloud returns "GMT" not "+0000" — verify the parser handles it
        let dt = parse_datetime("Thu, 01 Jan 2024 00:00:00 GMT");
        assert!(
            dt.is_some(),
            "RFC 2822 with GMT timezone must parse successfully"
        );
        assert_eq!(dt.unwrap().year(), 2024);
    }

    #[test]
    fn test_parse_datetime_invalid() {
        assert!(parse_datetime("not a date").is_none());
        assert!(parse_datetime("").is_none());
    }

    #[test]
    fn test_build_file_event_with_dates() {
        let entry = DavEntry {
            href: "/remote.php/dav/files/alice/doc.pdf".into(),
            file_id: Some("42".into()),
            creation_date: Some("2024-03-15T10:30:00+00:00".into()),
            last_modified: Some("Fri, 15 Mar 2024 14:30:00 +0000".into()),
            ..Default::default()
        };

        let event = build_file_event(
            &entry,
            "alice",
            "https://nc.local",
            "run-1",
            "src-1",
            "cnt-1",
            None,
            false,
        );

        match event {
            ConnectorEvent::DocumentCreated { metadata, .. } => {
                assert!(metadata.created_at.is_some(), "created_at should be parsed");
                assert!(metadata.updated_at.is_some(), "updated_at should be parsed");
                assert_eq!(metadata.created_at.unwrap().year(), 2024);
                assert_eq!(metadata.updated_at.unwrap().year(), 2024);
            }
            _ => panic!("Expected DocumentCreated"),
        }
    }
}

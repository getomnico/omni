use anyhow::{anyhow, Context, Result};
use dashmap::DashMap;
use shared::models::{SourceType, SyncRequest};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use tracing::{error, info, warn};

use crate::models::FileSystemSource;
use crate::scanner::FileSystemScanner;
use shared::SdkClient;

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
            "Starting filesystem sync for source: {} (sync_run_id: {})",
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
        if source.source_type != SourceType::LocalFiles
            && source.source_type != SourceType::FileSystem
        {
            return Err(anyhow!(
                "Invalid source type for filesystem connector: {:?}",
                source.source_type
            ));
        }

        let fs_source = Self::parse_filesystem_source(&source)?;

        let cancelled = Arc::new(AtomicBool::new(false));
        self.active_syncs
            .insert(sync_run_id.to_string(), cancelled.clone());

        let result = self
            .execute_sync(&fs_source, source_id, sync_run_id, &cancelled)
            .await;

        if cancelled.load(Ordering::SeqCst) {
            info!("Filesystem sync {} was cancelled", sync_run_id);
            let _ = self.sdk_client.cancel(sync_run_id).await;
            self.active_syncs.remove(sync_run_id);
            return Ok(());
        }

        self.active_syncs.remove(sync_run_id);

        match result {
            Ok((total_scanned, total_processed)) => {
                info!(
                    "Filesystem sync completed for source {}: {} scanned, {} processed",
                    source.name, total_scanned, total_processed
                );
                self.sdk_client
                    .complete(
                        sync_run_id,
                        total_scanned as i32,
                        total_processed as i32,
                        None,
                    )
                    .await?;
                Ok(())
            }
            Err(e) => {
                error!("Filesystem sync failed for source {}: {}", source.name, e);
                Err(e)
            }
        }
    }

    fn parse_filesystem_source(
        source: &shared::models::Source,
    ) -> Result<FileSystemSource> {
        let config = &source.config;

        let base_path = config
            .get("base_path")
            .and_then(|v| v.as_str())
            .context("Missing base_path in config")?;

        let scan_interval_seconds = config
            .get("scan_interval_seconds")
            .and_then(|v| v.as_u64())
            .unwrap_or(300);

        let file_extensions = config
            .get("file_extensions")
            .and_then(|v| v.as_array())
            .map(|arr| {
                arr.iter()
                    .filter_map(|v| v.as_str().map(|s| s.to_string()))
                    .collect()
            });

        let exclude_patterns = config
            .get("exclude_patterns")
            .and_then(|v| v.as_array())
            .map(|arr| {
                arr.iter()
                    .filter_map(|v| v.as_str().map(|s| s.to_string()))
                    .collect()
            });

        let max_file_size_bytes = config.get("max_file_size_bytes").and_then(|v| v.as_u64());

        Ok(FileSystemSource {
            id: source.id.clone(),
            name: source.name.clone(),
            base_path: PathBuf::from(base_path),
            scan_interval_seconds,
            file_extensions,
            exclude_patterns,
            max_file_size_bytes,
        })
    }

    async fn execute_sync(
        &self,
        fs_source: &FileSystemSource,
        source_id: &str,
        sync_run_id: &str,
        cancelled: &AtomicBool,
    ) -> Result<(usize, usize)> {
        let scanner = FileSystemScanner::new(fs_source.clone());

        let files = scanner.scan_directory().await?;
        let total_scanned = files.len();
        let mut total_processed = 0;

        info!("Found {} files to process", total_scanned);

        for file in files {
            if cancelled.load(Ordering::SeqCst) {
                info!("Sync cancelled, stopping scan");
                break;
            }

            let file_path = file.path.clone();
            let file_name = file.name.clone();
            let mime_type = file.mime_type.clone();

            // Read raw file bytes for content extraction
            let data = match std::fs::read(&file_path) {
                Ok(d) => d,
                Err(e) => {
                    warn!("Failed to read file {}: {}", file_path.display(), e);
                    continue;
                }
            };

            // Use SDK to extract and store content
            let content_id = match self
                .sdk_client
                .extract_and_store_content(
                    sync_run_id,
                    data,
                    &mime_type,
                    Some(&file_name),
                )
                .await
            {
                Ok(id) => id,
                Err(e) => {
                    warn!(
                        "Failed to extract/store content for {}: {}",
                        file_path.display(),
                        e
                    );
                    continue;
                }
            };

            // Convert to connector event and emit
            let event =
                file.to_connector_event(sync_run_id.to_string(), source_id.to_string(), content_id);

            if let Err(e) = self.sdk_client.emit_event(sync_run_id, source_id, event).await {
                error!("Failed to emit event for {}: {}", file_path.display(), e);
                continue;
            }

            total_processed += 1;

            if total_processed % 100 == 0 {
                info!("Processed {} files", total_processed);
                let _ = self
                    .sdk_client
                    .increment_scanned(sync_run_id, 100)
                    .await;
            }
        }

        Ok((total_scanned, total_processed))
    }
}

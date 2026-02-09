use anyhow::{Context, Result};
use sqlx::PgPool;
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use tokio::sync::mpsc;
use tokio::time::{interval, Duration};
use tracing::{error, info, warn};

use crate::models::FileSystemSource;
use crate::scanner::FileSystemScanner;
use crate::watcher::{FileSystemEventProcessor, FileSystemWatcher};
use shared::db::repositories::SyncRunRepository;
use shared::models::{Source, SyncType};
use shared::queue::EventQueue;
use shared::ObjectStorage;

pub struct FileSystemSyncManager {
    pool: PgPool,
    event_queue: EventQueue,
    content_storage: Arc<dyn ObjectStorage>,
    sources: HashMap<String, FileSystemSource>,
}

impl FileSystemSyncManager {
    pub async fn new(pool: PgPool, event_queue: EventQueue) -> Result<Self> {
        let content_storage = shared::StorageFactory::from_env(pool.clone()).await?;
        Ok(Self {
            pool,
            event_queue,
            content_storage,
            sources: HashMap::new(),
        })
    }

    pub async fn load_sources(&mut self) -> Result<()> {
        info!("Loading filesystem sources from database");
        // TODO: Implement actual database query when testing with real database
        // For now, just log that we would load sources
        info!("Would load filesystem sources from database");
        Ok(())
    }

    fn parse_filesystem_source(&self, source: &Source) -> Result<FileSystemSource> {
        let config = &source.config;

        let base_path = config
            .get("base_path")
            .and_then(|v| v.as_str())
            .context("Missing base_path in config")?;

        let scan_interval_seconds = config
            .get("scan_interval_seconds")
            .and_then(|v| v.as_u64())
            .unwrap_or(300); // Default 5 minutes

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

    pub async fn start_sync_manager(&self) -> Result<()> {
        info!("Starting filesystem sync manager");

        let mut tasks = Vec::new();

        // Start a sync task for each source
        for (source_id, source) in &self.sources {
            let source = source.clone();
            let pool = self.pool.clone();
            let event_queue = self.event_queue.clone();
            let source_id = source_id.clone();

            let task = tokio::spawn(async move {
                if let Err(e) = Self::run_source_sync(source, pool, event_queue, source_id).await {
                    error!("Source sync task failed: {}", e);
                }
            });

            tasks.push(task);
        }

        // Wait for all tasks to complete (which should be never in normal operation)
        for task in tasks {
            if let Err(e) = task.await {
                error!("Sync task panicked: {}", e);
            }
        }

        Ok(())
    }

    async fn run_source_sync(
        source: FileSystemSource,
        pool: PgPool,
        event_queue: EventQueue,
        source_id: String,
    ) -> Result<()> {
        info!("Starting sync for filesystem source: {}", source.name);

        // Create content storage for the event processor
        let content_storage = shared::StorageFactory::from_env(pool.clone()).await?;

        // Create channel for file system events
        let (event_sender, event_receiver) = mpsc::unbounded_channel();

        // Start the file watcher
        let watcher = FileSystemWatcher::new(source.clone(), event_sender);
        let mut watcher_task = {
            let watcher = watcher;
            tokio::spawn(async move {
                if let Err(e) = watcher.start_watching().await {
                    error!("File watcher failed: {}", e);
                }
            })
        };

        // Create scanner for the event processor
        let processor_scanner = FileSystemScanner::new(source.clone());

        // Default idle timeout: 30 seconds
        let idle_timeout_secs = 30;

        // Start the event processor with all dependencies
        let mut event_processor = FileSystemEventProcessor::new(
            event_receiver,
            processor_scanner,
            event_queue.clone(),
            content_storage.clone(),
            source_id.clone(),
            pool.clone(),
            idle_timeout_secs,
        );
        let mut processor_task = tokio::spawn(async move {
            if let Err(e) = event_processor.process_events().await {
                error!("Event processor failed: {}", e);
            }
        });

        // Start periodic full scans
        let scanner = FileSystemScanner::new(source.clone());
        let scan_interval = Duration::from_secs(source.scan_interval_seconds);
        let mut scan_timer = interval(scan_interval);

        loop {
            tokio::select! {
                _ = scan_timer.tick() => {
                    info!("Starting periodic scan for source: {}", source.name);
                    // Create content storage for this task
                    let content_storage = match shared::StorageFactory::from_env(pool.clone()).await {
                        Ok(storage) => storage,
                        Err(e) => {
                            error!("Failed to create content storage: {}", e);
                            continue;
                        }
                    };
                    if let Err(e) = Self::perform_full_scan(&scanner, &pool, &event_queue, &source_id, &content_storage).await {
                        error!("Full scan failed for source {}: {}", source.name, e);
                    }
                }
                _ = &mut watcher_task => {
                    error!("File watcher task exited unexpectedly");
                    break;
                }
                _ = &mut processor_task => {
                    error!("Event processor task exited unexpectedly");
                    break;
                }
            }
        }

        Ok(())
    }

    async fn perform_full_scan(
        scanner: &FileSystemScanner,
        pool: &PgPool,
        event_queue: &EventQueue,
        source_id: &str,
        content_storage: &Arc<dyn ObjectStorage>,
    ) -> Result<()> {
        let sync_run_repo = SyncRunRepository::new(pool);
        let sync_run = sync_run_repo
            .create(source_id, SyncType::Full, "manual")
            .await?;

        info!("Starting full scan with sync_run_id: {}", sync_run.id);

        let result: Result<usize> = async {
            // Scan the filesystem
            let files = scanner.scan_directory().await?;
            let mut files_processed = 0;

            info!("Found {} files to process", files.len());

            for file in files {
                let file_path = file.path.clone();

                // Read file content
                let content = match scanner.read_file_content(&file).await {
                    Ok(content) => content,
                    Err(e) => {
                        warn!("Failed to read content for {}: {}", file_path.display(), e);
                        continue;
                    }
                };

                // Store content in LOB and get OID
                let content_id = match content_storage.store_text(&content, None).await {
                    Ok(oid) => oid,
                    Err(e) => {
                        error!(
                            "Failed to store content in LOB storage for file {}: {}",
                            file_path.display(),
                            e
                        );
                        continue;
                    }
                };

                // Convert to connector event
                let event =
                    file.to_connector_event(sync_run.id.clone(), source_id.to_string(), content_id);

                // Queue the event
                if let Err(e) = event_queue.enqueue(source_id, &event).await {
                    error!("Failed to queue event for {}: {}", file_path.display(), e);
                    continue;
                }

                files_processed += 1;

                if files_processed % 100 == 0 {
                    info!("Processed {} files", files_processed);
                    // Update scanned count in batches
                    sync_run_repo.increment_scanned(&sync_run.id, 100).await?;
                }
            }

            // Update remaining count (files_processed % 100)
            let remaining = files_processed % 100;
            if remaining > 0 {
                sync_run_repo
                    .increment_scanned(&sync_run.id, remaining as i32)
                    .await?;
            }

            info!(
                "Completed full scan for source_id: {}, processed {} files",
                source_id, files_processed
            );

            Ok(files_processed)
        }
        .await;

        match &result {
            Ok(files_processed) => {
                sync_run_repo
                    .mark_completed(
                        &sync_run.id,
                        *files_processed as i32,
                        *files_processed as i32,
                    )
                    .await?;
            }
            Err(e) => {
                sync_run_repo
                    .mark_failed(&sync_run.id, &e.to_string())
                    .await?;
            }
        }

        result.map(|_| ())
    }
}

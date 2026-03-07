use crate::models::FileSystemSource;
use crate::scanner::FileSystemScanner;
use anyhow::Result;
use notify::{Config, Event, EventKind, RecursiveMode, Watcher};
use shared::db::repositories::SyncRunRepository;
use shared::models::{ConnectorEvent, DocumentMetadata, DocumentPermissions, SyncRun, SyncType};
use shared::queue::EventQueue;
use shared::ObjectStorage;
use sqlx::PgPool;
use std::path::PathBuf;
use std::sync::mpsc;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::mpsc as tokio_mpsc;
use tokio::time::{interval, Instant};
use tracing::{debug, error, info, warn};

pub struct FileSystemWatcher {
    source: FileSystemSource,
    event_sender: tokio_mpsc::UnboundedSender<FileSystemEvent>,
}

#[derive(Debug, Clone)]
pub enum FileSystemEvent {
    FileCreated(PathBuf),
    FileModified(PathBuf),
    FileDeleted(PathBuf),
}

impl FileSystemWatcher {
    pub fn new(
        source: FileSystemSource,
        event_sender: tokio_mpsc::UnboundedSender<FileSystemEvent>,
    ) -> Self {
        Self {
            source,
            event_sender,
        }
    }

    pub async fn start_watching(&self) -> Result<()> {
        info!(
            "Starting filesystem watcher for source: {} at path: {}",
            self.source.name,
            self.source.base_path.display()
        );

        // Create a channel for the file watcher
        let (tx, rx) = mpsc::channel();
        let event_sender = self.event_sender.clone();
        let source = self.source.clone();

        // Spawn a blocking task to handle the file watcher
        let watcher_task = tokio::task::spawn_blocking(move || {
            // Use PollWatcher for better compatibility with network filesystems
            let config = Config::default()
                .with_poll_interval(Duration::from_secs(2))
                .with_compare_contents(true);

            let mut watcher = notify::PollWatcher::new(
                move |result: notify::Result<Event>| {
                    if let Err(e) = tx.send(result) {
                        error!("Failed to send file watcher event: {}", e);
                    }
                },
                config,
            )?;

            // Start watching the base path recursively
            watcher.watch(&source.base_path, RecursiveMode::Recursive)?;

            info!("File watcher started successfully");

            // Keep the watcher alive and process events
            for event_result in rx {
                match event_result {
                    Ok(event) => {
                        if let Err(e) = Self::process_file_event(&event, &source, &event_sender) {
                            error!("Failed to process file event: {}", e);
                        }
                    }
                    Err(e) => {
                        error!("File watcher error: {}", e);
                    }
                }
            }

            Ok::<(), anyhow::Error>(())
        });

        // Wait for the watcher task to complete (which should be never in normal operation)
        match watcher_task.await {
            Ok(Ok(())) => {
                warn!("File watcher task completed unexpectedly");
            }
            Ok(Err(e)) => {
                error!("File watcher task failed: {}", e);
                return Err(e);
            }
            Err(e) => {
                error!("File watcher task panicked: {}", e);
                return Err(anyhow::anyhow!("File watcher task panicked: {}", e));
            }
        }

        Ok(())
    }

    fn process_file_event(
        event: &Event,
        source: &FileSystemSource,
        event_sender: &tokio_mpsc::UnboundedSender<FileSystemEvent>,
    ) -> Result<()> {
        debug!("Processing file event: {:?}", event);

        for path in &event.paths {
            // Check if this file should be included based on our filters
            if !source.should_include_file(path) {
                debug!("Skipping file event due to filters: {}", path.display());
                continue;
            }

            // Skip directories
            if path.is_dir() {
                continue;
            }

            let filesystem_event = match event.kind {
                EventKind::Create(_) => {
                    debug!("File created: {}", path.display());
                    FileSystemEvent::FileCreated(path.clone())
                }
                EventKind::Modify(_) => {
                    debug!("File modified: {}", path.display());
                    FileSystemEvent::FileModified(path.clone())
                }
                EventKind::Remove(_) => {
                    debug!("File deleted: {}", path.display());
                    FileSystemEvent::FileDeleted(path.clone())
                }
                _ => {
                    // Other event types we don't care about
                    continue;
                }
            };

            if let Err(e) = event_sender.send(filesystem_event) {
                error!("Failed to send filesystem event: {}", e);
            }
        }

        Ok(())
    }
}

pub struct FileSystemEventProcessor {
    event_receiver: tokio_mpsc::UnboundedReceiver<FileSystemEvent>,
    scanner: FileSystemScanner,
    event_queue: EventQueue,
    content_storage: Arc<dyn ObjectStorage>,
    source_id: String,
    pool: PgPool,
    // Batched sync_run state
    current_sync_run: Option<SyncRun>,
    idle_timeout: Duration,
    last_event_time: Instant,
    events_in_batch: i32,
}

impl FileSystemEventProcessor {
    pub fn new(
        event_receiver: tokio_mpsc::UnboundedReceiver<FileSystemEvent>,
        scanner: FileSystemScanner,
        event_queue: EventQueue,
        content_storage: Arc<dyn ObjectStorage>,
        source_id: String,
        pool: PgPool,
        idle_timeout_secs: u64,
    ) -> Self {
        Self {
            event_receiver,
            scanner,
            event_queue,
            content_storage,
            source_id,
            pool,
            current_sync_run: None,
            idle_timeout: Duration::from_secs(idle_timeout_secs),
            last_event_time: Instant::now(),
            events_in_batch: 0,
        }
    }

    pub async fn process_events(&mut self) -> Result<()> {
        info!("Starting filesystem event processor");

        let mut idle_check_interval = interval(Duration::from_secs(5));

        loop {
            tokio::select! {
                Some(event) = self.event_receiver.recv() => {
                    self.last_event_time = Instant::now();
                    if let Err(e) = self.handle_event(event).await {
                        error!("Failed to handle filesystem event: {}", e);
                    }
                }
                _ = idle_check_interval.tick() => {
                    self.maybe_commit_sync_run().await;
                }
            }
        }
    }

    async fn get_or_create_sync_run(&mut self) -> Result<String> {
        if let Some(ref run) = self.current_sync_run {
            return Ok(run.id.clone());
        }

        let sync_run_repo = SyncRunRepository::new(&self.pool);
        let sync_run = sync_run_repo
            .create(&self.source_id, SyncType::Incremental, "manual")
            .await?;

        info!(
            "Created new incremental sync_run for realtime events: {}",
            sync_run.id
        );

        self.current_sync_run = Some(sync_run.clone());
        self.events_in_batch = 0;

        Ok(sync_run.id)
    }

    async fn maybe_commit_sync_run(&mut self) {
        if self.current_sync_run.is_none() {
            return;
        }

        let elapsed = self.last_event_time.elapsed();
        if elapsed < self.idle_timeout {
            return;
        }

        if let Some(ref run) = self.current_sync_run {
            let sync_run_repo = SyncRunRepository::new(&self.pool);
            match sync_run_repo
                .mark_completed(&run.id, self.events_in_batch, self.events_in_batch)
                .await
            {
                Ok(_) => {
                    info!(
                        "Committed incremental sync_run {} with {} events",
                        run.id, self.events_in_batch
                    );
                }
                Err(e) => {
                    error!("Failed to commit sync_run {}: {}", run.id, e);
                }
            }
        }

        self.current_sync_run = None;
        self.events_in_batch = 0;
    }

    async fn handle_event(&mut self, event: FileSystemEvent) -> Result<()> {
        match event {
            FileSystemEvent::FileCreated(path) => {
                info!("Handling file creation: {}", path.display());
                self.handle_file_created_or_modified(&path, true).await?;
            }
            FileSystemEvent::FileModified(path) => {
                info!("Handling file modification: {}", path.display());
                self.handle_file_created_or_modified(&path, false).await?;
            }
            FileSystemEvent::FileDeleted(path) => {
                info!("Handling file deletion: {}", path.display());
                self.handle_file_deleted(&path).await?;
            }
        }

        Ok(())
    }

    async fn handle_file_created_or_modified(
        &mut self,
        path: &PathBuf,
        is_created: bool,
    ) -> Result<()> {
        // Get or create sync_run for this batch
        let sync_run_id = self.get_or_create_sync_run().await?;

        // Get file info
        let file = match self.scanner.get_file_info(path).await? {
            Some(f) => f,
            None => {
                debug!("Skipping file (filtered or directory): {}", path.display());
                return Ok(());
            }
        };

        // Read file content
        let content = self.scanner.read_file_content(&file).await?;
        if content.is_empty() {
            debug!("Skipping file with empty content: {}", path.display());
            return Ok(());
        }

        // Store content in object storage
        let content_id = self
            .content_storage
            .store_text(&content, None)
            .await
            .map_err(|e| anyhow::anyhow!("Failed to store content: {}", e))?;

        // Create the connector event
        let document_id = path.to_string_lossy().to_string();

        let event = if is_created {
            file.to_connector_event(sync_run_id, self.source_id.clone(), content_id)
        } else {
            // For updates, we create a DocumentUpdated event
            ConnectorEvent::DocumentUpdated {
                sync_run_id,
                source_id: self.source_id.clone(),
                document_id: document_id.clone(),
                content_id,
                metadata: DocumentMetadata {
                    title: Some(file.name),
                    author: None,
                    created_at: file.created_time.and_then(|t| {
                        t.duration_since(std::time::SystemTime::UNIX_EPOCH)
                            .ok()
                            .and_then(|d| {
                                time::OffsetDateTime::from_unix_timestamp(d.as_secs() as i64).ok()
                            })
                    }),
                    updated_at: file.modified_time.and_then(|t| {
                        t.duration_since(std::time::SystemTime::UNIX_EPOCH)
                            .ok()
                            .and_then(|d| {
                                time::OffsetDateTime::from_unix_timestamp(d.as_secs() as i64).ok()
                            })
                    }),
                    content_type: None,
                    mime_type: Some(file.mime_type),
                    size: Some(file.size.to_string()),
                    url: None,
                    path: Some(file.path.to_string_lossy().to_string()),
                    extra: None,
                },
                permissions: Some(DocumentPermissions {
                    public: true,
                    users: vec![],
                    groups: vec![],
                }),
                attributes: None,
            }
        };

        // Queue the event
        self.event_queue.enqueue(&self.source_id, &event).await?;
        self.events_in_batch += 1;

        info!(
            "Queued {} event for file: {}",
            if is_created { "create" } else { "update" },
            path.display()
        );

        Ok(())
    }

    async fn handle_file_deleted(&mut self, path: &PathBuf) -> Result<()> {
        // Get or create sync_run for this batch
        let sync_run_id = self.get_or_create_sync_run().await?;

        let document_id = path.to_string_lossy().to_string();

        let event = ConnectorEvent::DocumentDeleted {
            sync_run_id,
            source_id: self.source_id.clone(),
            document_id: document_id.clone(),
        };

        // Queue the event
        self.event_queue.enqueue(&self.source_id, &event).await?;
        self.events_in_batch += 1;

        info!("Queued delete event for file: {}", path.display());

        Ok(())
    }
}

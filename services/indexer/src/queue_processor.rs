use crate::{lexeme_refresh, AppState};
use anyhow::Result;
use futures::future::join_all;
use pgvector::Vector;
use shared::db::repositories::{DocumentRepository, EmbeddingRepository};
use shared::models::{ConnectorEvent, Document, DocumentMetadata, DocumentPermissions, Embedding};
use shared::queue::EventQueue;
use sqlx::postgres::PgListener;
use std::sync::Arc;
use tokio::sync::Semaphore;
use tokio::time::{interval, Duration};
use tracing::{error, info, warn};
use ulid::Ulid;

pub struct QueueProcessor {
    pub state: AppState,
    pub event_queue: EventQueue,
    pub batch_size: i32,
    pub parallelism: usize,
    semaphore: Arc<Semaphore>,
}

impl QueueProcessor {
    pub fn new(state: AppState) -> Self {
        let event_queue = EventQueue::new(state.db_pool.pool().clone());
        let parallelism = (num_cpus::get() / 2).max(1); // Half the CPU cores, minimum 1
        let semaphore = Arc::new(Semaphore::new(parallelism));
        Self {
            state,
            event_queue,
            batch_size: 10,
            parallelism,
            semaphore,
        }
    }

    pub fn with_parallelism(mut self, parallelism: usize) -> Self {
        self.parallelism = parallelism;
        self.semaphore = Arc::new(Semaphore::new(parallelism));
        self
    }

    pub async fn start(&self) -> Result<()> {
        info!(
            "Starting queue processor with batch size: {}, parallelism: {}",
            self.batch_size, self.parallelism
        );

        let mut listener = PgListener::connect_with(self.state.db_pool.pool()).await?;
        listener.listen("indexer_queue").await?;

        let mut poll_interval = interval(Duration::from_secs(60)); // Backup polling every minute
        let mut heartbeat_interval = interval(Duration::from_secs(300));
        let mut retry_interval = interval(Duration::from_secs(300)); // 5 minutes
        let mut cleanup_interval = interval(Duration::from_secs(3600)); // 1 hour

        // Process any existing events first
        if let Err(e) = self.process_batch().await {
            error!("Failed to process initial batch: {}", e);
        }

        loop {
            tokio::select! {
                notification = listener.recv() => {
                    match notification {
                        Ok(_) => {
                            if let Err(e) = self.process_batch().await {
                                error!("Failed to process batch after notification: {}", e);
                            }
                        }
                        Err(e) => {
                            error!("Failed to receive notification: {}", e);
                            // Reconnect listener
                            if let Ok(mut new_listener) = PgListener::connect_with(self.state.db_pool.pool()).await {
                                if new_listener.listen("indexer_queue").await.is_ok() {
                                    listener = new_listener;
                                    info!("Reconnected to notification listener");
                                }
                            }
                        }
                    }
                }
                _ = poll_interval.tick() => {
                    // Backup polling mechanism
                    if let Err(e) = self.process_batch().await {
                        error!("Failed to process batch during backup poll: {}", e);
                    }
                }
                _ = heartbeat_interval.tick() => {
                    if let Ok(stats) = self.event_queue.get_queue_stats().await {
                        info!(
                            "Queue stats - Pending: {}, Processing: {}, Completed: {}, Failed: {}, Dead Letter: {}",
                            stats.pending, stats.processing, stats.completed, stats.failed, stats.dead_letter
                        );
                    }
                }
                _ = retry_interval.tick() => {
                    if let Ok(retried) = self.event_queue.retry_failed_events().await {
                        if retried > 0 {
                            info!("Retried {} failed events", retried);
                        }
                    }
                }
                _ = cleanup_interval.tick() => {
                    if let Ok(result) = self.event_queue.cleanup_old_events(7).await {
                        if result.completed_deleted > 0 || result.dead_letter_deleted > 0 {
                            info!(
                                "Cleaned up old events - Completed: {}, Dead Letter: {}",
                                result.completed_deleted, result.dead_letter_deleted
                            );
                        }
                    }
                }
            }
        }
    }

    async fn process_batch(&self) -> Result<()> {
        let events = self.event_queue.dequeue_batch(self.batch_size).await?;

        if events.is_empty() {
            return Ok(());
        }

        info!(
            "Processing batch of {} events with parallelism: {}",
            events.len(),
            self.parallelism
        );

        // Process events concurrently
        let mut tasks = Vec::new();

        for event_item in events {
            let event_id = event_item.id.clone();
            let _sync_run_id = event_item.sync_run_id.clone();
            let payload = event_item.payload.clone();
            let state = self.state.clone();
            let event_queue = self.event_queue.clone();
            let semaphore = self.semaphore.clone();

            let task = tokio::spawn(async move {
                // Acquire semaphore permit to limit concurrency
                let _permit = semaphore.acquire().await.unwrap();

                info!("Processing event {} on task thread", event_id);

                let processor = ProcessorContext::new(state);
                match processor.process_event(&payload).await {
                    Ok(_) => {
                        if let Err(e) = event_queue.mark_completed(&event_id).await {
                            error!("Failed to mark event {} as completed: {}", event_id, e);
                            false
                        } else {
                            true
                        }
                    }
                    Err(e) => {
                        error!("Failed to process event {}: {}", event_id, e);
                        if let Err(mark_err) =
                            event_queue.mark_failed(&event_id, &e.to_string()).await
                        {
                            error!("Failed to mark event {} as failed: {}", event_id, mark_err);
                        }
                        false
                    }
                }
            });

            tasks.push(task);
        }

        // Wait for all tasks to complete
        let results = join_all(tasks).await;

        // Count successful processes
        let processed_count = results
            .iter()
            .filter_map(|r| r.as_ref().ok())
            .filter(|&&success| success)
            .count();

        // After processing events, refresh lexemes if any documents were processed
        if processed_count > 0 {
            info!(
                "Refreshing lexemes after processing {} events",
                processed_count
            );
            if let Err(e) = lexeme_refresh::refresh_lexemes(&self.state.db_pool).await {
                error!("Failed to refresh lexemes after batch processing: {}", e);
            } else {
                info!("Lexeme refresh completed after batch processing");
            }
        }

        Ok(())
    }
}

// Context for processing individual events concurrently
struct ProcessorContext {
    state: AppState,
}

impl ProcessorContext {
    fn new(state: AppState) -> Self {
        Self { state }
    }

    async fn process_event(&self, payload: &serde_json::Value) -> Result<()> {
        let event: ConnectorEvent = serde_json::from_value(payload.clone())?;
        let sync_run_id = event.sync_run_id().to_string();

        // Update sync run progress
        if let Err(e) = self.increment_sync_run_progress(&sync_run_id).await {
            warn!(
                "Failed to update sync run progress for {}: {}",
                sync_run_id, e
            );
        }

        match event {
            ConnectorEvent::DocumentCreated {
                sync_run_id: _,
                source_id,
                document_id,
                content,
                metadata,
                permissions,
            } => {
                self.handle_document_created(
                    source_id,
                    document_id,
                    content,
                    metadata,
                    permissions,
                )
                .await?;
            }
            ConnectorEvent::DocumentUpdated {
                sync_run_id: _,
                source_id,
                document_id,
                content,
                metadata,
                permissions,
            } => {
                self.handle_document_updated(
                    source_id,
                    document_id,
                    content,
                    metadata,
                    permissions,
                )
                .await?;
            }
            ConnectorEvent::DocumentDeleted {
                sync_run_id: _,
                source_id,
                document_id,
            } => {
                self.handle_document_deleted(source_id, document_id).await?;
            }
        }

        Ok(())
    }

    async fn handle_document_created(
        &self,
        source_id: String,
        document_id: String,
        content: String,
        metadata: DocumentMetadata,
        permissions: DocumentPermissions,
    ) -> Result<()> {
        info!(
            "Processing document created/updated: {} from source {}",
            document_id, source_id
        );

        let now = sqlx::types::time::OffsetDateTime::now_utc();
        let metadata_json = serde_json::to_value(&metadata)?;
        let permissions_json = serde_json::to_value(&permissions)?;

        // Extract file extension from URL or mime type
        let file_extension = metadata.url.as_ref().and_then(|url| {
            url.split('.')
                .last()
                .filter(|ext| !ext.contains('/') && !ext.contains('?'))
                .map(|ext| ext.to_lowercase())
        });

        // Parse file size from string to i64
        let file_size = metadata
            .size
            .as_ref()
            .and_then(|size_str| size_str.parse::<i64>().ok());

        let document = Document {
            id: ulid::Ulid::new().to_string(),
            source_id: source_id.clone(),
            external_id: document_id.clone(),
            title: metadata.title.unwrap_or_else(|| "Untitled".to_string()),
            content: Some(content.clone()),
            content_type: metadata.mime_type.clone(),
            file_size,
            file_extension,
            url: metadata.url.clone(),
            parent_id: metadata.parent_id.clone(),
            metadata: metadata_json,
            permissions: permissions_json,
            created_at: now,
            updated_at: now,
            last_indexed_at: now,
        };

        let repo = DocumentRepository::new(self.state.db_pool.pool());
        let upserted = repo.upsert(document).await?;

        repo.update_search_vector(&upserted.id).await?;

        // Generate embeddings for the document
        if let Err(e) = self.generate_embeddings(&upserted).await {
            error!(
                "Failed to generate embeddings for document {} (title: '{}', content length: {}): {}",
                document_id,
                upserted.title,
                content.len(),
                e
            );
            // Log more details about the failure
            warn!(
                "Document {} embedding failure details - source: {}, external_id: {}, content_type: {:?}",
                document_id, source_id, document_id, metadata.mime_type
            );
            // Don't fail the entire operation if embeddings fail
        }

        repo.mark_as_indexed(&upserted.id).await?;

        info!("Document upserted successfully: {}", document_id);
        Ok(())
    }

    async fn handle_document_updated(
        &self,
        source_id: String,
        document_id: String,
        content: String,
        metadata: DocumentMetadata,
        permissions: Option<DocumentPermissions>,
    ) -> Result<()> {
        info!(
            "Processing document updated: {} from source {}",
            document_id, source_id
        );

        let repo = DocumentRepository::new(self.state.db_pool.pool());

        if let Some(mut document) = repo.find_by_external_id(&source_id, &document_id).await? {
            let now = sqlx::types::time::OffsetDateTime::now_utc();
            let metadata_json = serde_json::to_value(&metadata)?;
            let doc_id = document.id.clone();

            document.title = metadata.title.unwrap_or(document.title);
            document.content = Some(content);
            document.metadata = metadata_json;
            if let Some(perms) = permissions {
                document.permissions = serde_json::to_value(&perms)?;
            }
            document.updated_at = now;

            let updated_document = repo.update(&doc_id, document).await?;
            repo.update_search_vector(&doc_id).await?;

            // Generate embeddings for the updated document
            if let Some(updated_doc) = &updated_document {
                if let Err(e) = self.generate_embeddings(updated_doc).await {
                    error!(
                        "Failed to generate embeddings for updated document {}: {}",
                        document_id, e
                    );
                    // Don't fail the entire operation if embeddings fail
                }
            }

            repo.mark_as_indexed(&doc_id).await?;

            info!("Document updated successfully: {}", document_id);
        } else {
            warn!(
                "Document not found for update: {} from source {}",
                document_id, source_id
            );
        }

        Ok(())
    }

    async fn handle_document_deleted(&self, source_id: String, document_id: String) -> Result<()> {
        info!(
            "Processing document deleted: {} from source {}",
            document_id, source_id
        );

        let repo = DocumentRepository::new(self.state.db_pool.pool());

        if let Some(document) = repo.find_by_external_id(&source_id, &document_id).await? {
            // Delete embeddings first
            let embedding_repo = EmbeddingRepository::new(self.state.db_pool.pool());
            embedding_repo.delete_by_document_id(&document.id).await?;

            // Then delete the document
            repo.delete(&document.id).await?;
            info!(
                "Document and embeddings deleted successfully: {}",
                document_id
            );
        } else {
            warn!(
                "Document not found for deletion: {} from source {}",
                document_id, source_id
            );
        }

        Ok(())
    }

    /// Generate embeddings for a document by chunking and calling the AI service
    async fn generate_embeddings(&self, document: &Document) -> Result<()> {
        // Skip documents without content
        if document.content.is_none() || document.content.as_ref().unwrap().trim().is_empty() {
            info!(
                "Skipping embedding generation for document {} - no content",
                document.id
            );
            return Ok(());
        }

        info!("Generating embeddings for document: {}", document.id);

        // First, delete existing embeddings for this document
        let embedding_repo = EmbeddingRepository::new(self.state.db_pool.pool());
        embedding_repo.delete_by_document_id(&document.id).await?;

        // Generate embeddings using AI service's semantic chunking
        let content = match &document.content {
            Some(content) => content.clone(),
            None => {
                info!("No content to embed for document {}", document.id);
                return Ok(());
            }
        };

        info!("Generating embeddings for document {}", document.id);

        // Use AI service to generate embeddings with semantic chunking
        let text_embeddings = match self
            .state
            .ai_client
            .generate_embeddings_with_options(
                &[content],
                Some("retrieval.passage".to_string()),
                None, // no chunk size needed for semantic chunking
                Some("semantic".to_string()),
            )
            .await
        {
            Ok(embeddings) => embeddings,
            Err(e) => {
                error!(
                    "Failed to generate embeddings for document {}: {}",
                    document.id, e
                );
                return Ok(());
            }
        };

        if text_embeddings.is_empty() {
            info!("No embeddings generated for document {}", document.id);
            return Ok(());
        }

        let text_embedding = &text_embeddings[0]; // We only sent one text

        info!(
            "Generated {} chunks for document {}",
            text_embedding.chunk_embeddings.len(),
            document.id
        );

        // Create embedding records from AI service response
        let mut embeddings = Vec::new();
        let mut skipped_chunks = 0;

        for (chunk_index, (chunk_embedding, chunk_span)) in text_embedding
            .chunk_embeddings
            .iter()
            .zip(text_embedding.chunk_spans.iter())
            .enumerate()
        {
            // Validate chunk bounds before creating embedding
            if chunk_span.0 >= chunk_span.1 {
                warn!(
                    "Skipping invalid chunk {} for document {} - invalid bounds: start={}, end={}",
                    chunk_index, document.id, chunk_span.0, chunk_span.1
                );
                skipped_chunks += 1;
                continue;
            }

            let embedding = Embedding {
                id: Ulid::new().to_string(),
                document_id: document.id.clone(),
                chunk_index: chunk_index as i32,
                chunk_start_offset: chunk_span.0,
                chunk_end_offset: chunk_span.1,
                embedding: Vector::from(chunk_embedding.clone()),
                model_name: text_embedding
                    .model_name
                    .clone()
                    .unwrap_or_else(|| "unknown".to_string()),
                created_at: sqlx::types::time::OffsetDateTime::now_utc(),
            };
            embeddings.push(embedding);
        }

        if skipped_chunks > 0 {
            warn!(
                "Skipped {} invalid chunks out of {} total for document {}",
                skipped_chunks,
                text_embedding.chunk_embeddings.len(),
                document.id
            );
        }

        if !embeddings.is_empty() {
            let embedding_count = embeddings.len();
            embedding_repo.bulk_create(embeddings).await?;
            info!(
                "Successfully stored {} embeddings for document {}",
                embedding_count, document.id
            );
        } else {
            warn!("No embeddings were generated for document {}", document.id);
        }

        Ok(())
    }

    async fn increment_sync_run_progress(&self, sync_run_id: &str) -> Result<()> {
        sqlx::query(
            "UPDATE sync_runs 
             SET documents_processed = documents_processed + 1, 
                 updated_at = CURRENT_TIMESTAMP 
             WHERE id = $1",
        )
        .bind(sync_run_id)
        .execute(self.state.db_pool.pool())
        .await?;

        // Notify listeners about sync run progress update
        sqlx::query("NOTIFY sync_run_update")
            .execute(self.state.db_pool.pool())
            .await?;

        Ok(())
    }
}

use crate::{lexeme_refresh, AppState};
use anyhow::Result;
use pgvector::Vector;
use shared::db::repositories::{DocumentRepository, EmbeddingRepository};
use shared::models::{ConnectorEvent, Document, DocumentMetadata, DocumentPermissions, Embedding};
use shared::queue::EventQueue;
use sqlx::postgres::PgListener;
use tokio::time::{interval, Duration};
use tracing::{error, info, warn};
use ulid::Ulid;

pub struct QueueProcessor {
    pub state: AppState,
    pub event_queue: EventQueue,
    pub batch_size: i32,
}

impl QueueProcessor {
    pub fn new(state: AppState) -> Self {
        let event_queue = EventQueue::new(state.db_pool.pool().clone());
        Self {
            state,
            event_queue,
            batch_size: 10,
        }
    }

    pub async fn start(&self) -> Result<()> {
        info!(
            "Starting queue processor with batch size: {}",
            self.batch_size
        );

        let mut listener = PgListener::connect_with(self.state.db_pool.pool()).await?;
        listener.listen("indexer_queue").await?;

        let mut poll_interval = interval(Duration::from_secs(60)); // Backup polling every minute
        let mut heartbeat_interval = interval(Duration::from_secs(30));
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

        let mut processed_count = 0;
        for event_item in events {
            let event_id = event_item.id.clone();

            match self.process_event(&event_item.payload).await {
                Ok(_) => {
                    self.event_queue.mark_completed(&event_id).await?;
                    processed_count += 1;
                }
                Err(e) => {
                    error!("Failed to process event {}: {}", event_id, e);
                    self.event_queue
                        .mark_failed(&event_id, &e.to_string())
                        .await?;
                }
            }
        }

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

    async fn process_event(&self, payload: &serde_json::Value) -> Result<()> {
        let event: ConnectorEvent = serde_json::from_value(payload.clone())?;

        match event {
            ConnectorEvent::DocumentCreated {
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
                "Failed to generate embeddings for document {}: {}",
                document_id, e
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
        const MAX_CHUNK_SIZE: usize = 1000; // Maximum characters per chunk

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

        // Chunk the document content
        let chunks = document.chunk_content(MAX_CHUNK_SIZE);

        if chunks.is_empty() {
            info!("No chunks generated for document {}", document.id);
            return Ok(());
        }

        info!(
            "Generated {} chunks for document {}",
            chunks.len(),
            document.id
        );

        // Generate embeddings for each chunk
        let mut embeddings = Vec::new();

        for chunk in chunks {
            match self.state.ai_client.generate_embedding(&chunk.text).await {
                Ok(embedding_vec) => {
                    let embedding = Embedding {
                        id: Ulid::new().to_string(),
                        document_id: document.id.clone(),
                        chunk_index: chunk.index,
                        chunk_text: chunk.text,
                        embedding: Vector::from(embedding_vec),
                        model_name: "intfloat/e5-large-v2".to_string(),
                        created_at: sqlx::types::time::OffsetDateTime::now_utc(),
                    };
                    embeddings.push(embedding);
                }
                Err(e) => {
                    error!(
                        "Failed to generate embedding for chunk {} of document {}: {}",
                        chunk.index, document.id, e
                    );
                    // Continue with other chunks even if one fails
                }
            }
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
}

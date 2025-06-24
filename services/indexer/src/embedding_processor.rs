use crate::AppState;
use anyhow::Result;
use pgvector::Vector;
use shared::db::repositories::EmbeddingRepository;
use shared::embedding_queue::{EmbeddingQueue, EmbeddingQueueItem};
use shared::models::Embedding;
use sqlx::postgres::PgListener;
use tokio::time::{interval, Duration};
use tracing::{debug, error, info, warn};
use ulid::Ulid;

pub struct EmbeddingProcessor {
    pub state: AppState,
    pub embedding_queue: EmbeddingQueue,
    pub batch_size: i32,
    pub max_content_length: usize,
}

impl EmbeddingProcessor {
    pub fn new(state: AppState) -> Self {
        let embedding_queue = EmbeddingQueue::new(state.db_pool.pool().clone());
        Self {
            state,
            embedding_queue,
            batch_size: 10,              // Process up to 10 documents at once
            max_content_length: 100_000, // Limit content size per batch
        }
    }

    pub async fn start(&self) -> Result<()> {
        info!(
            "Starting embedding processor with batch size: {}",
            self.batch_size
        );

        let mut listener = PgListener::connect_with(self.state.db_pool.pool()).await?;
        listener.listen("embedding_queue").await?;

        let mut poll_interval = interval(Duration::from_secs(30)); // Poll every 30 seconds
        let mut stats_interval = interval(Duration::from_secs(300)); // Log stats every 5 minutes
        let mut cleanup_interval = interval(Duration::from_secs(3600)); // Cleanup every hour

        // Process any existing items first
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
                                if new_listener.listen("embedding_queue").await.is_ok() {
                                    listener = new_listener;
                                    info!("Reconnected to embedding queue listener");
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
                _ = stats_interval.tick() => {
                    if let Ok(stats) = self.embedding_queue.get_queue_stats().await {
                        info!(
                            "Embedding queue stats - Pending: {}, Processing: {}, Completed: {}, Failed: {}",
                            stats.pending, stats.processing, stats.completed, stats.failed
                        );
                    }
                }
                _ = cleanup_interval.tick() => {
                    if let Ok(cleaned) = self.embedding_queue.cleanup_completed(1).await {
                        if cleaned > 0 {
                            info!("Cleaned up {} completed embedding queue items", cleaned);
                        }
                    }
                }
            }
        }
    }

    async fn process_batch(&self) -> Result<()> {
        let items = self.embedding_queue.dequeue_batch(self.batch_size).await?;

        if items.is_empty() {
            return Ok(());
        }

        info!("Processing batch of {} embedding requests", items.len());

        // Group items by size to create optimal batches
        let batches = self.create_batches(items);

        for batch in batches {
            if let Err(e) = self.process_embedding_batch(batch).await {
                error!("Failed to process embedding batch: {}", e);
            }
        }

        Ok(())
    }

    fn create_batches(&self, items: Vec<EmbeddingQueueItem>) -> Vec<Vec<EmbeddingQueueItem>> {
        let mut batches = Vec::new();
        let mut current_batch = Vec::new();
        let mut current_size = 0;

        for item in items {
            let item_size = item.content.len();

            // If this item alone exceeds max size, process it individually
            if item_size > self.max_content_length {
                if !current_batch.is_empty() {
                    batches.push(current_batch);
                    current_batch = Vec::new();
                    current_size = 0;
                }
                batches.push(vec![item]);
                continue;
            }

            // If adding this item would exceed max size, start a new batch
            if current_size + item_size > self.max_content_length && !current_batch.is_empty() {
                batches.push(current_batch);
                current_batch = Vec::new();
                current_size = 0;
            }

            current_batch.push(item);
            current_size += item_size;
        }

        if !current_batch.is_empty() {
            batches.push(current_batch);
        }

        batches
    }

    async fn process_embedding_batch(&self, batch: Vec<EmbeddingQueueItem>) -> Result<()> {
        let start_time = std::time::Instant::now();

        // Extract contents for batch processing
        let contents: Vec<String> = batch.iter().map(|item| item.content.clone()).collect();

        info!(
            "Generating embeddings for {} documents (total size: {} chars)",
            contents.len(),
            contents.iter().map(|c| c.len()).sum::<usize>()
        );

        // Call AI service to generate embeddings for all texts at once
        let ai_service_start = std::time::Instant::now();
        let text_embeddings = match self
            .state
            .ai_client
            .generate_embeddings_with_options(
                &contents,
                Some("retrieval.passage".to_string()),
                Some(512),
                Some("sentence".to_string()),
            )
            .await
        {
            Ok(embeddings) => embeddings,
            Err(e) => {
                error!("Failed to generate embeddings: {}", e);
                // Mark all items as failed
                let ids: Vec<String> = batch.iter().map(|item| item.id.clone()).collect();
                self.embedding_queue
                    .mark_failed_batch(&ids, &e.to_string())
                    .await?;
                return Ok(());
            }
        };
        debug!(
            "AI service batch embedding generation took: {:?}",
            ai_service_start.elapsed()
        );

        // Process results for each document
        let embedding_repo = EmbeddingRepository::new(self.state.db_pool.pool());
        let mut success_ids = Vec::new();
        let mut failed_ids = Vec::new();

        for (item, text_embedding) in batch.iter().zip(text_embeddings.iter()) {
            match self
                .store_embeddings(&embedding_repo, &item.document_id, text_embedding)
                .await
            {
                Ok(_) => {
                    success_ids.push(item.id.clone());
                    debug!(
                        "Successfully stored embeddings for document {} ({} chunks)",
                        item.document_id,
                        text_embedding.chunk_embeddings.len()
                    );
                }
                Err(e) => {
                    error!(
                        "Failed to store embeddings for document {}: {}",
                        item.document_id, e
                    );
                    failed_ids.push(item.id.clone());
                }
            }
        }

        // Mark successful items as completed
        if !success_ids.is_empty() {
            self.embedding_queue.mark_completed(&success_ids).await?;

            // Update document embedding status
            for id in &success_ids {
                if let Some(item) = batch.iter().find(|i| i.id == *id) {
                    self.update_document_embedding_status(&item.document_id, "completed")
                        .await?;
                }
            }
        }

        // Mark failed items
        if !failed_ids.is_empty() {
            self.embedding_queue
                .mark_failed_batch(&failed_ids, "Failed to store embeddings")
                .await?;
        }

        info!(
            "Batch processing completed in {:?} - Success: {}, Failed: {}",
            start_time.elapsed(),
            success_ids.len(),
            failed_ids.len()
        );

        Ok(())
    }

    async fn store_embeddings(
        &self,
        repo: &EmbeddingRepository,
        document_id: &str,
        text_embedding: &shared::clients::ai::TextEmbedding,
    ) -> Result<()> {
        // First, delete existing embeddings for this document
        repo.delete_by_document_id(document_id).await?;

        if text_embedding.chunk_embeddings.is_empty() {
            warn!("No embeddings generated for document {}", document_id);
            return Ok(());
        }

        info!(
            "Storing {} chunks for document {}",
            text_embedding.chunk_embeddings.len(),
            document_id
        );

        // Create embedding records
        let mut embeddings = Vec::new();
        let mut skipped_chunks = 0;

        for (chunk_index, (chunk_embedding, chunk_span)) in text_embedding
            .chunk_embeddings
            .iter()
            .zip(text_embedding.chunk_spans.iter())
            .enumerate()
        {
            // Validate chunk bounds
            if chunk_span.0 >= chunk_span.1 {
                warn!(
                    "Skipping invalid chunk {} for document {} - invalid bounds: start={}, end={}",
                    chunk_index, document_id, chunk_span.0, chunk_span.1
                );
                skipped_chunks += 1;
                continue;
            }

            let embedding = Embedding {
                id: Ulid::new().to_string(),
                document_id: document_id.to_string(),
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
                document_id
            );
        }

        if !embeddings.is_empty() {
            let embedding_count = embeddings.len();
            repo.bulk_create(embeddings).await?;
            debug!(
                "Successfully stored {} embeddings for document {}",
                embedding_count, document_id
            );
        }

        Ok(())
    }

    async fn update_document_embedding_status(
        &self,
        document_id: &str,
        status: &str,
    ) -> Result<()> {
        sqlx::query(
            r#"
            UPDATE documents
            SET embedding_status = $2,
                updated_at = CURRENT_TIMESTAMP
            WHERE id = $1
            "#,
        )
        .bind(document_id)
        .bind(status)
        .execute(self.state.db_pool.pool())
        .await?;

        Ok(())
    }
}

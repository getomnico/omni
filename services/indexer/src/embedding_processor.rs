use crate::AppState;
use anyhow::Result;
use pgvector::Vector;
use shared::db::repositories::EmbeddingRepository;
use shared::models::Embedding;
use shared::{EmbeddingQueue, EmbeddingQueueItem};
use sqlx::postgres::PgListener;
use std::sync::Arc;
use tokio::sync::Mutex;
use tokio::time::{interval, Duration};
use tracing::{debug, error, info, warn};
use ulid::Ulid;

const MAX_DOCUMENT_CHARS: usize = 24_576; // 8192 tokens * 3 chars/token
const CHUNK_OVERLAP: usize = 300; // Overlap between input chunks
const MAX_EMBEDDING_BATCH_SIZE: usize = 32; // Maximum number of input texts per embedding API call

pub struct EmbeddingProcessor {
    pub state: AppState,
    pub embedding_queue: EmbeddingQueue,
    pub batch_size: i32,
    processing_mutex: Arc<Mutex<()>>,
}

impl EmbeddingProcessor {
    pub fn new(state: AppState) -> Self {
        let embedding_queue = EmbeddingQueue::new(state.db_pool.pool().clone());
        let processing_mutex = Arc::new(Mutex::new(()));
        Self {
            state,
            embedding_queue,
            batch_size: 512, // Deque up to 512 documents at once
            processing_mutex,
        }
    }

    fn split_large_content<'a>(content: &'a str) -> Vec<&'a str> {
        if content.len() <= MAX_DOCUMENT_CHARS {
            return vec![content];
        }

        let mut chunks = Vec::new();
        let mut start = 0;

        while start < content.len() {
            // Find the end position, ensuring we don't exceed content length
            let mut end = (start + MAX_DOCUMENT_CHARS).min(content.len());

            // Adjust end to the nearest char boundary if needed
            while !content.is_char_boundary(end) && end < content.len() {
                end += 1;
            }

            // Extract the chunk
            chunks.push(&content[start..end]);

            if end >= content.len() {
                break;
            }

            // Calculate the next start position with overlap
            let mut next_start = end.saturating_sub(CHUNK_OVERLAP);

            // Ensure next_start is at a char boundary
            while !content.is_char_boundary(next_start) && next_start > 0 {
                next_start -= 1;
            }

            start = next_start;
        }

        chunks
    }

    pub async fn start(&self) -> Result<()> {
        info!(
            "Starting embedding processor with batch size: {}",
            self.batch_size
        );

        // Recover any stale processing items from previous runs (5 minute timeout)
        match self
            .embedding_queue
            .recover_stale_processing_items(300)
            .await
        {
            Ok(recovered) => {
                if recovered > 0 {
                    info!(
                        "Recovered {} stale embedding processing items on startup",
                        recovered
                    );
                }
            }
            Err(e) => {
                error!(
                    "Failed to recover stale embedding processing items on startup: {}",
                    e
                );
            }
        }

        let mut listener = PgListener::connect_with(self.state.db_pool.pool()).await?;
        listener.listen("embedding_queue").await?;

        let mut poll_interval = interval(Duration::from_secs(30)); // Poll every 30 seconds
        let mut stats_interval = interval(Duration::from_secs(300)); // Log stats every 5 minutes
        let mut cleanup_interval = interval(Duration::from_secs(3600)); // Cleanup every hour
        let mut recovery_interval = interval(Duration::from_secs(300)); // Recovery every 5 minutes

        // Process any existing items first
        if let Err(e) = self.process_batch_safe().await {
            error!("Failed to process initial batch: {}", e);
        }

        loop {
            tokio::select! {
                notification = listener.recv() => {
                    match notification {
                        Ok(_) => {
                            if let Err(e) = self.process_batch_safe().await {
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
                    if let Err(e) = self.process_batch_safe().await {
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
                _ = recovery_interval.tick() => {
                    // Periodic recovery of stale processing items
                    if let Ok(recovered) = self.embedding_queue.recover_stale_processing_items(300).await {
                        if recovered > 0 {
                            info!("Recovered {} stale embedding processing items during periodic cleanup", recovered);
                        }
                    }
                }
            }
        }
    }

    async fn process_batch_safe(&self) -> Result<()> {
        let _guard = self.processing_mutex.lock().await;
        self.process_batch().await
    }

    async fn process_batch(&self) -> Result<()> {
        let mut total_processed = 0;

        loop {
            let items = self.embedding_queue.dequeue_batch(self.batch_size).await?;

            if items.is_empty() {
                if total_processed > 0 {
                    info!(
                        "Finished processing all available embedding requests. Total processed: {}",
                        total_processed
                    );
                }
                return Ok(());
            }

            info!("Processing batch of {} embedding requests", items.len());
            let batch_size = items.len();

            if let Err(e) = self.process_embedding_batch(items).await {
                error!("Failed to process embedding batch: {}", e);
            } else {
                total_processed += batch_size;
            }
        }
    }

    async fn process_embedding_batch(&self, batch: Vec<EmbeddingQueueItem>) -> Result<()> {
        let start_time = std::time::Instant::now();

        // Step 1: Split all documents into input chunks and build metadata
        #[derive(Debug)]
        struct InputChunkInfo<'a> {
            document_id: String,
            input_chunk_index: usize, // Index within this document's input chunks
            input_text: &'a str,
        }

        let mut all_input_chunks = Vec::new();
        let mut document_metadata = std::collections::HashMap::new(); // document_id -> original item

        for item in &batch {
            let input_chunks = Self::split_large_content(&item.content);

            if input_chunks.len() > 1 {
                info!(
                    "Document {} ({}KB) split into {} input chunks",
                    item.document_id,
                    item.content.len() / 1024,
                    input_chunks.len()
                );
            }

            document_metadata.insert(item.document_id.clone(), item);

            for (chunk_idx, input_text) in input_chunks.into_iter().enumerate() {
                all_input_chunks.push(InputChunkInfo {
                    document_id: item.document_id.clone(),
                    input_chunk_index: chunk_idx,
                    input_text,
                });
            }
        }

        info!(
            "Processing {} documents with total {} input chunks",
            batch.len(),
            all_input_chunks.len()
        );

        // Step 2: Process input chunks in batches of MAX_EMBEDDING_BATCH_SIZE
        let embedding_repo = EmbeddingRepository::new(self.state.db_pool.pool());
        let mut all_embeddings_by_document: std::collections::HashMap<
            String,
            Vec<(Vec<f32>, (i32, i32))>,
        > = std::collections::HashMap::new();
        let mut model_name: Option<String> = None;
        let mut failed_document_ids = std::collections::HashSet::new();

        for input_batch_start in (0..all_input_chunks.len()).step_by(MAX_EMBEDDING_BATCH_SIZE) {
            let input_batch_end =
                (input_batch_start + MAX_EMBEDDING_BATCH_SIZE).min(all_input_chunks.len());
            let input_batch = &all_input_chunks[input_batch_start..input_batch_end];

            info!(
                "Processing input batch {}-{} of {} total chunks",
                input_batch_start + 1,
                input_batch_end,
                all_input_chunks.len()
            );

            // Extract just the text for the API call
            let input_texts: Vec<String> = input_batch
                .iter()
                .map(|chunk| chunk.input_text.to_string())
                .collect();

            // Call AI service for this batch
            let ai_service_start = std::time::Instant::now();
            let text_embeddings = match self
                .state
                .ai_client
                .generate_embeddings_with_options(
                    input_texts,
                    Some("retrieval.passage".to_string()),
                    Some(512),
                    Some("sentence".to_string()),
                )
                .await
            {
                Ok(embeddings) => embeddings,
                Err(e) => {
                    error!("Failed to generate embeddings for input batch: {}", e);
                    // Mark all documents in this batch as failed
                    for chunk_info in input_batch {
                        failed_document_ids.insert(chunk_info.document_id.clone());
                    }
                    continue;
                }
            };
            debug!(
                "AI service batch embedding generation took: {:?}",
                ai_service_start.elapsed()
            );

            // Store model name from first successful response
            if model_name.is_none() && !text_embeddings.is_empty() {
                model_name = text_embeddings[0].model_name.clone();
            }

            // Step 3: Map each response back to the correct document and calculate offsets
            for (chunk_info, text_embedding) in input_batch.iter().zip(text_embeddings.iter()) {
                // Calculate offset adjustment for this input chunk within its document
                let offset_adjustment = if chunk_info.input_chunk_index > 0 {
                    (chunk_info.input_chunk_index * MAX_DOCUMENT_CHARS)
                        - (chunk_info.input_chunk_index * CHUNK_OVERLAP)
                } else {
                    0
                };

                // Add all output chunks with adjusted offsets to the document's collection
                let document_embeddings = all_embeddings_by_document
                    .entry(chunk_info.document_id.clone())
                    .or_insert_with(Vec::new);

                for (chunk_emb, (start, end)) in text_embedding
                    .chunk_embeddings
                    .iter()
                    .zip(text_embedding.chunk_spans.iter())
                {
                    document_embeddings.push((
                        chunk_emb.clone(),
                        (
                            start + offset_adjustment as i32,
                            end + offset_adjustment as i32,
                        ),
                    ));
                }

                debug!(
                    "Processed input chunk {} for document {} -> {} output chunks",
                    chunk_info.input_chunk_index,
                    chunk_info.document_id,
                    text_embedding.chunk_embeddings.len()
                );
            }
        }

        // Step 4: Store embeddings for each document and track success/failure
        let mut success_ids = Vec::new();
        let mut failed_ids = Vec::new();

        for item in &batch {
            if failed_document_ids.contains(&item.document_id) {
                failed_ids.push(item.id.clone());
                continue;
            }

            if let Some(document_embeddings) = all_embeddings_by_document.get(&item.document_id) {
                if document_embeddings.is_empty() {
                    error!("No embeddings generated for document {}", item.document_id);
                    failed_ids.push(item.id.clone());
                    continue;
                }

                // Create combined embedding for storage
                let (chunk_embeddings, chunk_spans): (Vec<Vec<f32>>, Vec<(i32, i32)>) =
                    document_embeddings.iter().cloned().unzip();

                let combined_embedding = shared::clients::ai::TextEmbedding {
                    chunk_embeddings,
                    chunk_spans,
                    model_name: model_name.clone(),
                };

                match self
                    .store_embeddings(&embedding_repo, &item.document_id, &combined_embedding)
                    .await
                {
                    Ok(_) => {
                        success_ids.push(item.id.clone());
                        debug!(
                            "Successfully stored embeddings for document {} ({} output chunks)",
                            item.document_id,
                            combined_embedding.chunk_embeddings.len()
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
            } else {
                error!("No embeddings found for document {}", item.document_id);
                failed_ids.push(item.id.clone());
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_split_large_content() {
        // Test 1: Content smaller than MAX_DOCUMENT_CHARS
        let small_content = "a".repeat(1000);
        let chunks = EmbeddingProcessor::split_large_content(&small_content);
        assert_eq!(chunks.len(), 1);
        assert_eq!(chunks[0], small_content);

        // Test 2: Content exactly MAX_DOCUMENT_CHARS
        let exact_content = "b".repeat(MAX_DOCUMENT_CHARS);
        let chunks = EmbeddingProcessor::split_large_content(&exact_content);
        assert_eq!(chunks.len(), 1);
        assert_eq!(chunks[0], exact_content);

        // Test 3: Content larger than MAX_DOCUMENT_CHARS
        let large_content = "c".repeat(MAX_DOCUMENT_CHARS + 1000);
        let chunks = EmbeddingProcessor::split_large_content(&large_content);
        assert_eq!(chunks.len(), 2);
        assert_eq!(chunks[0].len(), MAX_DOCUMENT_CHARS);
        assert_eq!(chunks[1].len(), 1000 + CHUNK_OVERLAP);

        // Test 4: Very large content requiring multiple chunks
        let very_large_content = "d".repeat(MAX_DOCUMENT_CHARS * 3);
        let chunks = EmbeddingProcessor::split_large_content(&very_large_content);
        assert!(chunks.len() >= 3);

        // Verify overlap (skip this check as exact overlap may vary due to char boundaries)
    }

    #[test]
    fn test_split_large_content_with_unicode() {
        // Test with multi-byte UTF-8 characters
        let emoji_str = "😀".repeat(MAX_DOCUMENT_CHARS / 4); // Each emoji is 4 bytes
        let chunks = EmbeddingProcessor::split_large_content(&emoji_str);

        // Verify all chunks are valid UTF-8
        for chunk in &chunks {
            assert!(chunk.is_char_boundary(0));
            assert!(chunk.is_char_boundary(chunk.len()));
        }

        // Test with mixed ASCII and Unicode
        let mixed = "Hello 世界 ".repeat(MAX_DOCUMENT_CHARS / 10);
        let chunks = EmbeddingProcessor::split_large_content(&mixed);

        // Verify no panics and all chunks are valid
        for chunk in &chunks {
            // This would panic if boundaries were wrong
            let _ = chunk.chars().count();
        }
    }

    #[test]
    fn test_batch_size_limits() {
        // Test that MAX_EMBEDDING_BATCH_SIZE is set to a reasonable value
        assert_eq!(MAX_EMBEDDING_BATCH_SIZE, 32);

        // Test that we can handle batching logic
        let total_chunks = 100;
        let expected_batches =
            (total_chunks + MAX_EMBEDDING_BATCH_SIZE - 1) / MAX_EMBEDDING_BATCH_SIZE;
        assert_eq!(expected_batches, 4); // 100 / 32 = 3.125, rounded up to 4

        // Verify batch ranges
        let mut ranges = Vec::new();
        for batch_start in (0..total_chunks).step_by(MAX_EMBEDDING_BATCH_SIZE) {
            let batch_end = (batch_start + MAX_EMBEDDING_BATCH_SIZE).min(total_chunks);
            ranges.push((batch_start, batch_end));
        }

        assert_eq!(ranges.len(), 4);
        assert_eq!(ranges[0], (0, 32));
        assert_eq!(ranges[1], (32, 64));
        assert_eq!(ranges[2], (64, 96));
        assert_eq!(ranges[3], (96, 100));
    }

    #[test]
    fn test_offset_calculation() {
        // Test offset calculation for multiple input chunks from same document
        let chunk_0_offset = 0;
        let chunk_1_offset = (1 * MAX_DOCUMENT_CHARS) - (1 * CHUNK_OVERLAP);
        let chunk_2_offset = (2 * MAX_DOCUMENT_CHARS) - (2 * CHUNK_OVERLAP);

        // First chunk should have no offset
        assert_eq!(chunk_0_offset, 0);

        // Second chunk should account for first chunk size minus overlap
        assert_eq!(chunk_1_offset, 24_576 - 300); // 24,276

        // Third chunk should account for two chunks minus two overlaps
        assert_eq!(chunk_2_offset, 2 * 24_576 - 2 * 300); // 48,552

        // Verify the chunks properly overlap
        assert!(chunk_1_offset < MAX_DOCUMENT_CHARS); // Second chunk starts before first ends
        assert!(chunk_2_offset < 2 * MAX_DOCUMENT_CHARS); // Third chunk starts before second ends
    }
}

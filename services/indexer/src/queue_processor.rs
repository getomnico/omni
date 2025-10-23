use crate::AppState;
use anyhow::Result;
use futures::future::join_all;
use shared::db::repositories::{DocumentRepository, EmbeddingRepository};
use shared::embedding_queue::EmbeddingQueue;
use shared::models::{
    ConnectorEvent, ConnectorEventQueueItem, Document, DocumentMetadata, DocumentPermissions,
};
use shared::queue::EventQueue;
use sqlx::postgres::PgListener;
use std::sync::Arc;
use tokio::sync::{Mutex, Semaphore};
use tokio::time::{interval, Duration};
use tracing::{debug, error, info, warn};

// Batch processing types
#[derive(Debug)]
struct EventBatch {
    documents_created: Vec<(Document, Vec<String>)>, // (document, event_ids)
    documents_updated: Vec<(Document, Vec<String>)>, // (document, event_ids)
    documents_deleted: Vec<(String, String, Vec<String>)>, // (source_id, document_id, event_ids)
}

impl EventBatch {
    fn new() -> Self {
        Self {
            documents_created: Vec::new(),
            documents_updated: Vec::new(),
            documents_deleted: Vec::new(),
        }
    }

    fn is_empty(&self) -> bool {
        self.documents_created.is_empty()
            && self.documents_updated.is_empty()
            && self.documents_deleted.is_empty()
    }

    #[allow(dead_code)]
    fn total_events(&self) -> usize {
        self.documents_created
            .iter()
            .map(|(_, event_ids)| event_ids.len())
            .sum::<usize>()
            + self
                .documents_updated
                .iter()
                .map(|(_, event_ids)| event_ids.len())
                .sum::<usize>()
            + self
                .documents_deleted
                .iter()
                .map(|(_, _, event_ids)| event_ids.len())
                .sum::<usize>()
    }
}

#[derive(Debug)]
struct BatchProcessingResult {
    successful_event_ids: Vec<String>,
    failed_events: Vec<(String, String)>, // (event_id, error_message)
}

impl BatchProcessingResult {
    fn new() -> Self {
        Self {
            successful_event_ids: Vec::new(),
            failed_events: Vec::new(),
        }
    }
}

#[derive(Clone)]
pub struct QueueProcessor {
    pub state: AppState,
    pub event_queue: EventQueue,
    pub embedding_queue: EmbeddingQueue,
    pub batch_size: i32,
    pub parallelism: usize,
    semaphore: Arc<Semaphore>,
    processing_mutex: Arc<Mutex<()>>,
}

impl QueueProcessor {
    pub fn new(state: AppState) -> Self {
        let event_queue = EventQueue::new(state.db_pool.pool().clone());
        let embedding_queue = EmbeddingQueue::new(state.db_pool.pool().clone());
        let parallelism = (num_cpus::get() / 2).max(1); // Half the CPU cores, minimum 1
        let semaphore = Arc::new(Semaphore::new(parallelism));
        let processing_mutex = Arc::new(Mutex::new(()));
        Self {
            state,
            event_queue,
            embedding_queue,
            batch_size: 32,
            parallelism,
            semaphore,
            processing_mutex,
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

        // Recover any stale processing items from previous runs (5 minute timeout)
        match self.event_queue.recover_stale_processing_items(300).await {
            Ok(recovered) => {
                if recovered > 0 {
                    info!("Recovered {} stale processing items on startup", recovered);
                }
            }
            Err(e) => {
                error!("Failed to recover stale processing items on startup: {}", e);
            }
        }

        let mut listener = PgListener::connect_with(self.state.db_pool.pool()).await?;
        listener.listen("indexer_queue").await?;

        let mut poll_interval = interval(Duration::from_secs(60)); // Backup polling every minute
        let mut heartbeat_interval = interval(Duration::from_secs(300));
        let mut retry_interval = interval(Duration::from_secs(300)); // 5 minutes
        let mut cleanup_interval = interval(Duration::from_secs(3600)); // 1 hour
        let mut recovery_interval = interval(Duration::from_secs(300)); // 5 minutes

        // Process any existing events first
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
                    if let Err(e) = self.process_batch_safe().await {
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
                _ = recovery_interval.tick() => {
                    // Periodic recovery of stale processing items
                    if let Ok(recovered) = self.event_queue.recover_stale_processing_items(300).await {
                        if recovered > 0 {
                            info!("Recovered {} stale processing items during periodic cleanup", recovered);
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
            let events = self.event_queue.dequeue_batch(self.batch_size).await?;

            if events.is_empty() {
                if total_processed > 0 {
                    info!(
                        "Finished processing all available events. Total processed: {}",
                        total_processed
                    );
                }
                return Ok(());
            }

            let batch_start_time = std::time::Instant::now();
            info!(
                "Processing batch of {} events using batch operations",
                events.len()
            );

            // Store events for potential fallback processing
            let events_clone = events.clone();

            // Group events by type for batch processing
            let batch = self.group_events_by_type(events).await?;

            if batch.is_empty() {
                continue;
            }

            info!(
                "Batch contains: {} created, {} updated, {} deleted documents ({} created events, {} updated events, {} deleted events)",
                batch.documents_created.len(),
                batch.documents_updated.len(),
                batch.documents_deleted.len(),
                batch.documents_created.iter().map(|(_, event_ids)| event_ids.len()).sum::<usize>(),
                batch.documents_updated.iter().map(|(_, event_ids)| event_ids.len()).sum::<usize>(),
                batch.documents_deleted.iter().map(|(_, _, event_ids)| event_ids.len()).sum::<usize>()
            );

            // Process the batch with fallback to individual processing
            let result = self.process_event_batch(batch).await;

            match result {
                Ok(batch_result) => {
                    // Mark events as completed/failed in batch
                    if !batch_result.successful_event_ids.is_empty() {
                        if let Err(e) = self
                            .event_queue
                            .mark_events_completed_batch(batch_result.successful_event_ids.clone())
                            .await
                        {
                            error!(
                                "Failed to mark {} events as completed: {}",
                                batch_result.successful_event_ids.len(),
                                e
                            );
                        }
                    }

                    if !batch_result.failed_events.is_empty() {
                        if let Err(e) = self
                            .event_queue
                            .mark_events_dead_letter_batch(batch_result.failed_events.clone())
                            .await
                        {
                            error!(
                                "Failed to mark {} events as failed: {}",
                                batch_result.failed_events.len(),
                                e
                            );
                        }
                    }

                    let processed_count = batch_result.successful_event_ids.len();
                    total_processed += processed_count;

                    let batch_duration = batch_start_time.elapsed();
                    info!(
                        "Batch processing completed: {} successful, {} failed (took {:?}, {:.1} events/sec)",
                        batch_result.successful_event_ids.len(),
                        batch_result.failed_events.len(),
                        batch_duration,
                        batch_result.successful_event_ids.len() as f64 / batch_duration.as_secs_f64()
                    );
                }
                Err(e) => {
                    error!(
                        "Batch processing failed, falling back to individual processing: {}",
                        e
                    );

                    // Fall back to individual processing for this batch
                    let fallback_result = self.process_events_individually(events_clone).await;
                    match fallback_result {
                        Ok(processed_count) => {
                            total_processed += processed_count;
                            info!(
                                "Fallback processing completed successfully: {} events",
                                processed_count
                            );
                        }
                        Err(fallback_error) => {
                            error!("Fallback processing also failed: {}", fallback_error);
                        }
                    }
                }
            }
        }
    }

    async fn group_events_by_type(
        &self,
        events: Vec<ConnectorEventQueueItem>,
    ) -> Result<EventBatch> {
        let mut batch = EventBatch::new();

        // Temporary storage for grouping events by document key
        let mut created_docs: std::collections::HashMap<String, (Document, Vec<String>)> =
            std::collections::HashMap::new();
        let mut updated_docs: std::collections::HashMap<String, (Document, Vec<String>)> =
            std::collections::HashMap::new();
        let mut deleted_docs: std::collections::HashMap<String, (String, String, Vec<String>)> =
            std::collections::HashMap::new();

        for event_item in events {
            let event_id = event_item.id.clone();
            let sync_run_id = event_item.sync_run_id.clone();

            // Parse the event payload
            let event: ConnectorEvent = serde_json::from_value(event_item.payload.clone())?;

            // Update sync run progress for each event
            if let Err(e) = self.increment_sync_run_progress(&sync_run_id).await {
                warn!(
                    "Failed to update sync run progress for {}: {}",
                    sync_run_id, e
                );
            }

            match event {
                ConnectorEvent::DocumentCreated {
                    source_id,
                    document_id,
                    content_id,
                    metadata,
                    permissions,
                    ..
                } => {
                    let document = self.create_document_from_event(
                        source_id.clone(),
                        document_id.clone(),
                        content_id,
                        metadata,
                        permissions,
                    )?;

                    // Use source_id + external_id as deduplication key
                    let key = format!("{}:{}", source_id, document_id);

                    if let Some((_, event_ids)) = created_docs.get_mut(&key) {
                        // Already have this document, just add the event_id
                        event_ids.push(event_id);
                    } else {
                        // New document, create new entry
                        created_docs.insert(key, (document, vec![event_id]));
                    }
                }
                ConnectorEvent::DocumentUpdated {
                    source_id,
                    document_id,
                    content_id,
                    metadata,
                    permissions,
                    ..
                } => {
                    let document = self
                        .create_document_from_event_update(
                            source_id.clone(),
                            document_id.clone(),
                            content_id,
                            metadata,
                            permissions,
                        )
                        .await?;
                    if let Some(doc) = document {
                        // Use source_id + external_id as deduplication key
                        let key = format!("{}:{}", source_id, document_id);

                        if let Some((_, event_ids)) = updated_docs.get_mut(&key) {
                            // Already have this document, just add the event_id
                            event_ids.push(event_id);
                        } else {
                            // New document, create new entry
                            updated_docs.insert(key, (doc, vec![event_id]));
                        }
                    }
                }
                ConnectorEvent::DocumentDeleted {
                    source_id,
                    document_id,
                    ..
                } => {
                    // Use source_id + external_id as deduplication key
                    let key = format!("{}:{}", source_id, document_id);

                    if let Some((_, _, event_ids)) = deleted_docs.get_mut(&key) {
                        // Already have this deletion, just add the event_id
                        event_ids.push(event_id);
                    } else {
                        // New deletion, create new entry
                        deleted_docs.insert(key, (source_id, document_id, vec![event_id]));
                    }
                }
            }
        }

        // Convert the HashMap results to Vec format for EventBatch
        batch.documents_created = created_docs.into_values().collect();
        batch.documents_updated = updated_docs.into_values().collect();
        batch.documents_deleted = deleted_docs.into_values().collect();

        Ok(batch)
    }

    async fn process_event_batch(&self, batch: EventBatch) -> Result<BatchProcessingResult> {
        let mut result = BatchProcessingResult::new();

        // Process document creations in batch
        if !batch.documents_created.is_empty() {
            match self
                .process_documents_created_batch(&batch.documents_created)
                .await
            {
                Ok(successful_ids) => {
                    result.successful_event_ids.extend(successful_ids);
                }
                Err(e) => {
                    error!("Batch document creation failed: {}", e);
                    // Add all creation events to failed list
                    for (_, event_ids) in batch.documents_created {
                        for event_id in event_ids {
                            result.failed_events.push((event_id, e.to_string()));
                        }
                    }
                }
            }
        }

        // Process document updates in batch
        if !batch.documents_updated.is_empty() {
            match self
                .process_documents_updated_batch(&batch.documents_updated)
                .await
            {
                Ok(successful_ids) => {
                    result.successful_event_ids.extend(successful_ids);
                }
                Err(e) => {
                    error!("Batch document update failed: {}", e);
                    // Add all update events to failed list
                    for (_, event_ids) in batch.documents_updated {
                        for event_id in event_ids {
                            result.failed_events.push((event_id, e.to_string()));
                        }
                    }
                }
            }
        }

        // Process document deletions in batch
        if !batch.documents_deleted.is_empty() {
            match self
                .process_documents_deleted_batch(&batch.documents_deleted)
                .await
            {
                Ok(successful_ids) => {
                    result.successful_event_ids.extend(successful_ids);
                }
                Err(e) => {
                    error!("Batch document deletion failed: {}", e);
                    // Add all deletion events to failed list
                    for (_, _, event_ids) in batch.documents_deleted {
                        for event_id in event_ids {
                            result.failed_events.push((event_id, e.to_string()));
                        }
                    }
                }
            }
        }

        Ok(result)
    }

    // Helper methods for batch processing
    fn convert_metadata_to_json(&self, metadata: &DocumentMetadata) -> Result<serde_json::Value> {
        let mut metadata_json = serde_json::to_value(metadata)?;

        // Convert size from string to number if present
        if let Some(size_str) = &metadata.size {
            if let Ok(size_num) = size_str.parse::<i64>() {
                if let Some(obj) = metadata_json.as_object_mut() {
                    obj.insert(
                        "size".to_string(),
                        serde_json::Value::Number(size_num.into()),
                    );
                }
            }
        }

        Ok(metadata_json)
    }

    fn create_document_from_event(
        &self,
        source_id: String,
        document_id: String,
        content_id: String,
        metadata: DocumentMetadata,
        permissions: DocumentPermissions,
    ) -> Result<Document> {
        let now = sqlx::types::time::OffsetDateTime::now_utc();
        let metadata_json = self.convert_metadata_to_json(&metadata)?;
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

        // Ensure last_indexed_at is after created_at
        let last_indexed_at = now + std::time::Duration::from_millis(1);

        Ok(Document {
            id: ulid::Ulid::new().to_string(),
            source_id,
            external_id: document_id,
            title: metadata.title.unwrap_or_else(|| "Untitled".to_string()),
            content_id: Some(content_id),
            content_type: metadata.mime_type,
            file_size,
            file_extension,
            url: metadata.url,
            metadata: metadata_json,
            permissions: permissions_json,
            created_at: now,
            updated_at: now,
            last_indexed_at,
        })
    }

    async fn create_document_from_event_update(
        &self,
        source_id: String,
        document_id: String,
        content_id: String,
        metadata: DocumentMetadata,
        permissions: Option<DocumentPermissions>,
    ) -> Result<Option<Document>> {
        let repo = DocumentRepository::new(self.state.db_pool.pool());

        if let Some(mut document) = repo.find_by_external_id(&source_id, &document_id).await? {
            let now = sqlx::types::time::OffsetDateTime::now_utc();
            let metadata_json = self.convert_metadata_to_json(&metadata)?;

            document.title = metadata.title.unwrap_or(document.title);
            document.content_id = Some(content_id);
            document.metadata = metadata_json;
            if let Some(perms) = permissions {
                document.permissions = serde_json::to_value(&perms)?;
            }
            document.updated_at = now;

            Ok(Some(document))
        } else {
            warn!(
                "Document not found for update: {} from source {}",
                document_id, source_id
            );
            Ok(None)
        }
    }

    async fn process_documents_created_batch(
        &self,
        documents_with_event_ids: &[(Document, Vec<String>)],
    ) -> Result<Vec<String>> {
        let start_time = std::time::Instant::now();
        let documents: Vec<Document> = documents_with_event_ids
            .iter()
            .map(|(doc, _)| doc.clone())
            .collect();

        let repo = DocumentRepository::new(self.state.db_pool.pool());

        // Batch upsert documents
        let upsert_start = std::time::Instant::now();
        let upserted_documents = repo.batch_upsert(documents).await?;
        debug!(
            "Batch upsert of {} documents took {:?}",
            upserted_documents.len(),
            upsert_start.elapsed()
        );

        // Batch mark as indexed
        let document_ids: Vec<String> = upserted_documents.iter().map(|d| d.id.clone()).collect();
        repo.batch_mark_as_indexed(document_ids.clone()).await?;

        // Add documents to embedding queue
        let embedding_start = std::time::Instant::now();
        let mut embedding_tasks = Vec::new();
        for upserted_doc in upserted_documents.iter() {
            let embedding_queue = self.state.embedding_queue.clone();
            let doc_id = upserted_doc.id.clone();

            embedding_tasks.push(tokio::spawn(async move {
                if let Err(e) = embedding_queue.enqueue(doc_id.clone()).await {
                    error!("Failed to queue embeddings for document {}: {}", doc_id, e);
                }
            }));
        }

        // Wait for all embedding queue operations to complete
        futures::future::join_all(embedding_tasks).await;
        debug!(
            "Embedding queue operations took {:?}",
            embedding_start.elapsed()
        );

        let total_duration = start_time.elapsed();
        info!(
            "Batch processed {} documents successfully (took {:?}, {:.1} docs/sec)",
            upserted_documents.len(),
            total_duration,
            upserted_documents.len() as f64 / total_duration.as_secs_f64()
        );

        // Return all the event IDs that were successful
        Ok(documents_with_event_ids
            .iter()
            .flat_map(|(_, event_ids)| event_ids.clone())
            .collect())
    }

    async fn process_documents_updated_batch(
        &self,
        documents_with_event_ids: &[(Document, Vec<String>)],
    ) -> Result<Vec<String>> {
        let repo = DocumentRepository::new(self.state.db_pool.pool());

        // For updates, we need to handle them individually since we need to find existing documents
        let mut successful_event_ids = Vec::new();
        let mut updated_documents = Vec::new();

        for (document, event_ids) in documents_with_event_ids {
            match repo.update(&document.id, document.clone()).await {
                Ok(Some(updated_doc)) => {
                    updated_documents.push((event_ids.clone(), updated_doc));
                    successful_event_ids.extend(event_ids.clone());
                }
                Ok(None) => {
                    warn!("Document not found for update: {}", document.external_id);
                }
                Err(e) => {
                    error!("Failed to update document {}: {}", document.external_id, e);
                    return Err(e.into());
                }
            }
        }

        if !updated_documents.is_empty() {
            // Queue embeddings and mark as indexed
            let mut embedding_tasks = Vec::new();
            for (_event_ids, updated_doc) in updated_documents {
                let embedding_queue = self.state.embedding_queue.clone();
                let doc_id = updated_doc.id.clone();

                embedding_tasks.push(tokio::spawn(async move {
                    if let Err(e) = embedding_queue.enqueue(doc_id.clone()).await {
                        error!(
                            "Failed to queue embeddings for updated document {}: {}",
                            doc_id, e
                        );
                    }
                }));

                // Mark as indexed
                if let Err(e) = repo.mark_as_indexed(&updated_doc.id).await {
                    error!(
                        "Failed to mark document {} as indexed: {}",
                        updated_doc.id, e
                    );
                }
            }

            // Wait for all embedding operations to complete
            futures::future::join_all(embedding_tasks).await;
        }

        info!(
            "Batch updated {} documents successfully",
            successful_event_ids.len()
        );
        Ok(successful_event_ids)
    }

    async fn process_documents_deleted_batch(
        &self,
        deletions: &[(String, String, Vec<String>)], // (source_id, document_id, event_ids)
    ) -> Result<Vec<String>> {
        let start_time = std::time::Instant::now();
        let repo = DocumentRepository::new(self.state.db_pool.pool());
        let embedding_repo = EmbeddingRepository::new(self.state.db_pool.pool());

        let mut successful_event_ids = Vec::new();
        let mut document_ids_to_delete = Vec::new();

        // First, find all documents that exist
        for (source_id, document_id, event_ids) in deletions {
            if let Some(document) = repo.find_by_external_id(source_id, document_id).await? {
                document_ids_to_delete.push(document.id.clone());
                successful_event_ids.extend(event_ids.clone());
            } else {
                warn!(
                    "Document not found for deletion: {} from source {}",
                    document_id, source_id
                );
                // Still count as successful since the document doesn't exist
                successful_event_ids.extend(event_ids.clone());
            }
        }

        if !document_ids_to_delete.is_empty() {
            // Delete embeddings in batch
            for doc_id in &document_ids_to_delete {
                if let Err(e) = embedding_repo.delete_by_document_id(doc_id).await {
                    error!("Failed to delete embeddings for document {}: {}", doc_id, e);
                }
            }

            // Delete documents in batch
            let delete_start = std::time::Instant::now();
            let deleted_count = repo.batch_delete(document_ids_to_delete.clone()).await?;
            debug!("Batch document deletion took {:?}", delete_start.elapsed());

            let total_duration = start_time.elapsed();
            info!(
                "Batch deleted {} documents and their embeddings (took {:?})",
                deleted_count, total_duration
            );
        }

        Ok(successful_event_ids)
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
    // Fallback method for individual processing when batch operations fail
    async fn process_events_individually(
        &self,
        events: Vec<ConnectorEventQueueItem>,
    ) -> Result<usize> {
        info!(
            "Processing {} events individually as fallback",
            events.len()
        );

        // Process events concurrently using the original individual approach
        let mut tasks = Vec::new();

        for event_item in events {
            let event_id = event_item.id.clone();
            let payload = event_item.payload.clone();
            let state = self.state.clone();
            let event_queue = self.event_queue.clone();
            let semaphore = self.semaphore.clone();

            let task = tokio::spawn(async move {
                // Acquire semaphore permit to limit concurrency
                let _permit = semaphore.acquire().await.unwrap();

                info!("Processing event {} individually", event_id);

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

        info!(
            "Individual processing completed: {} successful out of {} events",
            processed_count,
            results.len()
        );
        Ok(processed_count)
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
        let start_time = std::time::Instant::now();
        let event: ConnectorEvent = serde_json::from_value(payload.clone())?;
        let sync_run_id = event.sync_run_id().to_string();
        debug!("Started processing event, sync_run_id: {}", sync_run_id);

        // Update sync run progress
        let sync_update_start = std::time::Instant::now();
        if let Err(e) = self.increment_sync_run_progress(&sync_run_id).await {
            warn!(
                "Failed to update sync run progress for {}: {}",
                sync_run_id, e
            );
        }
        debug!(
            "Sync run progress update took: {:?}",
            sync_update_start.elapsed()
        );

        match event {
            ConnectorEvent::DocumentCreated {
                sync_run_id: _,
                source_id,
                document_id,
                content_id,
                metadata,
                permissions,
            } => {
                self.handle_document_created(
                    source_id,
                    document_id,
                    content_id,
                    metadata,
                    permissions,
                )
                .await?;
            }
            ConnectorEvent::DocumentUpdated {
                sync_run_id: _,
                source_id,
                document_id,
                content_id,
                metadata,
                permissions,
            } => {
                self.handle_document_updated(
                    source_id,
                    document_id,
                    content_id,
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

        debug!("Total event processing time: {:?}", start_time.elapsed());
        Ok(())
    }

    async fn handle_document_created(
        &self,
        source_id: String,
        document_id: String,
        content_id: String,
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
            content_id: Some(content_id.clone()),
            content_type: metadata.mime_type.clone(),
            file_size,
            file_extension,
            url: metadata.url.clone(),
            metadata: metadata_json,
            permissions: permissions_json,
            created_at: now,
            updated_at: now,
            last_indexed_at: now,
        };

        let repo = DocumentRepository::new(self.state.db_pool.pool());
        let upsert_start = std::time::Instant::now();
        let upserted = repo.upsert(document).await?;
        debug!("Document upsert took: {:?}", upsert_start.elapsed());

        // Fetch content from LOB storage for tsvector generation and embedding queueing
        let content = match self.state.content_storage.get_text(&content_id).await {
            Ok(content) => content,
            Err(e) => {
                error!(
                    "Failed to fetch content from LOB storage for document {}: {}",
                    document_id, e
                );
                return Err(e.into());
            }
        };

        let search_vector_start = std::time::Instant::now();
        repo.update_search_vector(&upserted.id).await?;
        debug!(
            "Search vector update took: {:?}",
            search_vector_start.elapsed()
        );

        // Queue embeddings for async generation instead of generating them synchronously
        if content.trim().is_empty() {
            info!(
                "Skipping embedding queue for document {} - no content",
                document_id
            );
        } else {
            let queue_start = std::time::Instant::now();
            if let Err(e) = self
                .state
                .embedding_queue
                .enqueue(upserted.id.clone())
                .await
            {
                error!(
                    "Failed to queue embeddings for document {}: {}",
                    document_id, e
                );
            } else {
                debug!(
                    "Embeddings queued for document {} (took: {:?})",
                    document_id,
                    queue_start.elapsed()
                );
            }
        }

        let mark_indexed_start = std::time::Instant::now();
        repo.mark_as_indexed(&upserted.id).await?;
        debug!("Mark as indexed took: {:?}", mark_indexed_start.elapsed());

        info!("Document upserted successfully: {}", document_id);
        Ok(())
    }

    async fn handle_document_updated(
        &self,
        source_id: String,
        document_id: String,
        content_id: String,
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
            document.content_id = Some(content_id.clone());
            document.metadata = metadata_json;
            if let Some(perms) = permissions {
                document.permissions = serde_json::to_value(&perms)?;
            }
            document.updated_at = now;

            let updated_document = repo.update(&doc_id, document).await?;

            // Fetch content from LOB storage for tsvector generation and embedding queueing
            let content = match self.state.content_storage.get_text(&content_id).await {
                Ok(content) => content,
                Err(e) => {
                    error!(
                        "Failed to fetch content from LOB storage for document {}: {}",
                        document_id, e
                    );
                    return Err(e.into());
                }
            };

            repo.update_search_vector(&doc_id).await?;

            // Queue embeddings for async generation
            if let Some(_updated_doc) = &updated_document {
                if !content.trim().is_empty() {
                    if let Err(e) = self.state.embedding_queue.enqueue(doc_id.clone()).await {
                        error!(
                            "Failed to queue embeddings for updated document {}: {}",
                            document_id, e
                        );
                    }
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

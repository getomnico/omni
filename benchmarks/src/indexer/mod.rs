use crate::config::BenchmarkConfig;
use crate::datasets::{Dataset, Document};
use crate::evaluator::metrics::SystemInfo;
use anyhow::Result;
use futures::stream::StreamExt;
use futures::Stream;
use serde::{Deserialize, Serialize};
use shared::models::{ConnectorEvent, DocumentMetadata, DocumentPermissions};
use shared::queue::EventQueue;
use shared::utils::generate_ulid;
use shared::ContentStorage;
use sqlx::{Pool, Postgres, Row};
use std::collections::HashMap;
use std::pin::Pin;
use std::time::{Duration, Instant};
use tracing::{info, warn};

pub struct BenchmarkIndexer {
    config: BenchmarkConfig,
    db_pool: Pool<Postgres>,
    event_queue: EventQueue,
    content_storage: ContentStorage,
}

impl BenchmarkIndexer {
    pub async fn new(config: BenchmarkConfig) -> Result<Self> {
        // Connect to benchmark database
        let db_pool = sqlx::postgres::PgPoolOptions::new()
            .max_connections(10)
            .connect(&config.database_url)
            .await?;

        let event_queue = EventQueue::new(db_pool.clone());
        let content_storage = ContentStorage::new(db_pool.clone());

        Ok(Self {
            config,
            db_pool,
            event_queue,
            content_storage,
        })
    }

    pub async fn setup_benchmark_database(&self) -> Result<()> {
        info!("Setting up benchmark database");

        if self.config.reset_db_on_start {
            info!("Resetting benchmark database");
            self.clear_all_data().await?;
        }

        // Ensure database schema exists
        self.ensure_schema().await?;

        // Ensure benchmark user exists
        self.ensure_benchmark_user().await?;

        info!("Benchmark database setup completed");
        Ok(())
    }

    async fn clear_all_data(&self) -> Result<()> {
        self.cleanup_benchmark_data().await
    }

    /// Clean up all benchmark data from the database
    /// This truncates all tables but preserves the schema
    pub async fn cleanup_benchmark_data(&self) -> Result<()> {
        info!("Cleaning up benchmark data...");

        let tables = vec![
            "embeddings",
            "documents",
            "content_store",
            "sources",
            "users",
            "sync_runs",
            "connector_events_queue",
            "embedding_queue",
            "service_credentials",
        ];

        for table in tables {
            let query = format!("TRUNCATE TABLE {} CASCADE", table);
            if let Err(e) = sqlx::query(&query).execute(&self.db_pool).await {
                warn!("Failed to truncate table {}: {}", table, e);
            }
        }

        info!("Cleared all benchmark data");
        Ok(())
    }

    /// Clean up data for a specific source only
    pub async fn cleanup_source_data(&self, source_id: &str) -> Result<()> {
        info!("Cleaning up data for source: {}", source_id);

        // Delete embeddings for this source's documents
        sqlx::query(
            "DELETE FROM embeddings WHERE document_id IN (SELECT id FROM documents WHERE source_id = $1)",
        )
        .bind(source_id)
        .execute(&self.db_pool)
        .await?;

        // Delete documents
        sqlx::query("DELETE FROM documents WHERE source_id = $1")
            .bind(source_id)
            .execute(&self.db_pool)
            .await?;

        // Delete content store entries for this source
        sqlx::query(
            "DELETE FROM content_store WHERE id IN (
                SELECT content_id FROM connector_events_queue WHERE source_id = $1
            )",
        )
        .bind(source_id)
        .execute(&self.db_pool)
        .await?;

        // Delete queue entries
        sqlx::query("DELETE FROM connector_events_queue WHERE source_id = $1")
            .bind(source_id)
            .execute(&self.db_pool)
            .await?;

        // Delete sync runs
        sqlx::query("DELETE FROM sync_runs WHERE source_id = $1")
            .bind(source_id)
            .execute(&self.db_pool)
            .await?;

        // Delete the source itself
        sqlx::query("DELETE FROM sources WHERE id = $1")
            .bind(source_id)
            .execute(&self.db_pool)
            .await?;

        info!("Cleaned up data for source: {}", source_id);
        Ok(())
    }

    async fn ensure_schema(&self) -> Result<()> {
        // Check if required tables exist, if not run migrations
        let table_exists = sqlx::query(
            "SELECT EXISTS (SELECT FROM information_schema.tables WHERE table_name = 'documents')",
        )
        .fetch_one(&self.db_pool)
        .await?
        .get::<bool, _>(0);

        if !table_exists {
            return Err(anyhow::anyhow!(
                "Database schema not found. Please run migrations first:\n\
                 cd services/migrations && cargo run --bin migrator"
            ));
        }

        info!("Database schema verified");
        Ok(())
    }

    async fn ensure_benchmark_user(&self) -> Result<String> {
        let benchmark_user_id = "01BENCHMARK000000000000001";

        // Check if benchmark user already exists
        let user_exists: bool =
            sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM users WHERE id = $1)")
                .bind(benchmark_user_id)
                .fetch_one(&self.db_pool)
                .await?;

        if !user_exists {
            // Create benchmark user
            sqlx::query(
                r#"
                INSERT INTO users (id, email, password_hash, full_name, role, is_active, created_at, updated_at)
                VALUES ($1, $2, $3, $4, $5, true, NOW(), NOW())
                "#,
            )
            .bind(benchmark_user_id)
            .bind("benchmark@clio.local")
            .bind("$2y$12$dummy_hash") // Dummy password hash since this user won't login
            .bind("Benchmark User")
            .bind("admin")
            .execute(&self.db_pool)
            .await?;

            info!("Created benchmark user: {}", benchmark_user_id);
        }

        Ok(benchmark_user_id.to_string())
    }

    pub async fn index_dataset(&self, dataset: &Dataset) -> Result<String> {
        info!("Starting to index dataset: {}", dataset.name);
        info!("Total documents to index: {}", dataset.documents.len());

        // Create a benchmark source
        let source_id = self.create_benchmark_source(&dataset.name).await?;

        // Create a sync run for this benchmark
        let sync_run_id = self.create_sync_run(&source_id).await?;

        // Convert dataset documents to connector events and enqueue them
        let batch_size = 100;
        let total_batches = (dataset.documents.len() + batch_size - 1) / batch_size;

        for (batch_idx, chunk) in dataset.documents.chunks(batch_size).enumerate() {
            info!(
                "Queuing batch {}/{} ({} documents)",
                batch_idx + 1,
                total_batches,
                chunk.len()
            );

            if let Err(e) = self
                .queue_document_batch(chunk, &source_id, &sync_run_id)
                .await
            {
                warn!("Failed to queue batch {}: {}", batch_idx + 1, e);
                continue;
            }

            // Small delay between batches to avoid overwhelming the queue
            // tokio::time::sleep(Duration::from_millis(50)).await;
        }

        // Notify indexer to start processing the queued events
        sqlx::query("NOTIFY indexer_queue")
            .execute(&self.db_pool)
            .await?;

        info!("Dataset indexing completed: {}", dataset.name);
        Ok(source_id)
    }

    pub async fn index_document_stream(
        &self,
        dataset_name: &str,
        document_stream: Pin<Box<dyn Stream<Item = Result<Document>> + Send>>,
    ) -> Result<String> {
        info!("Starting to index document stream: {}", dataset_name);

        // Create a benchmark source
        let source_id = self.create_benchmark_source(dataset_name).await?;

        // Create a sync run for this benchmark
        let sync_run_id = self.create_sync_run(&source_id).await?;

        // Process documents in batches from the stream
        let batch_size = 100;
        let mut document_stream = document_stream;
        let mut batch = Vec::new();
        let mut total_processed = 0;

        while let Some(doc_result) = document_stream.next().await {
            match doc_result {
                Ok(document) => {
                    batch.push(document);

                    // Process batch when it reaches the batch size
                    if batch.len() >= batch_size {
                        if let Err(e) = self
                            .queue_document_batch(&batch, &source_id, &sync_run_id)
                            .await
                        {
                            warn!("Failed to queue batch: {}", e);
                        } else {
                            total_processed += batch.len();
                            info!(
                                "Queued batch of {} documents (total: {})",
                                batch.len(),
                                total_processed
                            );
                        }
                        batch.clear();

                        // Small delay between batches to avoid overwhelming the queue
                        tokio::time::sleep(Duration::from_millis(50)).await;
                    }
                }
                Err(e) => {
                    warn!("Error reading document from stream: {}", e);
                    continue;
                }
            }
        }

        // Process remaining documents in the final batch
        if !batch.is_empty() {
            if let Err(e) = self
                .queue_document_batch(&batch, &source_id, &sync_run_id)
                .await
            {
                warn!("Failed to queue final batch: {}", e);
            } else {
                total_processed += batch.len();
                info!(
                    "Queued final batch of {} documents (total: {})",
                    batch.len(),
                    total_processed
                );
            }
        }

        // Notify indexer to start processing the queued events
        sqlx::query("NOTIFY indexer_queue")
            .execute(&self.db_pool)
            .await?;

        info!(
            "Document stream indexing completed: {} (total documents: {})",
            dataset_name, total_processed
        );
        Ok(source_id)
    }

    async fn create_benchmark_source(&self, dataset_name: &str) -> Result<String> {
        let source_id = generate_ulid();
        let source_name = format!(
            "benchmark_{}",
            dataset_name.to_lowercase().replace("-", "_")
        );

        // Get the benchmark user ID
        let benchmark_user_id = self.ensure_benchmark_user().await?;

        sqlx::query(
            r#"
            INSERT INTO sources (id, name, source_type, config, is_active, created_at, updated_at, created_by)
            VALUES ($1, $2, $3, '{}', true, NOW(), NOW(), $4)
            "#,
        )
        .bind(&source_id)
        .bind(&source_name)
        .bind("local_files") // We need to adhere to the source_type constraint in the db 
        .bind(&benchmark_user_id)
        .execute(&self.db_pool)
        .await?;

        info!("Created benchmark source: {} ({})", source_name, source_id);
        Ok(source_id)
    }

    async fn create_sync_run(&self, source_id: &str) -> Result<String> {
        let sync_run_id = generate_ulid();
        let now = sqlx::types::time::OffsetDateTime::now_utc();

        sqlx::query(
            r#"
            INSERT INTO sync_runs (id, source_id, sync_type, started_at, status, documents_processed, documents_updated, created_at, updated_at)
            VALUES ($1, $2, 'full', $3, 'running', 0, 0, $4, $5)
            "#,
        )
        .bind(&sync_run_id)
        .bind(source_id)
        .bind(&now)
        .bind(&now)
        .bind(&now)
        .execute(&self.db_pool)
        .await?;

        info!(
            "Created sync run: {} for source: {}",
            sync_run_id, source_id
        );
        Ok(sync_run_id)
    }

    async fn convert_document_to_event(
        &self,
        doc: &Document,
        source_id: &str,
        sync_run_id: &str,
    ) -> Result<ConnectorEvent> {
        let mut extra_metadata = HashMap::new();
        for (key, value) in &doc.metadata {
            extra_metadata.insert(key.clone(), serde_json::Value::String(value.clone()));
        }
        extra_metadata.insert("benchmark".to_string(), serde_json::Value::Bool(true));

        let metadata = DocumentMetadata {
            title: Some(doc.title.clone()),
            author: None,
            created_at: None,
            updated_at: None,
            mime_type: Some("text/plain".to_string()),
            size: Some(doc.content.len().to_string()),
            url: None,
            path: None,
            extra: Some(extra_metadata),
        };

        let permissions = DocumentPermissions {
            public: true,
            users: vec![],
            groups: vec![],
        };

        // Store content in content storage and get ID
        let content_id = self.content_storage.store_text(doc.content.clone()).await?;

        Ok(ConnectorEvent::DocumentCreated {
            sync_run_id: sync_run_id.to_string(),
            source_id: source_id.to_string(),
            document_id: doc.id.clone(),
            content_id,
            metadata,
            permissions,
            attributes: None,
        })
    }

    async fn queue_document_batch(
        &self,
        documents: &[Document],
        source_id: &str,
        sync_run_id: &str,
    ) -> Result<()> {
        // Convert documents to connector events and enqueue them
        for doc in documents {
            let event = match self
                .convert_document_to_event(doc, source_id, sync_run_id)
                .await
            {
                Ok(event) => event,
                Err(e) => {
                    warn!("Failed to convert document {} to event: {}", doc.id, e);
                    continue;
                }
            };

            if let Err(e) = self.event_queue.enqueue(source_id, &event).await {
                warn!("Failed to enqueue document {}: {}", doc.id, e);
                continue;
            }
        }

        info!("Queued {} documents for processing", documents.len());
        Ok(())
    }

    pub async fn wait_for_indexing_completion(&self, source_id: &str) -> Result<()> {
        self.wait_for_indexing_completion_with_timeout(source_id, Duration::from_secs(30 * 60))
            .await
    }

    pub async fn wait_for_indexing_completion_with_timeout(
        &self,
        source_id: &str,
        timeout: Duration,
    ) -> Result<()> {
        info!(
            "Waiting for indexing to complete (timeout: {}s)...",
            timeout.as_secs()
        );

        let start_time = Instant::now();

        // Poll the queue and document counts until processing is complete
        let mut last_processed_count = 0i64;
        let mut stable_count = 0;

        loop {
            // Check timeout
            if start_time.elapsed() > timeout {
                return Err(anyhow::anyhow!(
                    "Indexing timed out after {}s. Documents indexed so far: {}",
                    timeout.as_secs(),
                    last_processed_count
                ));
            }

            // Check queue stats for this source specifically
            let queue_stats: (i64, i64, i64, i64) = sqlx::query_as(
                r#"
                SELECT
                    COUNT(*) FILTER (WHERE status = 'pending') as pending,
                    COUNT(*) FILTER (WHERE status = 'processing') as processing,
                    COUNT(*) FILTER (WHERE status = 'completed') as completed,
                    COUNT(*) FILTER (WHERE status = 'failed' OR status = 'dead_letter') as failed
                FROM connector_events_queue
                WHERE source_id = $1
                "#,
            )
            .bind(source_id)
            .fetch_one(&self.db_pool)
            .await?;

            let (pending, processing, completed, failed) = queue_stats;

            // Check document count for this source
            let doc_count: i64 =
                sqlx::query_scalar("SELECT COUNT(*) FROM documents WHERE source_id = $1")
                    .bind(source_id)
                    .fetch_one(&self.db_pool)
                    .await?;

            // Check embedding queue for documents from this source
            let embedding_stats: (i64, i64) = sqlx::query_as(
                r#"
                SELECT
                    COUNT(*) FILTER (WHERE eq.status = 'pending') as pending,
                    COUNT(*) FILTER (WHERE eq.status = 'processing') as processing
                FROM embedding_queue eq
                JOIN documents d ON eq.document_id = d.id
                WHERE d.source_id = $1
                "#,
            )
            .bind(source_id)
            .fetch_one(&self.db_pool)
            .await
            .unwrap_or((0, 0));

            let (emb_pending, emb_processing) = embedding_stats;

            info!(
                "Indexing progress - Queue: pending={}, processing={}, completed={}, failed={} | Embeddings: pending={}, processing={} | Docs: {}",
                pending, processing, completed, failed, emb_pending, emb_processing, doc_count
            );

            // Check if both connector queue and embedding queue are done
            if pending == 0 && processing == 0 && emb_pending == 0 && emb_processing == 0 {
                if doc_count == last_processed_count {
                    stable_count += 1;
                    if stable_count >= 3 {
                        break;
                    }
                } else {
                    stable_count = 0;
                    last_processed_count = doc_count;
                }
            } else {
                stable_count = 0;
                last_processed_count = doc_count;
            }

            tokio::time::sleep(Duration::from_secs(2)).await;
        }

        info!(
            "Indexing completed in {:.1}s. Total documents: {}",
            start_time.elapsed().as_secs_f64(),
            last_processed_count
        );
        Ok(())
    }

    pub async fn get_index_stats(&self, source_id: &str) -> Result<IndexStats> {
        let doc_count: i64 =
            sqlx::query_scalar("SELECT COUNT(*) FROM documents WHERE source_id = $1")
                .bind(source_id)
                .fetch_one(&self.db_pool)
                .await?;

        let embedding_count: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM embeddings e JOIN documents d ON e.document_id = d.id WHERE d.source_id = $1"
        )
        .bind(source_id)
        .fetch_one(&self.db_pool)
        .await
        .unwrap_or(0);

        Ok(IndexStats {
            total_documents: doc_count,
            total_embeddings: embedding_count,
        })
    }

    /// Index document stream with throughput statistics
    pub async fn index_document_stream_with_stats(
        &self,
        dataset_name: &str,
        document_stream: Pin<Box<dyn Stream<Item = Result<Document>> + Send>>,
    ) -> Result<(String, IndexingThroughputStats)> {
        info!(
            "Starting to index document stream with stats: {}",
            dataset_name
        );

        let start_time = Instant::now();

        // Create a benchmark source
        let source_id = self.create_benchmark_source(dataset_name).await?;

        // Create a sync run for this benchmark
        let sync_run_id = self.create_sync_run(&source_id).await?;

        // Process documents in batches from the stream
        let batch_size = 100;
        let mut document_stream = document_stream;
        let mut batch = Vec::new();
        let mut total_processed = 0;
        let mut total_bytes = 0usize;

        let queue_start = Instant::now();

        while let Some(doc_result) = document_stream.next().await {
            match doc_result {
                Ok(document) => {
                    total_bytes += document.content.len();
                    batch.push(document);

                    // Process batch when it reaches the batch size
                    if batch.len() >= batch_size {
                        if let Err(e) = self
                            .queue_document_batch(&batch, &source_id, &sync_run_id)
                            .await
                        {
                            warn!("Failed to queue batch: {}", e);
                        } else {
                            total_processed += batch.len();
                            if total_processed % 1000 == 0 {
                                let elapsed = queue_start.elapsed().as_secs_f64();
                                let rate = total_processed as f64 / elapsed;
                                info!(
                                    "Queued {} documents ({:.1} docs/sec)",
                                    total_processed, rate
                                );
                            }
                        }
                        batch.clear();

                        // Small delay between batches to avoid overwhelming the queue
                        tokio::time::sleep(Duration::from_millis(50)).await;
                    }
                }
                Err(e) => {
                    warn!("Error reading document from stream: {}", e);
                    continue;
                }
            }
        }

        // Process remaining documents in the final batch
        if !batch.is_empty() {
            if let Err(e) = self
                .queue_document_batch(&batch, &source_id, &sync_run_id)
                .await
            {
                warn!("Failed to queue final batch: {}", e);
            } else {
                total_processed += batch.len();
            }
        }

        let queue_time_secs = queue_start.elapsed().as_secs_f64();

        // Notify indexer to start processing the queued events
        sqlx::query("NOTIFY indexer_queue")
            .execute(&self.db_pool)
            .await?;

        info!(
            "Document stream queuing completed: {} docs in {:.2}s ({:.1} docs/sec)",
            total_processed,
            queue_time_secs,
            total_processed as f64 / queue_time_secs
        );

        // Wait for indexing to complete
        let indexing_start = Instant::now();
        self.wait_for_indexing_completion(&source_id).await?;
        let indexing_time_secs = indexing_start.elapsed().as_secs_f64();

        let total_time_secs = start_time.elapsed().as_secs_f64();

        let stats = IndexingThroughputStats {
            total_documents: total_processed,
            total_bytes,
            queue_time_secs,
            indexing_time_secs,
            total_time_secs,
            documents_per_second: total_processed as f64 / total_time_secs,
            bytes_per_second: total_bytes as f64 / total_time_secs,
        };

        info!(
            "Indexing completed: {} docs, {:.2} MB in {:.2}s ({:.1} docs/sec)",
            stats.total_documents,
            stats.total_bytes as f64 / 1024.0 / 1024.0,
            stats.total_time_secs,
            stats.documents_per_second
        );

        Ok((source_id, stats))
    }

    pub async fn get_system_info(&self) -> Result<SystemInfo> {
        let total_documents: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM documents")
            .fetch_one(&self.db_pool)
            .await
            .unwrap_or(0);

        let total_embeddings: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM embeddings")
            .fetch_one(&self.db_pool)
            .await
            .unwrap_or(0);

        let index_size_bytes: Option<i64> = sqlx::query_scalar(
            "SELECT pg_total_relation_size('documents') + COALESCE(pg_total_relation_size('embeddings'), 0)",
        )
        .fetch_one(&self.db_pool)
        .await
        .ok();

        let postgres_version: Option<String> = sqlx::query_scalar("SELECT version()")
            .fetch_one(&self.db_pool)
            .await
            .ok();

        Ok(SystemInfo {
            total_documents,
            total_embeddings,
            index_size_bytes,
            postgres_version,
        })
    }
}

#[derive(Debug, Clone)]
pub struct IndexStats {
    pub total_documents: i64,
    pub total_embeddings: i64,
}

/// Statistics about indexing throughput
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IndexingThroughputStats {
    pub total_documents: usize,
    pub total_bytes: usize,
    pub queue_time_secs: f64,
    pub indexing_time_secs: f64,
    pub total_time_secs: f64,
    pub documents_per_second: f64,
    pub bytes_per_second: f64,
}

impl IndexingThroughputStats {
    pub fn print_summary(&self) {
        println!("\n=== Indexing Throughput Stats ===");
        println!("Documents: {}", self.total_documents);
        println!(
            "Total Size: {:.2} MB",
            self.total_bytes as f64 / 1024.0 / 1024.0
        );
        println!();
        println!("Timing:");
        println!("  Queue Time: {:.2}s", self.queue_time_secs);
        println!("  Indexing Time: {:.2}s", self.indexing_time_secs);
        println!("  Total Time: {:.2}s", self.total_time_secs);
        println!();
        println!("Throughput:");
        println!("  {:.1} docs/sec", self.documents_per_second);
        println!("  {:.2} MB/sec", self.bytes_per_second / 1024.0 / 1024.0);
        println!("=================================\n");
    }
}

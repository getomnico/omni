use anyhow::Result;
use omni_indexer::embedding_processor::EmbeddingProcessor;
use omni_indexer::queue_processor::QueueProcessor;
use shared::db::repositories::{DocumentRepository, EmbeddingRepository};
use shared::models::{ConnectorEvent, DocumentMetadata, DocumentPermissions};
use shared::queue::EventQueue;
use std::collections::HashMap;
use std::fs;
use tokio::time::{sleep, timeout, Duration};
use tracing::{info, warn};
use ulid::Ulid;

mod common;
use common::setup_test_fixture;

/// Integration test for the embedding processor that validates the full flow:
/// 1. Reads text documents from test resources
/// 2. Populates connector events queue
/// 3. Runs queue processor to create documents
/// 4. Runs embedding processor to generate embeddings
/// 5. Validates embeddings table contains expected data
#[tokio::test]
async fn test_embedding_processor_full_flow() -> Result<()> {
    let _ = tracing_subscriber::fmt::try_init();

    info!("Starting embedding processor integration test");

    // Setup test environment
    let fixture = setup_test_fixture().await?;
    let state = fixture.state.clone();

    // Step 1: Read all text documents from test resources
    let text_files = read_test_text_files().await?;
    info!("Loaded {} text files for testing", text_files.len());

    // Step 2: Use existing test source and create sync run
    let source_id = "01JGF7V3E0Y2R1X8P5Q7W9T4N7".to_string(); // Pre-seeded test source
    let sync_run_id = Ulid::new().to_string();

    info!("Using existing test source: {}", source_id);

    // Create a test sync run
    sqlx::query(
        r#"
        INSERT INTO sync_runs (id, source_id, sync_type, status, started_at, created_at, updated_at)
        VALUES ($1, $2, $3, $4, CURRENT_TIMESTAMP, CURRENT_TIMESTAMP, CURRENT_TIMESTAMP)
        "#,
    )
    .bind(&sync_run_id)
    .bind(&source_id)
    .bind("full")
    .bind("running")
    .execute(state.db_pool.pool())
    .await?;

    info!("Created test sync run: {}", sync_run_id);

    let event_queue = EventQueue::new(state.db_pool.pool().clone());
    let mut content_ids = Vec::new();

    for (filename, content) in &text_files {
        // Store content in content storage
        let content_id = state.content_storage.store_text(content).await?;
        content_ids.push(content_id.clone());

        // Create document metadata
        let metadata = DocumentMetadata {
            title: Some(filename.replace(".txt", "").replace("_", " ")),
            mime_type: Some("text/plain".to_string()),
            size: Some(content.len().to_string()),
            url: Some(format!("file://test/{}", filename)),
            ..Default::default()
        };

        let permissions = DocumentPermissions {
            public: false,
            users: vec!["test_user".to_string()],
            groups: vec!["test_group".to_string()],
        };

        // Create connector event
        let event = ConnectorEvent::DocumentCreated {
            sync_run_id: sync_run_id.clone(),
            source_id: source_id.clone(),
            document_id: format!("test_doc_{}", filename.replace(".txt", "")),
            content_id,
            metadata,
            permissions,
        };

        // Queue the event
        event_queue.enqueue(&source_id, &event).await?;
    }

    info!(
        "Populated connector events queue with {} events",
        text_files.len()
    );

    // Step 3: Start queue processor in background to process events and create documents
    let queue_processor = QueueProcessor::new(state.clone()).with_parallelism(1);

    // Start the queue processor in the background
    let queue_processor_handle = tokio::spawn({
        let processor = queue_processor.clone();
        async move {
            if let Err(e) = processor.start().await {
                warn!("Queue processor error: {}", e);
            }
        }
    });

    // Wait for all events to be processed
    let doc_repo = DocumentRepository::new(state.db_pool.pool());
    let max_wait_time = Duration::from_secs(30);
    let start_time = std::time::Instant::now();

    loop {
        let events_pending = event_queue.get_queue_stats().await?.pending;
        info!("Events pending: {}", events_pending);

        if events_pending == 0 {
            info!("All events processed, queue is empty");
            break;
        }

        if start_time.elapsed() > max_wait_time {
            return Err(anyhow::anyhow!(
                "Timeout waiting for events to be processed"
            ));
        }

        sleep(Duration::from_millis(500)).await;
    }

    // Stop the queue processor
    queue_processor_handle.abort();

    // Verify all documents were created
    let mut documents_processed = 0;
    for (filename, _) in &text_files {
        let external_id = format!("test_doc_{}", filename.replace(".txt", ""));

        let doc = wait_for_document_creation(
            &doc_repo,
            &source_id,
            &external_id,
            Duration::from_secs(10),
        )
        .await?;
        info!(
            "Document created successfully: {} -> {}",
            external_id, doc.id
        );
        documents_processed += 1;
    }

    assert_eq!(
        documents_processed,
        text_files.len(),
        "All documents should be created"
    );
    info!("Successfully processed {} documents", documents_processed);

    // Step 4: Start embedding processor in background to generate embeddings
    let embedding_processor = EmbeddingProcessor::new(state.clone());

    // Start the embedding processor in the background
    let embedding_processor_handle = tokio::spawn(async move {
        if let Err(e) = embedding_processor.start().await {
            warn!("Embedding processor error: {}", e);
        }
    });

    // Wait for all embeddings to be processed
    let embedding_repo = EmbeddingRepository::new(state.db_pool.pool());
    let max_wait_time = Duration::from_secs(30);
    let start_time = std::time::Instant::now();

    loop {
        let queue_stats = state.embedding_queue.get_queue_stats().await?;
        info!("Embeddings pending: {}", queue_stats.pending);

        if queue_stats.pending == 0 {
            info!("All embeddings processed, queue is empty");
            break;
        }

        if start_time.elapsed() > max_wait_time {
            return Err(anyhow::anyhow!(
                "Timeout waiting for embeddings to be processed"
            ));
        }

        sleep(Duration::from_millis(500)).await;
    }

    // Stop the embedding processor
    embedding_processor_handle.abort();

    // Step 5: Validate embeddings were created for all documents
    let mut embeddings_processed = 0;
    for (filename, content) in &text_files {
        let external_id = format!("test_doc_{}", filename.replace(".txt", ""));

        // Find the document
        let doc = doc_repo
            .find_by_external_id(&source_id, &external_id)
            .await?
            .expect("Document should exist");

        // Wait for embeddings to be created
        let embeddings =
            wait_for_embeddings_creation(&embedding_repo, &doc.id, Duration::from_secs(15)).await?;

        info!(
            "Document '{}' has {} embeddings",
            filename,
            embeddings.len()
        );

        // Validate embeddings have reasonable properties
        assert!(
            !embeddings.is_empty(),
            "Document should have at least one embedding"
        );

        for embedding in &embeddings {
            // Validate embedding properties
            assert_eq!(embedding.document_id, doc.id);
            assert!(embedding.chunk_start_offset >= 0);
            assert!(embedding.chunk_end_offset > embedding.chunk_start_offset);
            assert!(embedding.chunk_end_offset <= content.len() as i32);
            assert_eq!(embedding.embedding.as_slice().len(), 1024); // jinaai/jina-embeddings-v3 dimension
            assert!(!embedding.model_name.is_empty());

            // Validate embedding vector has reasonable values (not all zeros)
            let non_zero_count = embedding
                .embedding
                .as_slice()
                .iter()
                .filter(|&&x| x != 0.0)
                .count();
            assert!(
                non_zero_count > 100,
                "Embedding should have many non-zero values"
            );
        }

        embeddings_processed += embeddings.len();
    }

    assert!(
        embeddings_processed > 0,
        "Should have generated some embeddings"
    );
    info!(
        "Successfully validated {} embeddings across {} documents",
        embeddings_processed,
        text_files.len()
    );

    // Additional validations
    let final_queue_stats = state.embedding_queue.get_queue_stats().await?;
    assert_eq!(
        final_queue_stats.pending, 0,
        "Embedding queue should be empty"
    );
    assert!(
        final_queue_stats.completed > 0,
        "Should have completed embeddings"
    );

    let event_queue_stats = event_queue.get_queue_stats().await?;
    assert_eq!(event_queue_stats.pending, 0, "Event queue should be empty");
    assert!(
        event_queue_stats.completed > 0,
        "Should have completed events"
    );

    info!("✅ Embedding processor integration test completed successfully!");
    info!(
        "📊 Final stats - Documents: {}, Embeddings: {}",
        documents_processed, embeddings_processed
    );

    Ok(())
}

/// Helper function to read all text files from test resources
async fn read_test_text_files() -> Result<HashMap<String, String>> {
    let test_resources_path = "tests/resources/texts";
    let mut text_files = HashMap::new();

    let entries = fs::read_dir(test_resources_path)?;

    for entry in entries {
        let entry = entry?;
        let path = entry.path();

        if path.extension().and_then(|s| s.to_str()) == Some("txt") {
            let filename = path
                .file_name()
                .and_then(|s| s.to_str())
                .ok_or_else(|| anyhow::anyhow!("Invalid filename"))?
                .to_string();

            let content = fs::read_to_string(&path)?;
            text_files.insert(filename, content);
        }
    }

    if text_files.is_empty() {
        return Err(anyhow::anyhow!(
            "No text files found in {}",
            test_resources_path
        ));
    }

    Ok(text_files)
}

/// Wait for a document to be created with polling and timeout
async fn wait_for_document_creation(
    repo: &DocumentRepository,
    source_id: &str,
    external_id: &str,
    timeout_duration: Duration,
) -> Result<shared::models::Document> {
    let result = timeout(timeout_duration, async {
        loop {
            if let Ok(Some(doc)) = repo.find_by_external_id(source_id, external_id).await {
                return doc;
            }
            sleep(Duration::from_millis(100)).await;
        }
    })
    .await;

    match result {
        Ok(doc) => Ok(doc),
        Err(_) => Err(anyhow::anyhow!(
            "Document {}:{} not created within timeout",
            source_id,
            external_id
        )),
    }
}

/// Wait for embeddings to be created for a document
async fn wait_for_embeddings_creation(
    repo: &EmbeddingRepository,
    document_id: &str,
    timeout_duration: Duration,
) -> Result<Vec<shared::models::Embedding>> {
    let result = timeout(timeout_duration, async {
        loop {
            if let Ok(embeddings) = repo.find_by_document_id(document_id).await {
                if !embeddings.is_empty() {
                    return embeddings;
                }
            }
            sleep(Duration::from_millis(100)).await;
        }
    })
    .await;

    match result {
        Ok(embeddings) => Ok(embeddings),
        Err(_) => Err(anyhow::anyhow!(
            "Embeddings for document {} not created within timeout",
            document_id
        )),
    }
}

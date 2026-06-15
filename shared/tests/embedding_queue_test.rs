#[cfg(test)]
mod tests {
    use shared::embedding_queue::EmbeddingQueue;
    use shared::test_environment::TestEnvironment;
    use sqlx::PgPool;
    use ulid::Ulid;

    const TEST_SOURCE_ID: &str = "01JGF7V3E0Y2R1X8P5Q7W9T4N7";

    async fn insert_active_embedding_provider(pool: &PgPool) {
        let id = Ulid::new().to_string();
        sqlx::query(
            r#"
            INSERT INTO embedding_providers (id, name, provider_type, config, is_current, is_deleted)
            VALUES ($1, 'test-provider', 'local', '{"model":"test-model"}', TRUE, FALSE)
            ON CONFLICT DO NOTHING
            "#,
        )
        .bind(&id)
        .execute(pool)
        .await
        .unwrap();
    }

    async fn create_document(pool: &PgPool) -> String {
        let doc_id = Ulid::new().to_string();
        sqlx::query(
            r#"
            INSERT INTO documents (id, source_id, external_id, title, content, metadata, permissions, attributes, created_at, updated_at)
            VALUES ($1, $2, $3, 'Test Doc', 'content', '{}', '{"users":["u1"]}', '{}', NOW(), NOW())
            "#,
        )
        .bind(&doc_id)
        .bind(TEST_SOURCE_ID)
        .bind(&format!("ext-{}", &doc_id))
        .execute(pool)
        .await
        .unwrap();
        doc_id
    }

    #[tokio::test]
    async fn test_enqueue_and_dequeue_lifecycle() {
        let env = TestEnvironment::new().await.unwrap();
        let pool = env.db_pool.pool().clone();
        let queue = EmbeddingQueue::new(pool.clone());
        insert_active_embedding_provider(&pool).await;

        let doc_id = create_document(&pool).await;

        let queue_id = queue.enqueue(doc_id.clone()).await.unwrap().unwrap();
        assert!(!queue_id.is_empty());

        let batch = queue.dequeue_batch(10).await.unwrap();
        assert_eq!(batch.len(), 1);
        assert_eq!(batch[0].document_id, doc_id);
        assert_eq!(batch[0].status.to_string(), "processing");

        // Dequeuing again should return empty
        let batch2 = queue.dequeue_batch(10).await.unwrap();
        assert!(batch2.is_empty());
    }

    #[tokio::test]
    async fn test_enqueue_batch() {
        let env = TestEnvironment::new().await.unwrap();
        let pool = env.db_pool.pool().clone();
        let queue = EmbeddingQueue::new(pool.clone());
        insert_active_embedding_provider(&pool).await;

        let mut doc_ids = Vec::new();
        for _ in 0..3 {
            doc_ids.push(create_document(&pool).await);
        }

        let queue_ids = queue.enqueue_batch(doc_ids.clone()).await.unwrap();
        assert_eq!(queue_ids.len(), 3);

        let batch = queue.dequeue_batch(10).await.unwrap();
        assert_eq!(batch.len(), 3);
    }

    #[tokio::test]
    async fn test_enqueue_batch_missing_current_embeddings_enqueues_multiple_missing_only() {
        let env = TestEnvironment::new().await.unwrap();
        let pool = env.db_pool.pool().clone();
        let queue = EmbeddingQueue::new(pool.clone());
        insert_active_embedding_provider(&pool).await;

        let doc_with_embedding = create_document(&pool).await;
        let doc_with_active_queue = create_document(&pool).await;
        let missing_doc_1 = create_document(&pool).await;
        let missing_doc_2 = create_document(&pool).await;

        sqlx::query(
            r#"
            INSERT INTO embeddings (id, document_id, chunk_index, chunk_start_offset, chunk_end_offset, embedding, model_name, dimensions)
            VALUES ($1, $2, 0, 0, 10, '[0.1,0.2,0.3]'::vector, 'test-model', 3)
            "#,
        )
        .bind(Ulid::new().to_string())
        .bind(&doc_with_embedding)
        .execute(&pool)
        .await
        .unwrap();

        queue
            .enqueue(doc_with_active_queue.clone())
            .await
            .unwrap()
            .unwrap();

        let queue_ids = queue
            .enqueue_batch_missing_current_embeddings(vec![
                doc_with_embedding.clone(),
                doc_with_active_queue.clone(),
                missing_doc_1.clone(),
                missing_doc_2.clone(),
            ])
            .await
            .unwrap();
        assert_eq!(queue_ids.len(), 2);

        let queued_missing_doc_count: (i64,) = sqlx::query_as(
            r#"
            SELECT COUNT(*)
            FROM embedding_queue
            WHERE document_id = ANY($1)
            "#,
        )
        .bind(vec![missing_doc_1, missing_doc_2])
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(queued_missing_doc_count.0, 2);

        let skipped_doc_count: (i64,) = sqlx::query_as(
            r#"
            SELECT COUNT(*)
            FROM embedding_queue
            WHERE document_id = $1
            "#,
        )
        .bind(&doc_with_embedding)
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(skipped_doc_count.0, 0);

        let existing_active_queue_count: (i64,) = sqlx::query_as(
            r#"
            SELECT COUNT(*)
            FROM embedding_queue
            WHERE document_id = $1
            "#,
        )
        .bind(&doc_with_active_queue)
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(existing_active_queue_count.0, 1);
    }

    #[tokio::test]
    async fn test_dequeue_picks_up_failed_with_low_retry_count() {
        let env = TestEnvironment::new().await.unwrap();
        let pool = env.db_pool.pool().clone();
        let queue = EmbeddingQueue::new(pool.clone());
        insert_active_embedding_provider(&pool).await;

        let doc_id = create_document(&pool).await;
        let queue_id = queue.enqueue(doc_id.clone()).await.unwrap().unwrap();

        // Dequeue then mark failed (retry_count becomes 1)
        let batch = queue.dequeue_batch(10).await.unwrap();
        assert_eq!(batch.len(), 1);
        queue
            .mark_failed(&queue_id, "transient error")
            .await
            .unwrap();

        // Dequeue should pick it up again (status=failed, retry_count=1 < 3)
        let batch = queue.dequeue_batch(10).await.unwrap();
        assert_eq!(batch.len(), 1);
        assert_eq!(batch[0].id, queue_id);
    }

    #[tokio::test]
    async fn test_dequeue_skips_failed_with_max_retries() {
        let env = TestEnvironment::new().await.unwrap();
        let pool = env.db_pool.pool().clone();
        let queue = EmbeddingQueue::new(pool.clone());
        insert_active_embedding_provider(&pool).await;

        let doc_id = create_document(&pool).await;
        let queue_id = queue.enqueue(doc_id.clone()).await.unwrap().unwrap();

        // Fail 3 times to exhaust retries
        for i in 0..3 {
            let batch = queue.dequeue_batch(10).await.unwrap();
            assert_eq!(batch.len(), 1, "Should dequeue on attempt {}", i);
            queue
                .mark_failed(&queue_id, &format!("error {}", i))
                .await
                .unwrap();
        }

        // retry_count is now 3 (>= 3), dequeue should skip it
        let batch = queue.dequeue_batch(10).await.unwrap();
        assert!(batch.is_empty());
    }

    #[tokio::test]
    async fn test_mark_completed_batch() {
        let env = TestEnvironment::new().await.unwrap();
        let pool = env.db_pool.pool().clone();
        let queue = EmbeddingQueue::new(pool.clone());
        insert_active_embedding_provider(&pool).await;

        let doc_id = create_document(&pool).await;
        let queue_id = queue.enqueue(doc_id).await.unwrap().unwrap();

        let batch = queue.dequeue_batch(10).await.unwrap();
        assert_eq!(batch.len(), 1);

        queue.mark_completed(&[queue_id.clone()]).await.unwrap();

        let stats = queue.get_queue_stats().await.unwrap();
        assert_eq!(stats.completed, 1);
        assert_eq!(stats.processing, 0);
    }

    #[tokio::test]
    async fn test_mark_failed_batch() {
        let env = TestEnvironment::new().await.unwrap();
        let pool = env.db_pool.pool().clone();
        let queue = EmbeddingQueue::new(pool.clone());
        insert_active_embedding_provider(&pool).await;

        let mut ids = Vec::new();
        for _ in 0..2 {
            let doc_id = create_document(&pool).await;
            let qid = queue.enqueue(doc_id).await.unwrap().unwrap();
            ids.push(qid);
        }

        queue.dequeue_batch(10).await.unwrap();

        queue
            .mark_failed_batch(&ids, "batch processing error")
            .await
            .unwrap();

        let stats = queue.get_queue_stats().await.unwrap();
        assert_eq!(stats.failed, 2);
    }

    #[tokio::test]
    async fn test_recover_stale_processing_items() {
        let env = TestEnvironment::new().await.unwrap();
        let pool = env.db_pool.pool().clone();
        let queue = EmbeddingQueue::new(pool.clone());
        insert_active_embedding_provider(&pool).await;

        let doc_id = create_document(&pool).await;
        queue.enqueue(doc_id).await.unwrap().unwrap();

        // Dequeue sets processing + processing_started_at
        queue.dequeue_batch(10).await.unwrap();

        // Recover with timeout=0 treats all processing items as stale
        let recovered = queue.recover_stale_processing_items(0).await.unwrap();
        assert_eq!(recovered, 1);

        // Should be pending again and dequeue-able
        let batch = queue.dequeue_batch(10).await.unwrap();
        assert_eq!(batch.len(), 1);
    }

    #[tokio::test]
    async fn test_queue_stats() {
        let env = TestEnvironment::new().await.unwrap();
        let pool = env.db_pool.pool().clone();
        let queue = EmbeddingQueue::new(pool.clone());
        insert_active_embedding_provider(&pool).await;

        let stats = queue.get_queue_stats().await.unwrap();
        assert_eq!(stats.pending, 0);
        assert_eq!(stats.processing, 0);
        assert_eq!(stats.completed, 0);
        assert_eq!(stats.failed, 0);

        // Add 3 items
        for _ in 0..3 {
            let doc_id = create_document(&pool).await;
            queue.enqueue(doc_id).await.unwrap().unwrap();
        }

        let stats = queue.get_queue_stats().await.unwrap();
        assert_eq!(stats.pending, 3);

        // Dequeue 2
        queue.dequeue_batch(2).await.unwrap();
        let stats = queue.get_queue_stats().await.unwrap();
        assert_eq!(stats.pending, 1);
        assert_eq!(stats.processing, 2);
    }

    #[tokio::test]
    async fn test_enqueue_skipped_without_active_provider() {
        let env = TestEnvironment::new().await.unwrap();
        let pool = env.db_pool.pool().clone();
        let queue = EmbeddingQueue::new(pool.clone());
        sqlx::query("UPDATE embedding_providers SET is_current = FALSE")
            .execute(&pool)
            .await
            .unwrap();

        let doc_id = create_document(&pool).await;

        let result = queue.enqueue(doc_id).await.unwrap();
        assert!(result.is_none());

        let mut doc_ids = Vec::new();
        for _ in 0..3 {
            doc_ids.push(create_document(&pool).await);
        }

        let ids = queue.enqueue_batch(doc_ids).await.unwrap();
        assert!(ids.is_empty());

        let stats = queue.get_queue_stats().await.unwrap();
        assert_eq!(stats.pending, 0);
    }
}

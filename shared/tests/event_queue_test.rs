#[cfg(test)]
mod tests {
    use shared::models::{ConnectorEvent, DocumentMetadata, DocumentPermissions, EventStatus};
    use shared::queue::EventQueue;
    use shared::test_environment::TestEnvironment;

    const TEST_SOURCE_ID: &str = "01JGF7V3E0Y2R1X8P5Q7W9T4N7";

    fn make_event(sync_run_id: &str, doc_id: &str) -> ConnectorEvent {
        ConnectorEvent::DocumentCreated {
            sync_run_id: sync_run_id.to_string(),
            source_id: TEST_SOURCE_ID.to_string(),
            document_id: doc_id.to_string(),
            content_id: "content-1".to_string(),
            metadata: DocumentMetadata::default(),
            permissions: DocumentPermissions {
                public: false,
                users: vec!["user1".to_string()],
                groups: vec![],
            },
            attributes: None,
        }
    }

    #[tokio::test]
    async fn test_enqueue_and_dequeue_lifecycle() {
        let env = TestEnvironment::new().await.unwrap();
        let queue = EventQueue::new(env.db_pool.pool().clone());

        let event = make_event("run-1", "doc-1");
        let event_id = queue.enqueue(TEST_SOURCE_ID, &event).await.unwrap();
        assert!(!event_id.is_empty());

        let batch = queue.dequeue_batch(10).await.unwrap();
        assert_eq!(batch.len(), 1);
        assert_eq!(batch[0].id, event_id);
        assert_eq!(batch[0].event_type, "document_created");
        assert!(matches!(batch[0].status, EventStatus::Processing));

        // Dequeuing again should return empty (already processing)
        let batch2 = queue.dequeue_batch(10).await.unwrap();
        assert!(batch2.is_empty());
    }

    #[tokio::test]
    async fn test_dequeue_batches_by_single_sync_run() {
        let env = TestEnvironment::new().await.unwrap();
        let queue = EventQueue::new(env.db_pool.pool().clone());

        let run_a = ulid::Ulid::new().to_string();
        let run_b = ulid::Ulid::new().to_string();

        // Enqueue 3 events for run_a and 1 for run_b
        for i in 0..3 {
            let event = make_event(&run_a, &format!("doc-a{}", i));
            queue.enqueue(TEST_SOURCE_ID, &event).await.unwrap();
        }
        let event_b = make_event(&run_b, "doc-b1");
        queue.enqueue(TEST_SOURCE_ID, &event_b).await.unwrap();

        // Dequeue should pick run_a (most pending events)
        let batch = queue.dequeue_batch(10).await.unwrap();
        assert_eq!(batch.len(), 3);
        for item in &batch {
            assert_eq!(item.sync_run_id, run_a);
        }
    }

    #[tokio::test]
    async fn test_mark_completed() {
        let env = TestEnvironment::new().await.unwrap();
        let queue = EventQueue::new(env.db_pool.pool().clone());

        let event = make_event("run-1", "doc-1");
        let event_id = queue.enqueue(TEST_SOURCE_ID, &event).await.unwrap();

        let batch = queue.dequeue_batch(10).await.unwrap();
        assert_eq!(batch.len(), 1);

        queue.mark_completed(&event_id).await.unwrap();

        let stats = queue.get_queue_stats().await.unwrap();
        assert_eq!(stats.completed, 1);
        assert_eq!(stats.pending, 0);
        assert_eq!(stats.processing, 0);
    }

    #[tokio::test]
    async fn test_mark_failed_increments_retry_count() {
        let env = TestEnvironment::new().await.unwrap();
        let queue = EventQueue::new(env.db_pool.pool().clone());

        let event = make_event("run-1", "doc-1");
        let event_id = queue.enqueue(TEST_SOURCE_ID, &event).await.unwrap();

        queue.dequeue_batch(10).await.unwrap();

        queue.mark_failed(&event_id, "timeout error").await.unwrap();

        let stats = queue.get_queue_stats().await.unwrap();
        assert_eq!(stats.failed, 1);
    }

    #[tokio::test]
    async fn test_dead_letter_after_max_retries() {
        let env = TestEnvironment::new().await.unwrap();
        let queue = EventQueue::new(env.db_pool.pool().clone());

        let event = make_event("run-1", "doc-1");
        let event_id = queue.enqueue(TEST_SOURCE_ID, &event).await.unwrap();

        // Dequeue and fail 3 times (default max_retries = 3)
        queue.dequeue_batch(10).await.unwrap();
        queue.mark_failed(&event_id, "error 1").await.unwrap();

        queue.retry_failed_events().await.unwrap();
        queue.dequeue_batch(10).await.unwrap();
        queue.mark_failed(&event_id, "error 2").await.unwrap();

        queue.retry_failed_events().await.unwrap();
        queue.dequeue_batch(10).await.unwrap();
        queue.mark_failed(&event_id, "error 3").await.unwrap();

        // After 3 failures (retry_count=3 >= max_retries=3), should be dead_letter
        let stats = queue.get_queue_stats().await.unwrap();
        assert_eq!(stats.dead_letter, 1);
        assert_eq!(stats.failed, 0);
    }

    #[tokio::test]
    async fn test_retry_failed_events() {
        let env = TestEnvironment::new().await.unwrap();
        let queue = EventQueue::new(env.db_pool.pool().clone());

        let event = make_event("run-1", "doc-1");
        let event_id = queue.enqueue(TEST_SOURCE_ID, &event).await.unwrap();

        queue.dequeue_batch(10).await.unwrap();
        queue
            .mark_failed(&event_id, "transient error")
            .await
            .unwrap();

        let retried = queue.retry_failed_events().await.unwrap();
        assert_eq!(retried, 1);

        // Should now be dequeue-able again
        let batch = queue.dequeue_batch(10).await.unwrap();
        assert_eq!(batch.len(), 1);
    }

    #[tokio::test]
    async fn test_recover_stale_processing_items() {
        let env = TestEnvironment::new().await.unwrap();
        let queue = EventQueue::new(env.db_pool.pool().clone());

        let event = make_event("run-1", "doc-1");
        queue.enqueue(TEST_SOURCE_ID, &event).await.unwrap();

        queue.dequeue_batch(10).await.unwrap();

        // timeout=0 means all processing items are considered stale
        let recovered = queue.recover_stale_processing_items(0).await.unwrap();
        assert_eq!(recovered, 1);

        let batch = queue.dequeue_batch(10).await.unwrap();
        assert_eq!(batch.len(), 1);
    }

    #[tokio::test]
    async fn test_batch_mark_completed() {
        let env = TestEnvironment::new().await.unwrap();
        let queue = EventQueue::new(env.db_pool.pool().clone());

        let mut ids = Vec::new();
        for i in 0..3 {
            let event = make_event("run-1", &format!("doc-{}", i));
            let id = queue.enqueue(TEST_SOURCE_ID, &event).await.unwrap();
            ids.push(id);
        }

        queue.dequeue_batch(10).await.unwrap();

        let completed = queue.mark_events_completed_batch(ids).await.unwrap();
        assert_eq!(completed, 3);

        let stats = queue.get_queue_stats().await.unwrap();
        assert_eq!(stats.completed, 3);
    }

    #[tokio::test]
    async fn test_batch_mark_failed() {
        let env = TestEnvironment::new().await.unwrap();
        let queue = EventQueue::new(env.db_pool.pool().clone());

        let mut ids = Vec::new();
        for i in 0..2 {
            let event = make_event("run-1", &format!("doc-{}", i));
            let id = queue.enqueue(TEST_SOURCE_ID, &event).await.unwrap();
            ids.push(id);
        }

        queue.dequeue_batch(10).await.unwrap();

        let errors: Vec<(String, String)> = ids
            .into_iter()
            .map(|id| (id, "batch error".to_string()))
            .collect();

        let failed = queue.mark_events_failed_batch(errors).await.unwrap();
        assert_eq!(failed, 2);

        let stats = queue.get_queue_stats().await.unwrap();
        assert_eq!(stats.failed, 2);
    }

    #[tokio::test]
    async fn test_queue_stats() {
        let env = TestEnvironment::new().await.unwrap();
        let queue = EventQueue::new(env.db_pool.pool().clone());

        let stats = queue.get_queue_stats().await.unwrap();
        assert_eq!(stats.pending, 0);
        assert_eq!(stats.processing, 0);
        assert_eq!(stats.completed, 0);
        assert_eq!(stats.failed, 0);
        assert_eq!(stats.dead_letter, 0);

        for i in 0..3 {
            let event = make_event("run-1", &format!("doc-{}", i));
            queue.enqueue(TEST_SOURCE_ID, &event).await.unwrap();
        }

        let stats = queue.get_queue_stats().await.unwrap();
        assert_eq!(stats.pending, 3);
    }

    #[tokio::test]
    async fn test_pending_count() {
        let env = TestEnvironment::new().await.unwrap();
        let queue = EventQueue::new(env.db_pool.pool().clone());

        assert_eq!(queue.get_pending_count().await.unwrap(), 0);

        for i in 0..5 {
            let event = make_event("run-1", &format!("doc-{}", i));
            queue.enqueue(TEST_SOURCE_ID, &event).await.unwrap();
        }

        assert_eq!(queue.get_pending_count().await.unwrap(), 5);

        queue.dequeue_batch(2).await.unwrap();
        assert_eq!(queue.get_pending_count().await.unwrap(), 3);
    }

    /// Regression test: events whose `sync_run_id` no longer exists in
    /// `sync_runs` (e.g. because the sync_run row was GC'd or the source
    /// was deleted but the queue rows weren't) must be drainable via
    /// `dequeue_batch_orphans`. Before the `FOR UPDATE OF q SKIP LOCKED`
    /// fix, this query failed at parse time with "FOR UPDATE cannot be
    /// applied to the nullable side of an outer join", which propagated
    /// up through `process_batch` and starved the entire indexer.
    #[tokio::test]
    async fn test_dequeue_batch_orphans_drains_events_with_missing_sync_run() {
        let env = TestEnvironment::new().await.unwrap();
        let queue = EventQueue::new(env.db_pool.pool().clone());

        // Enqueue events under sync_run_ids that have no row in sync_runs —
        // exactly the orphan condition produced when a source is deleted
        // and its sync_runs cascade away faster than the queue is drained.
        for i in 0..3 {
            let event = make_event("nonexistent-run", &format!("orphan-doc-{}", i));
            queue.enqueue(TEST_SOURCE_ID, &event).await.unwrap();
        }

        let batch = queue.dequeue_batch_orphans(10).await.unwrap();
        assert_eq!(batch.len(), 3);
        for item in &batch {
            assert!(matches!(item.status, EventStatus::Processing));
        }

        // Re-running should return empty — claimed rows are now `processing`.
        let again = queue.dequeue_batch_orphans(10).await.unwrap();
        assert!(again.is_empty());
    }

    /// Companion: `dequeue_batch_by_sync_type` must continue to work for
    /// events whose sync_run row exists. We applied the same `FOR UPDATE
    /// OF q` scoping there for hygiene; this test pins that the inner
    /// join + per-sync-type filter still routes correctly.
    #[tokio::test]
    async fn test_dequeue_batch_by_sync_type_routes_by_sync_runs() {
        use shared::models::SyncType;

        let env = TestEnvironment::new().await.unwrap();
        let pool = env.db_pool.pool().clone();
        let queue = EventQueue::new(pool.clone());

        // Insert two sync_runs with different sync_types.
        let full_run = ulid::Ulid::new().to_string();
        let inc_run = ulid::Ulid::new().to_string();
        for (id, sync_type) in [(&full_run, "full"), (&inc_run, "incremental")] {
            sqlx::query(
                r#"
                INSERT INTO sync_runs (id, source_id, sync_type, status, started_at, created_at, updated_at)
                VALUES ($1, $2, $3, 'running', NOW(), NOW(), NOW())
                "#,
            )
            .bind(id)
            .bind(TEST_SOURCE_ID)
            .bind(sync_type)
            .execute(&pool)
            .await
            .unwrap();
        }

        // Two events under the full sync_run, one under incremental.
        for i in 0..2 {
            let event = make_event(&full_run, &format!("full-doc-{}", i));
            queue.enqueue(TEST_SOURCE_ID, &event).await.unwrap();
        }
        let event_inc = make_event(&inc_run, "inc-doc-1");
        queue.enqueue(TEST_SOURCE_ID, &event_inc).await.unwrap();

        // Asking for `Full` should pick up only the full-run events.
        let full_batch = queue
            .dequeue_batch_by_sync_type(10, SyncType::Full)
            .await
            .unwrap();
        assert_eq!(full_batch.len(), 2);
        for item in &full_batch {
            assert_eq!(item.sync_run_id, full_run);
        }

        // The incremental event remains and is picked up by its own filter.
        let inc_batch = queue
            .dequeue_batch_by_sync_type(10, SyncType::Incremental)
            .await
            .unwrap();
        assert_eq!(inc_batch.len(), 1);
        assert_eq!(inc_batch[0].sync_run_id, inc_run);
    }
}

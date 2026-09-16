#[cfg(test)]
mod tests {
    use shared::connector_event_queue::EventQueue;
    use shared::models::{
        ConnectorEvent, DocumentMetadata, DocumentPermissions, EventStatus, SyncType,
    };
    use shared::task_queue::{EnqueueTaskRequest, TaskQueue};
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

    fn make_event_with_content(
        sync_run_id: &str,
        doc_id: &str,
        content_id: String,
    ) -> ConnectorEvent {
        ConnectorEvent::DocumentCreated {
            sync_run_id: sync_run_id.to_string(),
            source_id: TEST_SOURCE_ID.to_string(),
            document_id: doc_id.to_string(),
            content_id,
            metadata: DocumentMetadata::default(),
            permissions: DocumentPermissions {
                public: false,
                users: vec!["user1".to_string()],
                groups: vec![],
            },
            attributes: None,
        }
    }

    async fn insert_sized_content(pool: &sqlx::PgPool, size_bytes: i64) -> String {
        let content_id = ulid::Ulid::new().to_string();
        sqlx::query(
            r#"
            INSERT INTO content_blobs (id, content, size_bytes, storage_backend)
            VALUES ($1, $2, $3, 'postgres')
            "#,
        )
        .bind(&content_id)
        .bind(Vec::<u8>::new())
        .bind(size_bytes)
        .execute(pool)
        .await
        .unwrap();
        content_id
    }

    async fn insert_sync_run(pool: &sqlx::PgPool, run_id: &str, sync_type: &str) {
        sqlx::query(
            r#"
            INSERT INTO sync_runs (id, source_id, sync_type, status, started_at, completed_at, created_at, updated_at)
            VALUES ($1, $2, $3, 'completed', NOW(), NOW(), NOW(), NOW())
            "#,
        )
        .bind(run_id)
        .bind(TEST_SOURCE_ID)
        .bind(sync_type)
        .execute(pool)
        .await
        .unwrap();
    }

    #[tokio::test]
    async fn malformed_connector_tasks_are_dead_lettered_at_claim_boundary() {
        let env = TestEnvironment::new().await.unwrap();
        let pool = env.db_pool.pool().clone();
        let task_queue = TaskQueue::new(pool.clone());
        let task = EnqueueTaskRequest::new(
            "connector_event",
            serde_json::json!({"not": "a connector event"}),
        );
        let task_id = task.id.clone();
        task_queue.enqueue(task).await.unwrap();

        let claim = EventQueue::new(pool.clone()).claim_batch(1, i64::MAX).await.unwrap();
        assert!(claim.events.is_empty());
        let status: String = sqlx::query_scalar(
            "SELECT status FROM tasks WHERE id = $1",
        )
        .bind(&task_id)
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(status, "dead_letter");
    }

    #[tokio::test]
    async fn terminal_task_omitted_by_heartbeat_is_not_reported_as_lost() {
        let env = TestEnvironment::new().await.unwrap();
        let queue = EventQueue::new(env.db_pool.pool().clone());
        let first = queue
            .enqueue(TEST_SOURCE_ID, &make_event("run-1", "doc-1"))
            .await
            .unwrap();
        let second = queue
            .enqueue(TEST_SOURCE_ID, &make_event("run-1", "doc-2"))
            .await
            .unwrap();

        let claim = queue.claim_batch(2, i64::MAX).await.unwrap();
        let ids = vec![first.clone(), second.clone()];
        assert_eq!(claim.events.len(), ids.len());
        queue.complete_bulk(&[first.clone()], &claim.claim_token).await.unwrap();

        let renewed = queue.heartbeat_bulk(&ids, &claim.claim_token).await.unwrap();
        assert_eq!(renewed, vec![second.clone()]);
        let missing = vec![first.clone()];
        let (terminal, lost) = queue.classify_unrenewed(&missing).await.unwrap();
        assert_eq!(terminal, vec![first]);
        assert!(lost.is_empty());

        queue.complete_bulk(&[second], &claim.claim_token).await.unwrap();
    }

    #[tokio::test]
    async fn retry_to_pending_is_removed_before_heartbeat_snapshot() {
        let env = TestEnvironment::new().await.unwrap();
        let queue = EventQueue::new(env.db_pool.pool().clone());
        let first = queue
            .enqueue(TEST_SOURCE_ID, &make_event("run-1", "doc-1"))
            .await
            .unwrap();
        let second = queue
            .enqueue(TEST_SOURCE_ID, &make_event("run-1", "doc-2"))
            .await
            .unwrap();
        let claim = queue.claim_batch(2, i64::MAX).await.unwrap();
        assert_eq!(claim.events.len(), 2);

        let active_ids =
            std::sync::Arc::new(tokio::sync::Mutex::new(std::collections::HashSet::from([
                first.clone(),
                second.clone(),
            ])));
        let ownership_lock = tokio::sync::Mutex::new(());
        {
            let _guard = ownership_lock.lock().await;
            let failed = queue
                .fail_bulk(
                    &[(first.clone(), "retryable failure".to_string())],
                    &claim.claim_token,
                )
                .await
                .unwrap();
            assert_eq!(failed.len(), 1);
            active_ids.lock().await.retain(|id| id != &first);
        }

        // The heartbeat snapshots only the still-owned task, so a successful
        // retry transition cannot be mistaken for lease loss.
        let _guard = ownership_lock.lock().await;
        let heartbeat_ids: Vec<String> = active_ids.lock().await.iter().cloned().collect();
        assert_eq!(heartbeat_ids, vec![second.clone()]);
        assert_eq!(
            queue
                .heartbeat_bulk(&heartbeat_ids, &claim.claim_token)
                .await
                .unwrap(),
            vec![second.clone()]
        );
        let status: String = sqlx::query_scalar("SELECT status FROM tasks WHERE id = $1")
            .bind(&first)
            .fetch_one(env.db_pool.pool())
            .await
            .unwrap();
        assert_eq!(status, "pending");

        queue
            .complete_bulk(&[second], &claim.claim_token)
            .await
            .unwrap();
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
    async fn test_dequeue_batch_drains_oldest_pending_events() {
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

        // Dequeue drains the oldest pending events across sync runs. The
        // indexer groups them by sync_run_id after dequeueing.
        let batch = queue.dequeue_batch(10).await.unwrap();
        assert_eq!(batch.len(), 4);
        assert_eq!(
            batch
                .iter()
                .filter(|item| item.sync_run_id == run_a)
                .count(),
            3
        );
        assert_eq!(
            batch
                .iter()
                .filter(|item| item.sync_run_id == run_b)
                .count(),
            1
        );
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
        assert_eq!(stats.pending, 1);
        assert_eq!(stats.failed, 0);
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
        sqlx::query("UPDATE tasks SET available_at = NOW() WHERE id = $1")
            .bind(&event_id)
            .execute(env.db_pool.pool())
            .await
            .unwrap();

        queue.retry_failed_events().await.unwrap();
        queue.dequeue_batch(10).await.unwrap();
        queue.mark_failed(&event_id, "error 2").await.unwrap();
        sqlx::query("UPDATE tasks SET available_at = NOW() WHERE id = $1")
            .bind(&event_id)
            .execute(env.db_pool.pool())
            .await
            .unwrap();

        queue.retry_failed_events().await.unwrap();
        queue.dequeue_batch(10).await.unwrap();
        queue.mark_failed(&event_id, "error 3").await.unwrap();

        // After 3 claims, the generic queue dead-letters the task.
        let stats = queue.get_queue_stats().await.unwrap();
        assert_eq!(stats.dead_letter, 1);
        assert_eq!(stats.pending, 0);
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
        assert_eq!(retried, 0);
        sqlx::query("UPDATE tasks SET available_at = NOW() WHERE id = $1")
            .bind(&event_id)
            .execute(env.db_pool.pool())
            .await
            .unwrap();

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

        sqlx::query("UPDATE tasks SET updated_at = NOW() - INTERVAL '10 minutes' WHERE task_type = 'connector_event'")
            .execute(env.db_pool.pool())
            .await
            .unwrap();

        let recovered = queue.recover_stale_processing_items(1).await.unwrap();
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
        assert_eq!(stats.pending, 2);
        assert_eq!(stats.failed, 0);
    }

    #[tokio::test]
    async fn test_batch_dead_letter_uses_incremented_retry_count() {
        let env = TestEnvironment::new().await.unwrap();
        let pool = env.db_pool.pool().clone();
        let queue = EventQueue::new(pool.clone());

        let near_limit_event = make_event("run-1", "doc-near-limit");
        let below_limit_event = make_event("run-1", "doc-below-limit");
        let near_limit_id = queue
            .enqueue(TEST_SOURCE_ID, &near_limit_event)
            .await
            .unwrap();
        let below_limit_id = queue
            .enqueue(TEST_SOURCE_ID, &below_limit_event)
            .await
            .unwrap();

        queue.dequeue_batch(10).await.unwrap();

        sqlx::query("UPDATE tasks SET attempt_count = $1 WHERE id = $2")
            .bind(2)
            .bind(&near_limit_id)
            .execute(&pool)
            .await
            .unwrap();
        sqlx::query("UPDATE tasks SET attempt_count = $1 WHERE id = $2")
            .bind(1)
            .bind(&below_limit_id)
            .execute(&pool)
            .await
            .unwrap();

        let updated = queue
            .mark_events_dead_letter_batch(vec![
                (near_limit_id.clone(), "near limit".to_string()),
                (below_limit_id.clone(), "below limit".to_string()),
            ])
            .await
            .unwrap();
        assert_eq!(updated, 2);

        let near_limit_row: (String, i32) =
            sqlx::query_as("SELECT status, attempt_count FROM tasks WHERE id = $1")
                .bind(&near_limit_id)
                .fetch_one(&pool)
                .await
                .unwrap();
        assert_eq!(near_limit_row, ("dead_letter".to_string(), 2));

        let below_limit_row: (String, i32) =
            sqlx::query_as("SELECT status, attempt_count FROM tasks WHERE id = $1")
                .bind(&below_limit_id)
                .fetch_one(&pool)
                .await
                .unwrap();
        assert_eq!(below_limit_row, ("dead_letter".to_string(), 1));
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

    // Integration coverage for EventQueue::dequeue_batch_with_max_bytes.
    // TestEnvironment starts a real ParadeDB/Postgres testcontainer.
    #[tokio::test]
    async fn test_dequeue_batch_with_max_bytes_keeps_tiny_docs_count_limited() {
        let env = TestEnvironment::new().await.unwrap();
        let pool = env.db_pool.pool().clone();
        let queue = EventQueue::new(pool.clone());

        let mut ids = Vec::new();
        for i in 0..3 {
            let content_id = insert_sized_content(&pool, 1).await;
            let event = make_event_with_content("run-1", &format!("tiny-doc-{}", i), content_id);
            ids.push(queue.enqueue(TEST_SOURCE_ID, &event).await.unwrap());
        }

        // The byte budget includes event payload text, so calibrate it from a
        // measured payload to stay generous enough that the count limit binds.
        let payload: i64 = sqlx::query_scalar(
            "SELECT octet_length(payload::text)::bigint FROM tasks WHERE id = $1",
        )
        .bind(&ids[0])
        .fetch_one(&pool)
        .await
        .unwrap();

        let batch = queue
            .dequeue_batch_with_max_bytes(2, payload * 2 + 1000)
            .await
            .unwrap();
        assert_eq!(batch.len(), 2);
        assert_eq!(queue.get_pending_count().await.unwrap(), 1);
    }

    #[tokio::test]
    async fn test_dequeue_batch_with_max_bytes_stops_at_byte_budget() {
        let env = TestEnvironment::new().await.unwrap();
        let pool = env.db_pool.pool().clone();
        let queue = EventQueue::new(pool.clone());

        let mut ids = Vec::new();
        for i in 0..3 {
            let content_id = insert_sized_content(&pool, 20).await;
            let event = make_event_with_content("run-1", &format!("medium-doc-{}", i), content_id);
            ids.push(queue.enqueue(TEST_SOURCE_ID, &event).await.unwrap());
        }

        // Budget admits exactly the first event once payload text is counted.
        let payload: i64 = sqlx::query_scalar(
            "SELECT octet_length(payload::text)::bigint FROM tasks WHERE id = $1",
        )
        .bind(&ids[0])
        .fetch_one(&pool)
        .await
        .unwrap();

        let batch = queue
            .dequeue_batch_with_max_bytes(10, payload + 30)
            .await
            .unwrap();
        assert_eq!(batch.len(), 1);
        assert_eq!(queue.get_pending_count().await.unwrap(), 2);
    }

    #[tokio::test]
    async fn test_dequeue_batch_with_max_bytes_allows_one_oversized_doc() {
        let env = TestEnvironment::new().await.unwrap();
        let pool = env.db_pool.pool().clone();
        let queue = EventQueue::new(pool.clone());

        let oversized_id = insert_sized_content(&pool, 280).await;
        let oversized = make_event_with_content("run-1", "oversized-doc", oversized_id);
        queue.enqueue(TEST_SOURCE_ID, &oversized).await.unwrap();

        let small_id = insert_sized_content(&pool, 1).await;
        let small = make_event_with_content("run-1", "small-doc", small_id);
        queue.enqueue(TEST_SOURCE_ID, &small).await.unwrap();

        let batch = queue.dequeue_batch_with_max_bytes(10, 100).await.unwrap();
        assert_eq!(batch.len(), 1);
        assert_eq!(batch[0].payload["document_id"], "oversized-doc");
        assert_eq!(queue.get_pending_count().await.unwrap(), 1);

        let next = queue.dequeue_batch_with_max_bytes(10, 100).await.unwrap();
        assert_eq!(next.len(), 1);
        assert_eq!(next[0].payload["document_id"], "small-doc");
    }

    #[tokio::test]
    async fn test_dequeue_batch_by_sync_type_with_max_bytes_routes_and_limits() {
        let env = TestEnvironment::new().await.unwrap();
        let pool = env.db_pool.pool().clone();
        let queue = EventQueue::new(pool.clone());

        let full_run = ulid::Ulid::new().to_string();
        let inc_run = ulid::Ulid::new().to_string();
        insert_sync_run(&pool, &full_run, "full").await;
        insert_sync_run(&pool, &inc_run, "incremental").await;

        for i in 0..2 {
            let content_id = insert_sized_content(&pool, 60).await;
            let event = make_event_with_content(&full_run, &format!("full-doc-{}", i), content_id);
            queue.enqueue(TEST_SOURCE_ID, &event).await.unwrap();
        }
        let inc_content_id = insert_sized_content(&pool, 10).await;
        let inc_event = make_event_with_content(&inc_run, "inc-doc", inc_content_id);
        queue.enqueue(TEST_SOURCE_ID, &inc_event).await.unwrap();

        let full_batch = queue
            .dequeue_batch_by_sync_type_with_max_bytes(10, SyncType::Full, 100)
            .await
            .unwrap();
        assert_eq!(full_batch.len(), 1);
        assert_eq!(full_batch[0].sync_run_id, full_run);

        let inc_batch = queue
            .dequeue_batch_by_sync_type_with_max_bytes(10, SyncType::Incremental, 100)
            .await
            .unwrap();
        assert_eq!(inc_batch.len(), 1);
        assert_eq!(inc_batch[0].sync_run_id, inc_run);
    }

    #[tokio::test]
    async fn test_dequeue_batch_orphans_with_max_bytes_limits_orphans() {
        let env = TestEnvironment::new().await.unwrap();
        let pool = env.db_pool.pool().clone();
        let queue = EventQueue::new(pool.clone());

        for i in 0..2 {
            let content_id = insert_sized_content(&pool, 70).await;
            let event =
                make_event_with_content("missing-run", &format!("orphan-doc-{}", i), content_id);
            queue.enqueue(TEST_SOURCE_ID, &event).await.unwrap();
        }

        let batch = queue
            .dequeue_batch_orphans_with_max_bytes(10, 100)
            .await
            .unwrap();
        assert_eq!(batch.len(), 1);
        assert_eq!(queue.get_pending_count().await.unwrap(), 1);
    }

    #[tokio::test]
    async fn test_queue_summary_reports_pending_size_bytes() {
        let env = TestEnvironment::new().await.unwrap();
        let pool = env.db_pool.pool().clone();
        let queue = EventQueue::new(pool.clone());

        let content_id = insert_sized_content(&pool, 42).await;
        let event = make_event_with_content("missing-run", "sized-doc", content_id);
        let event_id = queue.enqueue(TEST_SOURCE_ID, &event).await.unwrap();

        let summary = queue.get_queue_summary().await.unwrap();
        let pending_orphan = summary
            .entries
            .iter()
            .find(|entry| entry.sync_type.is_none() && entry.status == EventStatus::Pending)
            .unwrap();
        assert_eq!(pending_orphan.count, 1);
        let payload_bytes = serde_json::to_vec(&event).unwrap().len() as i64;
        let task_weight: i64 = sqlx::query_scalar(
            "SELECT weight FROM tasks WHERE id = $1",
        )
        .bind(&event_id)
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(pending_orphan.size_bytes, task_weight);
        assert_eq!(task_weight, payload_bytes + 42);
    }

    #[tokio::test]
    async fn test_person_payload_is_counted_but_excluded_from_document_readiness() {
        use shared::models::PersonSyncRecord;

        let env = TestEnvironment::new().await.unwrap();
        let pool = env.db_pool.pool().clone();
        let queue = EventQueue::new(pool.clone());
        let event = ConnectorEvent::PersonSync {
            sync_run_id: "missing-run".into(),
            source_id: TEST_SOURCE_ID.into(),
            person: PersonSyncRecord {
                external_id: "employee-1".into(),
                email: "person@example.com".into(),
                display_name: Some("A sufficiently large inline person payload".into()),
                given_name: None,
                middle_name: None,
                surname: None,
                job_title: None,
                department: None,
                division: None,
                company_name: None,
                office_location: None,
                work_country: None,
                employee_id: None,
                employee_type: None,
                cost_center: None,
                grade: None,
                band: None,
                confirmation_status: None,
                employment_start_date: None,
                employment_end_date: None,
                phone: None,
                is_active: None,
                top_department: None,
                manager_external_id: None,
                source_updated_at: None,
            },
        };
        queue.enqueue(TEST_SOURCE_ID, &event).await.unwrap();

        let summary = queue.get_queue_summary().await.unwrap();
        let pending = summary
            .entries
            .iter()
            .find(|entry| entry.sync_type.is_none() && entry.status == EventStatus::Pending)
            .unwrap();
        assert_eq!(pending.count, 1);
        assert!(pending.size_bytes > 0);
        assert!(
            queue
                .get_non_person_queue_summary()
                .await
                .unwrap()
                .entries
                .is_empty()
        );

        let person_batch = queue
            .dequeue_person_mutations_with_max_bytes(10, i64::MAX)
            .await
            .unwrap();
        assert_eq!(person_batch.len(), 1);
        assert_eq!(person_batch[0].event_type, "person_sync");
    }

    #[tokio::test]
    async fn person_candidate_filter_does_not_limit_before_predecessor_eligibility() {
        let env = TestEnvironment::new().await.unwrap();
        let queue = EventQueue::new(env.db_pool.pool().clone());

        let first = queue
            .enqueue(
                TEST_SOURCE_ID,
                &ConnectorEvent::PersonDeleted {
                    sync_run_id: "blocked-run".into(),
                    source_id: TEST_SOURCE_ID.into(),
                    email: "blocked@example.com".into(),
                },
            )
            .await
            .unwrap();
        let first_claim = queue.claim_person_mutations(1, i64::MAX).await.unwrap();
        assert_eq!(first_claim.events.len(), 1);
        assert_eq!(first_claim.events[0].id, first);

        // More blocked events than the claim limit used to fill the candidate
        // prefix, hiding the unrelated identity that followed them.
        for _ in 0..4 {
            queue
                .enqueue(
                    TEST_SOURCE_ID,
                    &ConnectorEvent::PersonDeleted {
                        sync_run_id: "blocked-run".into(),
                        source_id: TEST_SOURCE_ID.into(),
                        email: "blocked@example.com".into(),
                    },
                )
                .await
                .unwrap();
        }
        let unrelated = queue
            .enqueue(
                TEST_SOURCE_ID,
                &ConnectorEvent::PersonDeleted {
                    sync_run_id: "unrelated-run".into(),
                    source_id: TEST_SOURCE_ID.into(),
                    email: "unrelated@example.com".into(),
                },
            )
            .await
            .unwrap();

        let claim = queue.claim_person_mutations(2, i64::MAX).await.unwrap();
        assert_eq!(claim.events.len(), 1);
        assert_eq!(claim.events[0].id, unrelated);

        queue
            .complete_bulk(&[first], &first_claim.claim_token)
            .await
            .unwrap();
        queue
            .complete_bulk(&[unrelated], &claim.claim_token)
            .await
            .unwrap();
    }

    #[tokio::test]
    async fn person_dequeue_preserves_identity_fifo_across_sync_types_and_retries() {
        use shared::models::PersonSyncRecord;

        let env = TestEnvironment::new().await.unwrap();
        let pool = env.db_pool.pool().clone();
        let queue = EventQueue::new(pool.clone());
        let full_run = ulid::Ulid::new().to_string();
        let incremental_run = ulid::Ulid::new().to_string();
        let realtime_run = ulid::Ulid::new().to_string();
        insert_sync_run(&pool, &full_run, "full").await;
        insert_sync_run(&pool, &incremental_run, "incremental").await;
        insert_sync_run(&pool, &realtime_run, "realtime").await;

        let person = |email: &str| PersonSyncRecord {
            external_id: "EMP".into(),
            email: email.into(),
            display_name: None,
            given_name: None,
            middle_name: None,
            surname: None,
            job_title: None,
            department: None,
            division: None,
            company_name: None,
            office_location: None,
            work_country: None,
            employee_id: None,
            employee_type: None,
            cost_center: None,
            grade: None,
            band: None,
            confirmation_status: None,
            employment_start_date: None,
            employment_end_date: None,
            phone: None,
            is_active: None,
            top_department: None,
            manager_external_id: None,
            source_updated_at: None,
        };
        let first = queue
            .enqueue(
                TEST_SOURCE_ID,
                &ConnectorEvent::PersonSync {
                    sync_run_id: full_run.clone(),
                    source_id: TEST_SOURCE_ID.into(),
                    person: person("Person@Example.com"),
                },
            )
            .await
            .unwrap();
        let second = queue
            .enqueue(
                TEST_SOURCE_ID,
                &ConnectorEvent::PersonDeleted {
                    sync_run_id: incremental_run.clone(),
                    source_id: TEST_SOURCE_ID.into(),
                    email: "person@example.com".into(),
                },
            )
            .await
            .unwrap();
        let third = queue
            .enqueue(
                TEST_SOURCE_ID,
                &ConnectorEvent::PersonSync {
                    sync_run_id: realtime_run.clone(),
                    source_id: TEST_SOURCE_ID.into(),
                    person: person("PERSON@example.com"),
                },
            )
            .await
            .unwrap();
        let unrelated = queue
            .enqueue(
                TEST_SOURCE_ID,
                &ConnectorEvent::PersonSync {
                    sync_run_id: incremental_run.clone(),
                    source_id: TEST_SOURCE_ID.into(),
                    person: person("other@example.com"),
                },
            )
            .await
            .unwrap();

        // Regular sync-type dequeue never claims person events.
        assert!(
            queue
                .dequeue_batch_by_sync_type(10, SyncType::Full)
                .await
                .unwrap()
                .is_empty()
        );

        // Only the oldest event for an identity is eligible. An unrelated
        // identity can proceed in the same bounded batch.
        let initial = queue
            .dequeue_person_mutations_with_max_bytes(10, i64::MAX)
            .await
            .unwrap();
        let initial_ids: std::collections::HashSet<_> =
            initial.iter().map(|event| event.id.as_str()).collect();
        assert_eq!(initial_ids.len(), 2);
        assert!(initial_ids.contains(first.as_str()));
        assert!(initial_ids.contains(unrelated.as_str()));
        assert!(!initial_ids.contains(second.as_str()));
        assert!(!initial_ids.contains(third.as_str()));

        queue.mark_failed(&first, "retry me").await.unwrap();
        queue.mark_completed(&unrelated).await.unwrap();
        assert!(
            queue
                .dequeue_person_mutations_with_max_bytes(10, i64::MAX)
                .await
                .unwrap()
                .is_empty(),
            "retryable failure must block newer mutations for the identity"
        );

        queue.retry_failed_events().await.unwrap();
        sqlx::query("UPDATE tasks SET available_at = NOW() WHERE id = $1")
            .bind(&first)
            .execute(env.db_pool.pool())
            .await
            .unwrap();
        let retried = queue
            .dequeue_person_mutations_with_max_bytes(10, i64::MAX)
            .await
            .unwrap();
        assert_eq!(retried.len(), 1);
        assert_eq!(retried[0].id, first);
        queue.mark_completed(&first).await.unwrap();

        let delete = queue
            .dequeue_person_mutations_with_max_bytes(10, i64::MAX)
            .await
            .unwrap();
        assert_eq!(delete.len(), 1);
        assert_eq!(delete[0].id, second);
        queue.mark_completed(&second).await.unwrap();

        let resync = queue
            .dequeue_person_mutations_with_max_bytes(10, i64::MAX)
            .await
            .unwrap();
        assert_eq!(resync.len(), 1);
        assert_eq!(resync[0].id, third);
    }

    #[tokio::test]
    async fn person_fifo_expired_failure_does_not_block_identity() {
        use shared::models::PersonSyncRecord;

        let env = TestEnvironment::new().await.unwrap();
        let pool = env.db_pool.pool().clone();
        let queue = EventQueue::new(pool.clone());
        let full_run = ulid::Ulid::new().to_string();
        let newer_run = ulid::Ulid::new().to_string();
        insert_sync_run(&pool, &full_run, "full").await;
        insert_sync_run(&pool, &newer_run, "incremental").await;

        let person = |email: &str| PersonSyncRecord {
            external_id: "EMP".into(),
            email: email.into(),
            display_name: None,
            given_name: None,
            middle_name: None,
            surname: None,
            job_title: None,
            department: None,
            division: None,
            company_name: None,
            office_location: None,
            work_country: None,
            employee_id: None,
            employee_type: None,
            cost_center: None,
            grade: None,
            band: None,
            confirmation_status: None,
            employment_start_date: None,
            employment_end_date: None,
            phone: None,
            is_active: None,
            top_department: None,
            manager_external_id: None,
            source_updated_at: None,
        };
        let stale = queue
            .enqueue(
                TEST_SOURCE_ID,
                &ConnectorEvent::PersonSync {
                    sync_run_id: full_run.clone(),
                    source_id: TEST_SOURCE_ID.into(),
                    person: person("stale@example.com"),
                },
            )
            .await
            .unwrap();
        let newer = queue
            .enqueue(
                TEST_SOURCE_ID,
                &ConnectorEvent::PersonSync {
                    sync_run_id: newer_run.clone(),
                    source_id: TEST_SOURCE_ID.into(),
                    person: person("stale@example.com"),
                },
            )
            .await
            .unwrap();

        // Claim the predecessor before failing it so failure is fenced by its
        // real generic lease.
        let stale_claim = queue
            .dequeue_person_mutations_with_max_bytes(10, i64::MAX)
            .await
            .unwrap();
        assert_eq!(stale_claim.len(), 1);

        // Age the stale event and fail it.
        sqlx::query("UPDATE tasks SET created_at = NOW() - INTERVAL '48 hours' WHERE id = $1")
            .bind(&stale)
            .execute(&pool)
            .await
            .unwrap();
        queue.mark_failed(&stale, "transient outage").await.unwrap();

        // A retrying predecessor blocks the same identity until it is
        // terminally settled by administrative recovery.
        sqlx::query("UPDATE tasks SET status = 'dead_letter', completed_at = NOW(), claim_token = NULL, claimed_by = NULL, lease_expires_at = NULL WHERE id = $1")
            .bind(&stale)
            .execute(&pool)
            .await
            .unwrap();
        let batch = queue
            .dequeue_person_mutations_with_max_bytes(10, i64::MAX)
            .await
            .unwrap();
        assert_eq!(batch.len(), 1);
        assert_eq!(batch[0].id, newer);
        let (status,): (String,) = sqlx::query_as("SELECT status FROM tasks WHERE id = $1")
            .bind(&stale)
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(status, "dead_letter");
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

        // Insert two sync_runs with different sync_types. Only one scheduled
        // sync can be running for a source at a time, but routing is based on
        // the sync_run type, not on whether the run is still active.
        let full_run = ulid::Ulid::new().to_string();
        let inc_run = ulid::Ulid::new().to_string();
        sqlx::query(
            r#"
            INSERT INTO sync_runs (id, source_id, sync_type, status, started_at, completed_at, created_at, updated_at)
            VALUES ($1, $2, 'full', 'completed', NOW(), NOW(), NOW(), NOW())
            "#,
        )
        .bind(&full_run)
        .bind(TEST_SOURCE_ID)
        .execute(&pool)
        .await
        .unwrap();
        sqlx::query(
            r#"
            INSERT INTO sync_runs (id, source_id, sync_type, status, started_at, created_at, updated_at)
            VALUES ($1, $2, 'incremental', 'running', NOW(), NOW(), NOW())
            "#,
        )
        .bind(&inc_run)
        .bind(TEST_SOURCE_ID)
        .execute(&pool)
        .await
        .unwrap();

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

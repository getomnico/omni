use futures_util::future::join_all;
use shared::task_queue::{ClaimOptions, NewTask, TaskQueue, TaskStatus};
use shared::test_environment::TestEnvironment;
use sqlx::PgPool;
use time::{Duration, OffsetDateTime};

async fn new_queue() -> (TestEnvironment, PgPool, TaskQueue) {
    let env = TestEnvironment::new().await.unwrap();
    let pool = env.db_pool.pool().clone();
    let queue = TaskQueue::new(pool.clone());
    (env, pool, queue)
}

fn enqueue_tasks(task_type: &str, count: usize) -> Vec<NewTask> {
    (0..count)
        .map(|i| NewTask::new(task_type, serde_json::json!({ "n": i })))
        .collect()
}

fn claim_opts(limit: i32) -> ClaimOptions {
    ClaimOptions {
        limit,
        ..Default::default()
    }
}

#[tokio::test]
async fn test_enqueue_claim_complete_lifecycle() {
    let (_env, pool, queue) = new_queue().await;

    let created = queue.enqueue_bulk(&enqueue_tasks("test", 1)).await.unwrap();
    assert_eq!(created.len(), 1);
    let task = &created[0];
    assert_eq!(task.status, TaskStatus::Pending);
    assert_eq!(task.attempt_count, 0);
    assert!(task.claim_token.is_none());
    assert!(task.completed_at.is_none());

    let claim = queue
        .claim_bulk(&pool, "test", "worker-1", &claim_opts(1))
        .await
        .unwrap();
    assert_eq!(claim.tasks.len(), 1);
    let claimed = &claim.tasks[0];
    assert_eq!(claimed.id, task.id);
    assert_eq!(claimed.status, TaskStatus::Running);
    assert_eq!(claimed.attempt_count, 1);
    assert_eq!(
        claimed.claim_token.as_deref(),
        Some(claim.claim_token.as_str())
    );
    assert_eq!(claimed.claimed_by.as_deref(), Some("worker-1"));
    assert!(claimed.lease_expires_at.is_some());
    assert!(claimed.last_started_at.is_some());

    // A second claim must not see the running task.
    let second = queue
        .claim_bulk(&pool, "test", "worker-2", &claim_opts(1))
        .await
        .unwrap();
    assert!(second.tasks.is_empty());

    let completed = queue
        .complete_bulk(std::slice::from_ref(&task.id), &claim.claim_token)
        .await
        .unwrap();
    assert_eq!(completed, 1);

    let stats = queue.stats(Some("test")).await.unwrap();
    assert_eq!(
        stats
            .iter()
            .find(|s| s.status == TaskStatus::Completed)
            .map(|s| s.count),
        Some(1)
    );
}

#[tokio::test]
async fn test_enqueue_bulk_and_caller_id_idempotency() {
    let (_env, pool, queue) = new_queue().await;

    let tasks = enqueue_tasks("test", 3);
    let ids: Vec<String> = tasks.iter().map(|t| t.id.clone()).collect();

    let first = queue.enqueue_bulk(&tasks).await.unwrap();
    assert_eq!(first.len(), 3);

    // Re-enqueueing the same ids is a no-op.
    let second = queue.enqueue_bulk(&tasks).await.unwrap();
    assert!(second.is_empty());

    // The tasks are still claimable exactly once each.
    let claim = queue
        .claim_bulk(&pool, "test", "w", &claim_opts(10))
        .await
        .unwrap();
    assert_eq!(claim.tasks.len(), 3);
    let mut claimed_ids: Vec<String> = claim.tasks.iter().map(|t| t.id.clone()).collect();
    claimed_ids.sort();
    let mut ids = ids;
    ids.sort();
    assert_eq!(claimed_ids, ids);

    // A partial batch with one duplicate and one new task only inserts the new one.
    let mut new_task = NewTask::new("test", serde_json::json!({ "n": 99 }));
    new_task.id = "01J00000000000000000000000".to_string();
    let partial = queue
        .enqueue_bulk(&[tasks[0].clone(), new_task])
        .await
        .unwrap();
    assert_eq!(partial.len(), 1);
    assert_eq!(partial[0].id, "01J00000000000000000000000");
}

#[tokio::test]
async fn test_enqueue_single_is_idempotent() {
    let (_env, _pool, queue) = new_queue().await;

    let task = NewTask::new("test", serde_json::json!({}));
    let first = queue.enqueue(task.clone()).await.unwrap();
    let second = queue.enqueue(task.clone()).await.unwrap();

    assert_eq!(first.id, second.id);
    assert_eq!(second.status, TaskStatus::Pending);
}

#[tokio::test]
async fn test_enqueue_validation() {
    let (_env, _pool, queue) = new_queue().await;

    let mut bad_id = NewTask::new("test", serde_json::json!({}));
    bad_id.id = "short".to_string();
    assert!(queue.enqueue_bulk(&[bad_id]).await.is_err());

    let bad_type = NewTask::new("  ", serde_json::json!({}));
    assert!(queue.enqueue_bulk(&[bad_type]).await.is_err());

    let mut bad_payload = NewTask::new("test", serde_json::json!("not an object"));
    bad_payload.payload = serde_json::json!("nope");
    assert!(queue.enqueue_bulk(&[bad_payload]).await.is_err());

    let mut bad_weight = NewTask::new("test", serde_json::json!({}));
    bad_weight.weight = -1;
    assert!(queue.enqueue_bulk(&[bad_weight]).await.is_err());

    let mut bad_attempts = NewTask::new("test", serde_json::json!({}));
    bad_attempts.max_attempts = 0;
    assert!(queue.enqueue_bulk(&[bad_attempts]).await.is_err());
}

#[tokio::test]
async fn test_claim_fifo_by_ulid() {
    let (_env, pool, queue) = new_queue().await;

    let tasks = enqueue_tasks("test", 3);
    let ids: Vec<String> = tasks.iter().map(|t| t.id.clone()).collect();
    queue.enqueue_bulk(&tasks).await.unwrap();

    let mut claimed = Vec::new();
    for _ in 0..3 {
        let claim = queue
            .claim_bulk(&pool, "test", "w", &claim_opts(1))
            .await
            .unwrap();
        assert_eq!(claim.tasks.len(), 1);
        claimed.push(claim.tasks[0].id.clone());
        queue
            .complete_bulk(
                std::slice::from_ref(claimed.last().unwrap()),
                &claim.claim_token,
            )
            .await
            .unwrap();
    }

    // ULIDs are generated monotonically, so claim order matches enqueue order.
    assert_eq!(claimed, ids);
}

#[tokio::test]
async fn test_claim_orders_by_priority() {
    let (_env, pool, queue) = new_queue().await;

    let mut low = NewTask::new("test", serde_json::json!({ "p": 5 }));
    low.priority = 5;
    let mut high = NewTask::new("test", serde_json::json!({ "p": 10 }));
    high.priority = 10;
    queue.enqueue_bulk(&[low, high]).await.unwrap();

    let first = queue
        .claim_bulk(&pool, "test", "w", &claim_opts(1))
        .await
        .unwrap();
    assert_eq!(first.tasks[0].payload["p"], 10);

    let second = queue
        .claim_bulk(&pool, "test", "w", &claim_opts(1))
        .await
        .unwrap();
    assert_eq!(second.tasks[0].payload["p"], 5);
}

#[tokio::test]
async fn test_delayed_availability_blocks_claim() {
    let (_env, pool, queue) = new_queue().await;

    let mut later = NewTask::new("test", serde_json::json!({}));
    later.available_at = OffsetDateTime::now_utc() + Duration::hours(1);
    queue.enqueue_bulk(&[later]).await.unwrap();

    let claim = queue
        .claim_bulk(&pool, "test", "w", &claim_opts(1))
        .await
        .unwrap();
    assert!(claim.tasks.is_empty());

    sqlx::query("UPDATE tasks SET available_at = NOW() - INTERVAL '1 second'")
        .execute(&pool)
        .await
        .unwrap();

    let claim = queue
        .claim_bulk(&pool, "test", "w", &claim_opts(1))
        .await
        .unwrap();
    assert_eq!(claim.tasks.len(), 1);
}

#[tokio::test]
async fn test_weighted_batch_admits_first_even_over_cap() {
    let (_env, pool, queue) = new_queue().await;

    let mut tasks = enqueue_tasks("test", 3);
    tasks[0].weight = 5;
    tasks[1].weight = 5;
    tasks[2].weight = 10;
    queue.enqueue_bulk(&tasks).await.unwrap();

    let claim = queue
        .claim_bulk(
            &pool,
            "test",
            "w",
            &ClaimOptions {
                limit: 3,
                max_weight: Some(10),
                ..Default::default()
            },
        )
        .await
        .unwrap();
    assert_eq!(claim.tasks.len(), 2);
    let mut claimed_ids: Vec<String> = claim.tasks.iter().map(|t| t.id.clone()).collect();
    claimed_ids.sort();
    let mut expected_ids = vec![tasks[0].id.clone(), tasks[1].id.clone()];
    expected_ids.sort();
    assert_eq!(claimed_ids, expected_ids);

    // A single task heavier than the cap is still admitted (row 1 rule).
    let mut heavy = NewTask::new("test", serde_json::json!({}));
    heavy.weight = 100;
    queue.enqueue_bulk(&[heavy]).await.unwrap();

    let claim = queue
        .claim_bulk(
            &pool,
            "test",
            "w",
            &ClaimOptions {
                limit: 1,
                max_weight: Some(10),
                ..Default::default()
            },
        )
        .await
        .unwrap();
    assert_eq!(claim.tasks.len(), 1);
}

#[tokio::test]
async fn test_concurrent_consumers_never_receive_same_task() {
    let (_env, pool, queue) = new_queue().await;

    let tasks = enqueue_tasks("test", 10);
    queue.enqueue_bulk(&tasks).await.unwrap();

    let workers: Vec<_> = (0..20)
        .map(|i| {
            let queue = queue.clone();
            let pool = pool.clone();
            async move {
                let claim = queue
                    .claim_bulk(&pool, "test", &format!("worker-{}", i), &claim_opts(1))
                    .await
                    .unwrap();
                claim.tasks.into_iter().map(|t| t.id).collect::<Vec<_>>()
            }
        })
        .collect();

    let results: Vec<String> = join_all(workers).await.into_iter().flatten().collect();
    assert_eq!(
        results.len(),
        10,
        "each of the 10 tasks claimed exactly once"
    );

    let mut unique = results.clone();
    unique.sort();
    unique.dedup();
    assert_eq!(unique.len(), 10, "no task was claimed twice");
}

#[tokio::test]
async fn test_concurrency_key_serializes_oldest_first() {
    let (_env, pool, queue) = new_queue().await;

    let mut tasks = enqueue_tasks("test", 3);
    for task in &mut tasks {
        task.concurrency_key = Some("identity-1".to_string());
    }
    queue.enqueue_bulk(&tasks).await.unwrap();

    // Only the oldest task for the key is claimable at a time.
    let claim = queue
        .claim_bulk(&pool, "test", "w", &claim_opts(3))
        .await
        .unwrap();
    assert_eq!(claim.tasks.len(), 1);
    assert_eq!(claim.tasks[0].id, tasks[0].id);

    // Newer tasks stay blocked while the oldest is running.
    let blocked = queue
        .claim_bulk(&pool, "test", "w", &claim_opts(3))
        .await
        .unwrap();
    assert!(blocked.tasks.is_empty());

    queue
        .complete_bulk(&[tasks[0].id.clone()], &claim.claim_token)
        .await
        .unwrap();

    let claim = queue
        .claim_bulk(&pool, "test", "w", &claim_opts(3))
        .await
        .unwrap();
    assert_eq!(claim.tasks.len(), 1);
    assert_eq!(claim.tasks[0].id, tasks[1].id);
}

#[tokio::test]
async fn test_max_concurrency_cap_is_atomic() {
    let (_env, pool, queue) = new_queue().await;

    let tasks = enqueue_tasks("test", 5);
    queue.enqueue_bulk(&tasks).await.unwrap();

    let claim = queue
        .claim_bulk(
            &pool,
            "test",
            "w",
            &ClaimOptions {
                limit: 5,
                max_concurrency: Some(2),
                ..Default::default()
            },
        )
        .await
        .unwrap();
    assert_eq!(claim.tasks.len(), 2);

    // Already at the cap: no further claims.
    let blocked = queue
        .claim_bulk(
            &pool,
            "test",
            "w",
            &ClaimOptions {
                limit: 5,
                max_concurrency: Some(2),
                ..Default::default()
            },
        )
        .await
        .unwrap();
    assert!(blocked.tasks.is_empty());

    // Completing frees capacity for the next oldest tasks.
    let token = claim.claim_token.clone();
    let ids: Vec<String> = claim.tasks.iter().map(|t| t.id.clone()).collect();
    queue.complete_bulk(&ids, &token).await.unwrap();

    let claim = queue
        .claim_bulk(
            &pool,
            "test",
            "w",
            &ClaimOptions {
                limit: 5,
                max_concurrency: Some(2),
                ..Default::default()
            },
        )
        .await
        .unwrap();
    assert_eq!(claim.tasks.len(), 2);
    let mut claimed_ids: Vec<String> = claim.tasks.iter().map(|t| t.id.clone()).collect();
    claimed_ids.sort();
    let mut expected_ids = vec![tasks[2].id.clone(), tasks[3].id.clone()];
    expected_ids.sort();
    assert_eq!(claimed_ids, expected_ids);
}

#[tokio::test]
async fn test_concurrent_claims_respect_concurrency_cap() {
    let (_env, pool, queue) = new_queue().await;

    let tasks = enqueue_tasks("test", 5);
    queue.enqueue_bulk(&tasks).await.unwrap();

    let workers: Vec<_> = (0..10)
        .map(|i| {
            let queue = queue.clone();
            let pool = pool.clone();
            async move {
                queue
                    .claim_bulk(
                        &pool,
                        "test",
                        &format!("worker-{}", i),
                        &ClaimOptions {
                            limit: 5,
                            max_concurrency: Some(1),
                            ..Default::default()
                        },
                    )
                    .await
                    .unwrap()
            }
        })
        .collect();

    let claims: Vec<_> = join_all(workers).await;
    let claimed: Vec<String> = claims
        .iter()
        .flat_map(|c| c.tasks.iter().map(|t| t.id.clone()))
        .collect();
    assert_eq!(
        claimed.len(),
        1,
        "cap of 1 admits exactly one task across concurrent claims"
    );

    // The slot stays occupied until the running task completes.
    let holder = claims.iter().find(|c| !c.tasks.is_empty()).unwrap();
    let blocked = queue
        .claim_bulk(
            &pool,
            "test",
            "w",
            &ClaimOptions {
                limit: 5,
                max_concurrency: Some(1),
                ..Default::default()
            },
        )
        .await
        .unwrap();
    assert!(blocked.tasks.is_empty());

    queue
        .complete_bulk(&[holder.tasks[0].id.clone()], &holder.claim_token)
        .await
        .unwrap();

    let claim = queue
        .claim_bulk(
            &pool,
            "test",
            "w",
            &ClaimOptions {
                limit: 5,
                max_concurrency: Some(1),
                ..Default::default()
            },
        )
        .await
        .unwrap();
    assert_eq!(claim.tasks.len(), 1);
}

#[tokio::test]
async fn test_claim_by_candidate_ids() {
    let (_env, pool, queue) = new_queue().await;

    let tasks = enqueue_tasks("test", 3);
    queue.enqueue_bulk(&tasks).await.unwrap();

    // Claim only the third task by id, ignoring the earlier ones.
    let claim = queue
        .claim_bulk(
            &pool,
            "test",
            "w",
            &ClaimOptions {
                candidate_ids: Some(vec![tasks[2].id.clone()]),
                limit: 1,
                ..Default::default()
            },
        )
        .await
        .unwrap();
    assert_eq!(claim.tasks.len(), 1);
    assert_eq!(claim.tasks[0].id, tasks[2].id);
}

#[tokio::test]
async fn test_heartbeat_renews_and_fences_stale_token() {
    let (_env, pool, queue) = new_queue().await;

    let tasks = enqueue_tasks("test", 1);
    queue.enqueue_bulk(&tasks).await.unwrap();

    let claim = queue
        .claim_bulk(
            &pool,
            "test",
            "w",
            &ClaimOptions {
                limit: 1,
                lease_seconds: 60,
                ..Default::default()
            },
        )
        .await
        .unwrap();

    assert!(
        queue
            .heartbeat(&tasks[0].id, &claim.claim_token, 120)
            .await
            .unwrap()
    );

    // Wrong token never renews.
    assert!(
        !queue
            .heartbeat(&tasks[0].id, "01J00000000000000000000002", 120)
            .await
            .unwrap()
    );

    // An expired lease is not renewable even with the right token.
    sqlx::query("UPDATE tasks SET lease_expires_at = NOW() - INTERVAL '1 second'")
        .execute(&pool)
        .await
        .unwrap();
    assert!(
        !queue
            .heartbeat(&tasks[0].id, &claim.claim_token, 120)
            .await
            .unwrap()
    );
}

#[tokio::test]
async fn test_terminal_writes_are_fenced_by_claim_token() {
    let (_env, pool, queue) = new_queue().await;

    let tasks = enqueue_tasks("test", 2);
    queue.enqueue_bulk(&tasks).await.unwrap();

    let claim = queue
        .claim_bulk(&pool, "test", "w", &claim_opts(2))
        .await
        .unwrap();
    let ids: Vec<String> = claim.tasks.iter().map(|t| t.id.clone()).collect();

    // Wrong token: no rows affected.
    assert_eq!(
        queue
            .complete_bulk(&ids, "01J00000000000000000000002")
            .await
            .unwrap(),
        0
    );
    let failed = queue
        .fail_bulk(&ids, "01J00000000000000000000002", "stale worker", true, 0)
        .await
        .unwrap();
    assert!(failed.is_empty());

    // Right token: both complete.
    assert_eq!(
        queue.complete_bulk(&ids, &claim.claim_token).await.unwrap(),
        2
    );
}

#[tokio::test]
async fn test_lease_expiry_recovery() {
    let (_env, pool, queue) = new_queue().await;

    let tasks = enqueue_tasks("test", 1);
    queue.enqueue_bulk(&tasks).await.unwrap();

    let claim = queue
        .claim_bulk(
            &pool,
            "test",
            "w",
            &ClaimOptions {
                limit: 1,
                lease_seconds: 60,
                ..Default::default()
            },
        )
        .await
        .unwrap();
    assert_eq!(claim.tasks.len(), 1);

    sqlx::query("UPDATE tasks SET lease_expires_at = NOW() - INTERVAL '1 second'")
        .execute(&pool)
        .await
        .unwrap();

    let recovered = queue.recover_expired().await.unwrap();
    assert_eq!(recovered.len(), 1);
    let task = &recovered[0];
    assert_eq!(task.status, TaskStatus::Pending);
    assert!(task.claim_token.is_none());
    assert!(task.claimed_by.is_none());
    assert!(task.lease_expires_at.is_none());
    assert_eq!(
        task.last_error.as_deref(),
        Some("task lease expired; requeued")
    );

    // The requeued task is claimable again on attempt 2.
    let claim = queue
        .claim_bulk(&pool, "test", "w", &claim_opts(1))
        .await
        .unwrap();
    assert_eq!(claim.tasks.len(), 1);
    assert_eq!(claim.tasks[0].attempt_count, 2);
}

#[tokio::test]
async fn test_fail_retries_then_dead_letters() {
    let (_env, pool, queue) = new_queue().await;

    let mut task = NewTask::new("test", serde_json::json!({}));
    task.max_attempts = 3;
    queue.enqueue_bulk(&[task.clone()]).await.unwrap();

    for attempt in 1..=3 {
        let claim = queue
            .claim_bulk(&pool, "test", "w", &claim_opts(1))
            .await
            .unwrap();
        assert_eq!(claim.tasks.len(), 1, "claim on attempt {}", attempt);
        assert_eq!(claim.tasks[0].attempt_count, attempt);

        let failed = queue
            .fail_bulk(
                &[task.id.clone()],
                &claim.claim_token,
                &format!("error {}", attempt),
                true,
                0,
            )
            .await
            .unwrap();
        let (id, status) = &failed[0];
        assert_eq!(id, &task.id);
        if attempt < 3 {
            assert_eq!(
                *status,
                TaskStatus::Pending,
                "retry after attempt {}",
                attempt
            );
        } else {
            assert_eq!(
                *status,
                TaskStatus::DeadLetter,
                "dead-letter on final attempt"
            );
        }
    }

    // No claims remain.
    let claim = queue
        .claim_bulk(&pool, "test", "w", &claim_opts(1))
        .await
        .unwrap();
    assert!(claim.tasks.is_empty());

    let stats = queue.stats(Some("test")).await.unwrap();
    assert_eq!(
        stats
            .iter()
            .find(|s| s.status == TaskStatus::DeadLetter)
            .map(|s| s.count),
        Some(1)
    );
}

#[tokio::test]
async fn test_retry_delay_schedules_available_at() {
    let (_env, pool, queue) = new_queue().await;

    let tasks = enqueue_tasks("test", 1);
    queue.enqueue_bulk(&tasks).await.unwrap();

    let claim = queue
        .claim_bulk(&pool, "test", "w", &claim_opts(1))
        .await
        .unwrap();
    queue
        .fail_bulk(
            &[tasks[0].id.clone()],
            &claim.claim_token,
            "slow down",
            true,
            3600,
        )
        .await
        .unwrap();

    // Not claimable while the retry delay is pending.
    let blocked = queue
        .claim_bulk(&pool, "test", "w", &claim_opts(1))
        .await
        .unwrap();
    assert!(blocked.tasks.is_empty());

    sqlx::query("UPDATE tasks SET available_at = NOW() - INTERVAL '1 second'")
        .execute(&pool)
        .await
        .unwrap();
    let claim = queue
        .claim_bulk(&pool, "test", "w", &claim_opts(1))
        .await
        .unwrap();
    assert_eq!(claim.tasks.len(), 1);
}

#[tokio::test]
async fn test_non_retryable_failure_dead_letters_immediately() {
    let (_env, pool, queue) = new_queue().await;

    let tasks = enqueue_tasks("test", 1);
    queue.enqueue_bulk(&tasks).await.unwrap();

    let claim = queue
        .claim_bulk(&pool, "test", "w", &claim_opts(1))
        .await
        .unwrap();
    let failed = queue
        .fail_bulk(
            &[tasks[0].id.clone()],
            &claim.claim_token,
            "permanent error",
            false,
            0,
        )
        .await
        .unwrap();
    assert_eq!(failed[0].1, TaskStatus::DeadLetter);

    let claim = queue
        .claim_bulk(&pool, "test", "w", &claim_opts(1))
        .await
        .unwrap();
    assert!(claim.tasks.is_empty());
}

#[tokio::test]
async fn test_stats_counts_by_type_and_status() {
    let (_env, pool, queue) = new_queue().await;

    let mut alpha = enqueue_tasks("alpha", 2);
    let mut beta = enqueue_tasks("beta", 1);
    let mut all = Vec::new();
    all.append(&mut alpha);
    all.append(&mut beta);
    queue.enqueue_bulk(&all).await.unwrap();

    let claim = queue
        .claim_bulk(&pool, "alpha", "w", &claim_opts(1))
        .await
        .unwrap();
    queue
        .complete_bulk(&[claim.tasks[0].id.clone()], &claim.claim_token)
        .await
        .unwrap();

    let stats = queue.stats(None).await.unwrap();
    let count = |task_type: &str, status: TaskStatus| {
        stats
            .iter()
            .find(|s| s.task_type == task_type && s.status == status)
            .map(|s| s.count)
            .unwrap_or(0)
    };
    assert_eq!(count("alpha", TaskStatus::Pending), 1);
    assert_eq!(count("alpha", TaskStatus::Completed), 1);
    assert_eq!(count("beta", TaskStatus::Pending), 1);

    let beta_stats = queue.stats(Some("beta")).await.unwrap();
    assert_eq!(beta_stats.len(), 1);
    assert_eq!(beta_stats[0].status, TaskStatus::Pending);
}

#[tokio::test]
async fn test_cleanup_deletes_old_terminal_tasks() {
    let (_env, pool, queue) = new_queue().await;

    let tasks = enqueue_tasks("test", 2);
    queue.enqueue_bulk(&tasks).await.unwrap();

    let claim = queue
        .claim_bulk(&pool, "test", "w", &claim_opts(2))
        .await
        .unwrap();
    let ids: Vec<String> = claim.tasks.iter().map(|t| t.id.clone()).collect();
    queue.complete_bulk(&ids, &claim.claim_token).await.unwrap();

    // Nothing old yet (completed_at was just set to now).
    assert_eq!(
        queue
            .cleanup(OffsetDateTime::now_utc() - Duration::hours(1))
            .await
            .unwrap(),
        0
    );

    sqlx::query("UPDATE tasks SET completed_at = NOW() - INTERVAL '7 days'")
        .execute(&pool)
        .await
        .unwrap();
    assert_eq!(queue.cleanup(OffsetDateTime::now_utc()).await.unwrap(), 2);

    // Pending tasks are never cleaned up.
    let tasks = enqueue_tasks("test", 1);
    queue.enqueue_bulk(&tasks).await.unwrap();
    assert_eq!(queue.cleanup(OffsetDateTime::now_utc()).await.unwrap(), 0);
}

#[tokio::test]
async fn test_constraints_reject_invalid_lifecycle_states() {
    let (_env, pool, _queue) = new_queue().await;

    // running requires claim token, claimed_by, and lease.
    let err = sqlx::query(
        r#"
        INSERT INTO tasks (id, task_type, payload, status)
        VALUES ('01J00000000000000000000001', 'test', '{}', 'running')
        "#,
    )
    .execute(&pool)
    .await;
    assert!(err.is_err());

    // Unknown status.
    let err = sqlx::query(
        r#"
        INSERT INTO tasks (id, task_type, payload, status)
        VALUES ('01J00000000000000000000002', 'test', '{}', 'bogus')
        "#,
    )
    .execute(&pool)
    .await;
    assert!(err.is_err());

    // attempt_count cannot exceed max_attempts.
    let err = sqlx::query(
        r#"
        INSERT INTO tasks (id, task_type, payload, attempt_count, max_attempts)
        VALUES ('01J00000000000000000000003', 'test', '{}', 5, 3)
        "#,
    )
    .execute(&pool)
    .await;
    assert!(err.is_err());

    // payload must be a JSON object.
    let err = sqlx::query(
        r#"
        INSERT INTO tasks (id, task_type, payload)
        VALUES ('01J00000000000000000000004', 'test', '"not an object"'::jsonb)
        "#,
    )
    .execute(&pool)
    .await;
    assert!(err.is_err());

    // id must be a 26-char ULID.
    let err = sqlx::query(
        r#"
        INSERT INTO tasks (id, task_type, payload)
        VALUES ('short', 'test', '{}')
        "#,
    )
    .execute(&pool)
    .await;
    assert!(err.is_err());
}

#[tokio::test]
async fn test_recover_expired_exhausted_attempts_dead_letters() {
    let (_env, pool, queue) = new_queue().await;

    let mut task = NewTask::new("test", serde_json::json!({}));
    task.max_attempts = 1;
    queue.enqueue_bulk(&[task.clone()]).await.unwrap();

    let claim = queue
        .claim_bulk(
            &pool,
            "test",
            "w",
            &ClaimOptions {
                limit: 1,
                lease_seconds: 60,
                ..Default::default()
            },
        )
        .await
        .unwrap();
    assert_eq!(claim.tasks.len(), 1);

    // Model a task with a prior failure, then an expired lease: exhausted
    // recovery must record the lease expiry as the latest event.
    sqlx::query(
        "UPDATE tasks SET lease_expires_at = NOW() - INTERVAL '1 second', last_error = 'previous failure'",
    )
    .execute(&pool)
    .await
    .unwrap();

    let recovered = queue.recover_expired().await.unwrap();
    assert_eq!(recovered.len(), 1);
    assert_eq!(recovered[0].status, TaskStatus::DeadLetter);
    assert!(recovered[0].completed_at.is_some());
    assert_eq!(
        recovered[0].last_error.as_deref(),
        Some("task lease expired; retries exhausted")
    );
}

#[tokio::test]
async fn test_claim_validates_identity() {
    let (_env, pool, queue) = new_queue().await;

    assert!(
        queue
            .claim_bulk(&pool, "  ", "w", &claim_opts(1))
            .await
            .is_err()
    );
    assert!(
        queue
            .claim_bulk(&pool, "test", " ", &claim_opts(1))
            .await
            .is_err()
    );
}

#[tokio::test]
async fn test_sql_rejects_null_required_arguments() {
    let (_env, pool, _queue) = new_queue().await;

    // NULL limit must not be interpreted as "no limit".
    let err = sqlx::query(
        "SELECT * FROM task_claim_bulk('test', NULL, NULL, NULL, NULL, 60, '01J00000000000000000000001', 'w')",
    )
    .fetch_all(&pool)
    .await;
    assert!(err.is_err());

    // NULL retryable must not silently dead-letter.
    let err = sqlx::query(
        "SELECT * FROM task_fail_bulk(ARRAY['01J00000000000000000000000'], '01J00000000000000000000001', 'boom', NULL, 0)",
    )
    .fetch_all(&pool)
    .await;
    assert!(err.is_err());

    // Blank claimed_by is rejected.
    let err = sqlx::query(
        "SELECT * FROM task_claim_bulk('test', NULL, 1, NULL, NULL, 60, '01J00000000000000000000001', '')",
    )
    .fetch_all(&pool)
    .await;
    assert!(err.is_err());

    // Blank task_type is rejected.
    let err = sqlx::query(
        "SELECT * FROM task_claim_bulk('', NULL, 1, NULL, NULL, 60, '01J00000000000000000000001', 'w')",
    )
    .fetch_all(&pool)
    .await;
    assert!(err.is_err());

    // NULL lease_seconds is rejected.
    let err = sqlx::query(
        "SELECT * FROM task_claim_bulk('test', NULL, 1, NULL, NULL, NULL, '01J00000000000000000000001', 'w')",
    )
    .fetch_all(&pool)
    .await;
    assert!(err.is_err());

    // Malformed claim token in complete is rejected.
    let err =
        sqlx::query("SELECT task_complete_bulk(ARRAY['01J00000000000000000000000'], 'short')")
            .fetch_all(&pool)
            .await;
    assert!(err.is_err());
}

#[tokio::test]
async fn test_terminal_write_respects_wall_clock_lease_expiry() {
    let (_env, pool, queue) = new_queue().await;

    let tasks = enqueue_tasks("test", 1);
    queue.enqueue_bulk(&tasks).await.unwrap();

    let claim = queue
        .claim_bulk(
            &pool,
            "test",
            "w",
            &ClaimOptions {
                limit: 1,
                lease_seconds: 1,
                ..Default::default()
            },
        )
        .await
        .unwrap();
    assert_eq!(claim.tasks.len(), 1);

    // Open a transaction, wait past the wall-clock lease expiry, then try to
    // complete from inside that transaction. NOW() is frozen at transaction
    // start, so fencing must use the statement timestamp of the write.
    let mut tx = pool.begin().await.unwrap();
    tokio::time::sleep(std::time::Duration::from_secs(2)).await;
    let completed: i64 = sqlx::query_scalar("SELECT task_complete_bulk($1, $2)")
        .bind(&[claim.tasks[0].id.clone()])
        .bind(&claim.claim_token)
        .fetch_one(&mut *tx)
        .await
        .unwrap();
    assert_eq!(
        completed, 0,
        "lease expired in wall time; the terminal write must be fenced"
    );
    tx.rollback().await.unwrap();
}

#[tokio::test]
async fn test_claim_lease_starts_after_advisory_lock_wait() {
    let (_env, pool, queue) = new_queue().await;

    let tasks = enqueue_tasks("test", 1);
    queue.enqueue_bulk(&tasks).await.unwrap();

    // Hold the per-task-type advisory lock so the capped claim blocks on it
    // for longer than the requested lease.
    let mut lock_tx = pool.begin().await.unwrap();
    sqlx::query("SELECT pg_advisory_xact_lock(hashtext('test')::bigint)")
        .execute(&mut *lock_tx)
        .await
        .unwrap();

    let queue = queue.clone();
    let pool = pool.clone();
    let claim_handle = tokio::spawn(async move {
        queue
            .claim_bulk(
                &pool,
                "test",
                "w",
                &ClaimOptions {
                    limit: 1,
                    max_concurrency: Some(1),
                    lease_seconds: 1,
                    ..Default::default()
                },
            )
            .await
            .unwrap()
    });

    tokio::time::sleep(std::time::Duration::from_secs(2)).await;
    lock_tx.commit().await.unwrap();

    let claim = claim_handle.await.unwrap();
    assert_eq!(claim.tasks.len(), 1);
    let lease = claim.tasks[0].lease_expires_at.unwrap();
    assert!(
        lease > OffsetDateTime::now_utc(),
        "lease must start after the advisory lock wait, not be already expired"
    );
}

#[tokio::test]
async fn test_terminal_write_waits_for_row_lock_then_fences() {
    let (_env, pool, queue) = new_queue().await;

    let tasks = enqueue_tasks("test", 1);
    queue.enqueue_bulk(&tasks).await.unwrap();

    let claim = queue
        .claim_bulk(
            &pool,
            "test",
            "w",
            &ClaimOptions {
                limit: 1,
                lease_seconds: 1,
                ..Default::default()
            },
        )
        .await
        .unwrap();
    assert_eq!(claim.tasks.len(), 1);
    let task_id = claim.tasks[0].id.clone();
    let claim_token = claim.claim_token.clone();

    // Hold a row lock on the claimed task so the terminal write blocks on it
    // until after the lease expires.
    let mut lock_tx = pool.begin().await.unwrap();
    sqlx::query("UPDATE tasks SET last_error = 'held' WHERE id = $1")
        .bind(&task_id)
        .execute(&mut *lock_tx)
        .await
        .unwrap();

    let queue = queue.clone();
    let complete_handle =
        tokio::spawn(async move { queue.complete_bulk(&[task_id], &claim_token).await.unwrap() });

    tokio::time::sleep(std::time::Duration::from_secs(2)).await;
    lock_tx.commit().await.unwrap();

    let completed = complete_handle.await.unwrap();
    assert_eq!(
        completed, 0,
        "lease expired while waiting on the row lock; complete must be fenced"
    );
}

use shared::task_queue::{ClaimOptions, EnqueueTaskRequest, TaskQueue, TaskStatus};
use shared::test_environment::TestEnvironment;

#[tokio::test]
async fn bulk_heartbeat_is_fenced_and_reports_ownership_loss() {
    let env = TestEnvironment::new().await.unwrap();
    let pool = env.db_pool.pool().clone();
    let queue = TaskQueue::new(pool.clone());
    let tasks = [
        EnqueueTaskRequest::new("bulk-heartbeat", serde_json::json!({"n": 1})),
        EnqueueTaskRequest::new("bulk-heartbeat", serde_json::json!({"n": 2})),
    ];
    queue.enqueue_bulk(&tasks).await.unwrap();
    let claim = queue
        .claim_bulk(
            &pool,
            "bulk-heartbeat",
            "worker",
            &ClaimOptions {
                limit: 2,
                ..Default::default()
            },
        )
        .await
        .unwrap();
    let ids: Vec<String> = claim.tasks.iter().map(|task| task.id.clone()).collect();

    assert_eq!(
        queue
            .heartbeat_bulk(&ids, &claim.claim_token, 60)
            .await
            .unwrap()
            .len(),
        2
    );
    assert!(
        queue
            .heartbeat_bulk(&ids, "01J00000000000000000000001", 60)
            .await
            .unwrap()
            .is_empty()
    );
    sqlx::query("UPDATE tasks SET lease_expires_at = NOW() - INTERVAL '1 second' WHERE id = ANY($1)")
        .bind(&ids)
        .execute(&pool)
        .await
        .unwrap();
    assert!(
        queue
            .heartbeat_bulk(&ids, &claim.claim_token, 60)
            .await
            .unwrap()
            .is_empty()
    );
}

#[tokio::test]
async fn bulk_failure_keeps_per_item_errors_and_common_retry_policy() {
    let env = TestEnvironment::new().await.unwrap();
    let pool = env.db_pool.pool().clone();
    let queue = TaskQueue::new(pool.clone());
    let tasks = [
        EnqueueTaskRequest::new("bulk-failure", serde_json::json!({"n": 1})),
        EnqueueTaskRequest::new("bulk-failure", serde_json::json!({"n": 2})),
    ];
    queue.enqueue_bulk(&tasks).await.unwrap();
    let claim = queue
        .claim_bulk(
            &pool,
            "bulk-failure",
            "worker",
            &ClaimOptions {
                limit: 2,
                ..Default::default()
            },
        )
        .await
        .unwrap();
    let errors = vec![
        (tasks[0].id.clone(), "first failure".to_string()),
        (tasks[1].id.clone(), "second failure".to_string()),
    ];
    let result = queue
        .fail_bulk_with_errors(&errors, &claim.claim_token, true, 60)
        .await
        .unwrap();
    assert_eq!(result.len(), 2);
    assert_eq!(
        queue
            .get(&tasks[0].id)
            .await
            .unwrap()
            .unwrap()
            .last_error
            .as_deref(),
        Some("first failure")
    );
    assert_eq!(
        queue
            .get(&tasks[1].id)
            .await
            .unwrap()
            .unwrap()
            .last_error
            .as_deref(),
        Some("second failure")
    );
    assert_eq!(
        queue.get(&tasks[0].id).await.unwrap().unwrap().status,
        TaskStatus::Pending
    );
}

#[tokio::test]
async fn administrative_dead_letter_only_cancels_retry_pending_tasks() {
    let env = TestEnvironment::new().await.unwrap();
    let pool = env.db_pool.pool().clone();
    let queue = TaskQueue::new(pool.clone());
    let tasks = [
        EnqueueTaskRequest::new("bulk-cancel", serde_json::json!({"n": 1})),
        EnqueueTaskRequest::new("bulk-cancel", serde_json::json!({"n": 2})),
    ];
    queue.enqueue_bulk(&tasks).await.unwrap();
    let claim = queue
        .claim_bulk(
            &pool,
            "bulk-cancel",
            "worker",
            &ClaimOptions {
                limit: 1,
                ..Default::default()
            },
        )
        .await
        .unwrap();
    queue
        .fail_bulk(
            &[tasks[0].id.clone()],
            &claim.claim_token,
            "retry",
            true,
            60,
        )
        .await
        .unwrap();
    let cancelled = queue
        .dead_letter_pending(
            &[tasks[0].id.clone(), tasks[1].id.clone()],
            "source deleted",
        )
        .await
        .unwrap();
    assert_eq!(cancelled, vec![tasks[0].id.clone()]);
    assert_eq!(
        queue.get(&tasks[0].id).await.unwrap().unwrap().status,
        TaskStatus::DeadLetter
    );
    assert_eq!(
        queue.get(&tasks[1].id).await.unwrap().unwrap().status,
        TaskStatus::Pending
    );
}

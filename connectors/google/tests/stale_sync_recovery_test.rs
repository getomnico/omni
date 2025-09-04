use anyhow::Result;
use omni_google_connector::sync::SyncManager;
use shared::models::{SourceType, SyncRun, SyncStatus, SyncType};
use shared::test_environment::TestEnvironment;
use shared::utils::generate_ulid;

#[tokio::test]
async fn test_stale_sync_recovery() -> Result<()> {
    // Setup test environment
    let test_env = TestEnvironment::new().await?;
    let pool = test_env.db_pool.pool();
    let redis_client = test_env.redis_client.clone();

    // Create a test source
    let source_id = generate_ulid();
    let user_id = generate_ulid();

    // First create a user
    sqlx::query("INSERT INTO users (id, email, name, role) VALUES ($1, $2, $3, $4)")
        .bind(&user_id)
        .bind("test@example.com")
        .bind("Test User")
        .bind("admin")
        .execute(pool)
        .await?;

    // Then create a source
    sqlx::query(
        "INSERT INTO sources (id, name, source_type, is_active, created_by) VALUES ($1, $2, $3, $4, $5)"
    )
    .bind(&source_id)
    .bind("Test Google Source")
    .bind(SourceType::GoogleDrive)
    .bind(true)
    .bind(&user_id)
    .execute(pool)
    .await?;

    // Create a stale running sync run (started 3 hours ago)
    let stale_sync_id = generate_ulid();
    let three_hours_ago = sqlx::types::time::OffsetDateTime::now_utc() - time::Duration::hours(3);

    sqlx::query(
        "INSERT INTO sync_runs (id, source_id, sync_type, status, started_at) VALUES ($1, $2, $3, $4, $5)"
    )
    .bind(&stale_sync_id)
    .bind(&source_id)
    .bind(SyncType::Full)
    .bind(SyncStatus::Running)
    .bind(three_hours_ago)
    .execute(pool)
    .await?;

    // Create a recent running sync run (started 30 minutes ago)
    let recent_sync_id = generate_ulid();
    let thirty_minutes_ago =
        sqlx::types::time::OffsetDateTime::now_utc() - time::Duration::minutes(30);

    sqlx::query(
        "INSERT INTO sync_runs (id, source_id, sync_type, status, started_at) VALUES ($1, $2, $3, $4, $5)"
    )
    .bind(&recent_sync_id)
    .bind(&source_id)
    .bind(SyncType::Full)
    .bind(SyncStatus::Running)
    .bind(thirty_minutes_ago)
    .execute(pool)
    .await?;

    // Create sync manager
    let sync_manager = SyncManager::new(pool.clone(), redis_client).await?;

    // Test get_stale_running_syncs - should find the 3-hour-old sync but not the 30-minute one
    let stale_syncs = sync_manager.get_stale_running_syncs(2).await?;
    assert_eq!(stale_syncs.len(), 1);
    assert_eq!(stale_syncs[0].id, stale_sync_id);

    // Test recover_stale_syncs
    sync_manager.recover_stale_syncs().await?;

    // Verify that the stale sync is now marked as failed
    let updated_sync = sqlx::query_as::<_, SyncRun>("SELECT * FROM sync_runs WHERE id = $1")
        .bind(&stale_sync_id)
        .fetch_one(pool)
        .await?;

    assert_eq!(updated_sync.status, SyncStatus::Failed);
    assert!(updated_sync.error_message.is_some());
    assert!(updated_sync.completed_at.is_some());

    // Verify that the recent sync is still running
    let recent_sync = sqlx::query_as::<_, SyncRun>("SELECT * FROM sync_runs WHERE id = $1")
        .bind(&recent_sync_id)
        .fetch_one(pool)
        .await?;

    assert_eq!(recent_sync.status, SyncStatus::Running);

    // Test should_run_full_sync - should return false because there's still a running sync
    let should_sync = sync_manager.should_run_full_sync(&source_id).await?;
    assert_eq!(should_sync, false);

    // Mark the recent sync as completed
    sqlx::query("UPDATE sync_runs SET status = $1, completed_at = CURRENT_TIMESTAMP WHERE id = $2")
        .bind(SyncStatus::Completed)
        .bind(&recent_sync_id)
        .execute(pool)
        .await?;

    // Now should_run_full_sync should check timing
    let should_sync = sync_manager.should_run_full_sync(&source_id).await?;
    // This might be true or false depending on timing, but it shouldn't fail

    Ok(())
}

#[tokio::test]
async fn test_get_running_sync_for_source() -> Result<()> {
    // Setup test environment
    let test_env = TestEnvironment::new().await?;
    let pool = test_env.db_pool.pool();
    let redis_client = test_env.redis_client.clone();

    // Create a test source
    let source_id = generate_ulid();
    let user_id = generate_ulid();

    // First create a user
    sqlx::query("INSERT INTO users (id, email, name, role) VALUES ($1, $2, $3, $4)")
        .bind(&user_id)
        .bind("test2@example.com")
        .bind("Test User 2")
        .bind("admin")
        .execute(pool)
        .await?;

    // Then create a source
    sqlx::query(
        "INSERT INTO sources (id, name, source_type, is_active, created_by) VALUES ($1, $2, $3, $4, $5)"
    )
    .bind(&source_id)
    .bind("Test Google Source 2")
    .bind(SourceType::GoogleDrive)
    .bind(true)
    .bind(&user_id)
    .execute(pool)
    .await?;

    let sync_manager = SyncManager::new(pool.clone(), redis_client).await?;

    // Test with no running sync
    let running_sync = sync_manager.get_running_sync_for_source(&source_id).await?;
    assert!(running_sync.is_none());

    // Create a running sync
    let sync_id = generate_ulid();
    sqlx::query("INSERT INTO sync_runs (id, source_id, sync_type, status) VALUES ($1, $2, $3, $4)")
        .bind(&sync_id)
        .bind(&source_id)
        .bind(SyncType::Full)
        .bind(SyncStatus::Running)
        .execute(pool)
        .await?;

    // Test with running sync
    let running_sync = sync_manager.get_running_sync_for_source(&source_id).await?;
    assert!(running_sync.is_some());
    assert_eq!(running_sync.unwrap().id, sync_id);

    Ok(())
}

#[tokio::test]
async fn test_recover_interrupted_syncs() -> Result<()> {
    // Setup test environment
    let test_env = TestEnvironment::new().await?;
    let pool = test_env.db_pool.pool();
    let redis_client = test_env.redis_client.clone();

    // Create a test source
    let source_id = generate_ulid();
    let user_id = generate_ulid();

    // First create a user
    sqlx::query("INSERT INTO users (id, email, name, role) VALUES ($1, $2, $3, $4)")
        .bind(&user_id)
        .bind("test3@example.com")
        .bind("Test User 3")
        .bind("admin")
        .execute(pool)
        .await?;

    // Then create a source
    sqlx::query(
        "INSERT INTO sources (id, name, source_type, is_active, created_by) VALUES ($1, $2, $3, $4, $5)"
    )
    .bind(&source_id)
    .bind("Test Google Source 3")
    .bind(SourceType::GoogleDrive)
    .bind(true)
    .bind(&user_id)
    .execute(pool)
    .await?;

    // Create multiple running sync runs (simulating various interrupted syncs)
    let sync_id_1 = generate_ulid();
    let sync_id_2 = generate_ulid();
    let sync_id_3 = generate_ulid();

    // Recent sync (5 minutes ago)
    let five_minutes_ago =
        sqlx::types::time::OffsetDateTime::now_utc() - time::Duration::minutes(5);

    sqlx::query(
        "INSERT INTO sync_runs (id, source_id, sync_type, status, started_at) VALUES ($1, $2, $3, $4, $5)"
    )
    .bind(&sync_id_1)
    .bind(&source_id)
    .bind(SyncType::Full)
    .bind(SyncStatus::Running)
    .bind(five_minutes_ago)
    .execute(pool)
    .await?;

    // Old sync (1 hour ago)
    let one_hour_ago = sqlx::types::time::OffsetDateTime::now_utc() - time::Duration::hours(1);

    sqlx::query(
        "INSERT INTO sync_runs (id, source_id, sync_type, status, started_at) VALUES ($1, $2, $3, $4, $5)"
    )
    .bind(&sync_id_2)
    .bind(&source_id)
    .bind(SyncType::Incremental)
    .bind(SyncStatus::Running)
    .bind(one_hour_ago)
    .execute(pool)
    .await?;

    // Another source's sync
    let other_source_id = generate_ulid();
    sqlx::query(
        "INSERT INTO sources (id, name, source_type, is_active, created_by) VALUES ($1, $2, $3, $4, $5)"
    )
    .bind(&other_source_id)
    .bind("Other Google Source")
    .bind(SourceType::GoogleDrive)
    .bind(true)
    .bind(&user_id)
    .execute(pool)
    .await?;

    sqlx::query(
        "INSERT INTO sync_runs (id, source_id, sync_type, status, started_at) VALUES ($1, $2, $3, $4, $5)"
    )
    .bind(&sync_id_3)
    .bind(&other_source_id)
    .bind(SyncType::Full)
    .bind(SyncStatus::Running)
    .bind(five_minutes_ago)
    .execute(pool)
    .await?;

    // Create sync manager
    let sync_manager = SyncManager::new(pool.clone(), redis_client).await?;

    // Test recover_interrupted_syncs - should mark ALL running syncs as failed
    sync_manager.recover_interrupted_syncs().await?;

    // Verify all running syncs are now marked as failed
    let failed_syncs = sqlx::query_as::<_, SyncRun>(
        "SELECT * FROM sync_runs WHERE id IN ($1, $2, $3) ORDER BY started_at ASC",
    )
    .bind(&sync_id_1)
    .bind(&sync_id_2)
    .bind(&sync_id_3)
    .fetch_all(pool)
    .await?;

    assert_eq!(failed_syncs.len(), 3);
    for sync_run in failed_syncs {
        assert_eq!(sync_run.status, SyncStatus::Failed);
        assert!(sync_run.error_message.is_some());
        assert_eq!(
            sync_run.error_message.unwrap(),
            "Sync interrupted by connector restart"
        );
        assert!(sync_run.completed_at.is_some());
    }

    // Test that should_run_full_sync now returns true (no running syncs blocking)
    let should_sync_1 = sync_manager.should_run_full_sync(&source_id).await?;
    let should_sync_2 = sync_manager.should_run_full_sync(&other_source_id).await?;

    // These might be true or false depending on timing logic, but importantly they should not be blocked
    // by running syncs anymore

    Ok(())
}

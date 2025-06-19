use anyhow::Result;
use chrono::{Duration, Utc};
use redis::Client as RedisClient;
use shared::models::{SyncRun, SyncStatus, SyncType};
use shared::test_environment::TestEnvironment;
use sqlx::PgPool;

#[tokio::test]
async fn test_sync_run_tracking() -> Result<()> {
    let test_env = TestEnvironment::new().await?;
    let pool = test_env.database.get_pool();
    let redis_client = RedisClient::open(test_env.redis.connection_string())?;

    // Create a test source
    let source_id = shared::utils::generate_ulid();
    let user_id = shared::utils::generate_ulid();

    // Insert test user first
    sqlx::query(
        "INSERT INTO users (id, email, full_name, role) VALUES ($1, 'test@example.com', 'Test User', 'admin')"
    )
    .bind(&user_id)
    .execute(pool)
    .await?;

    // Insert test source
    sqlx::query(
        "INSERT INTO sources (id, name, source_type, config, created_by) VALUES ($1, 'Test Source', 'google_drive', '{}', $2)"
    )
    .bind(&source_id)
    .bind(&user_id)
    .execute(pool)
    .await?;

    // Create sync manager
    let sync_manager = google_connector::sync::SyncManager::new(pool.clone(), redis_client).await?;

    // Test that should_run_full_sync returns true for new source (no previous sync)
    let should_sync = sync_manager.should_run_full_sync(&source_id).await?;
    assert!(
        should_sync,
        "Should run full sync for source with no previous sync"
    );

    // Create a completed sync run from 30 minutes ago
    let sync_run_id = shared::utils::generate_ulid();
    let completed_at = Utc::now() - Duration::minutes(30);

    sqlx::query(
        "INSERT INTO sync_runs (id, source_id, sync_type, status, completed_at, files_processed, files_updated) 
         VALUES ($1, $2, $3, $4, $5, 10, 5)"
    )
    .bind(&sync_run_id)
    .bind(&source_id)
    .bind(SyncType::Full)
    .bind(SyncStatus::Completed)
    .bind(completed_at)
    .execute(pool)
    .await?;

    // Test that should_run_full_sync returns false for recent sync (default interval is 24 hours)
    let should_sync = sync_manager.should_run_full_sync(&source_id).await?;
    assert!(
        !should_sync,
        "Should not run full sync for source with recent sync"
    );

    // Test creating a new sync run
    let new_sync_run_id = sync_manager
        .create_sync_run(&source_id, SyncType::Full)
        .await?;
    assert!(
        !new_sync_run_id.is_empty(),
        "Should return valid sync run ID"
    );

    // Verify the sync run was created
    let sync_run: Option<SyncRun> = sqlx::query_as("SELECT * FROM sync_runs WHERE id = $1")
        .bind(&new_sync_run_id)
        .fetch_optional(pool)
        .await?;

    assert!(sync_run.is_some(), "Sync run should be created in database");
    let sync_run = sync_run.unwrap();
    assert_eq!(sync_run.source_id, source_id);
    assert_eq!(sync_run.sync_type, SyncType::Full);
    assert_eq!(sync_run.status, SyncStatus::Running);

    // Test completing the sync run
    sync_manager
        .update_sync_run_completed(&new_sync_run_id, 100, 50)
        .await?;

    // Verify the sync run was updated
    let updated_sync_run: Option<SyncRun> = sqlx::query_as("SELECT * FROM sync_runs WHERE id = $1")
        .bind(&new_sync_run_id)
        .fetch_optional(pool)
        .await?;

    assert!(updated_sync_run.is_some(), "Updated sync run should exist");
    let updated_sync_run = updated_sync_run.unwrap();
    assert_eq!(updated_sync_run.status, SyncStatus::Completed);
    assert_eq!(updated_sync_run.files_processed, 100);
    assert_eq!(updated_sync_run.files_updated, 50);
    assert!(
        updated_sync_run.completed_at.is_some(),
        "Should have completed_at timestamp"
    );

    Ok(())
}

mod common;

use anyhow::Result;
use shared::models::{SourceType, SyncRun, SyncStatus, SyncType};
use time::OffsetDateTime;

use common::GoogleConnectorTestFixture;

/// Tests the stale sync recovery pattern - finding and marking stale syncs as failed.
/// This tests the repository-level operations that SyncManager.recover_interrupted_syncs() uses.
#[tokio::test]
async fn test_stale_sync_detection_and_recovery() -> Result<()> {
    let fixture = GoogleConnectorTestFixture::new().await?;
    let sync_run_repo = fixture.sync_run_repo();

    // Create test user and source
    let user_id = fixture.create_test_user("test_stale@example.com").await?;
    let source_id = fixture
        .create_test_source("Test Google Source", SourceType::GoogleDrive, &user_id)
        .await?;

    // Create a stale running sync run (started 3 hours ago)
    let three_hours_ago = OffsetDateTime::now_utc() - time::Duration::hours(3);
    let stale_sync_id = fixture
        .create_sync_run(&source_id, SyncType::Full, SyncStatus::Running, three_hours_ago)
        .await?;

    // Create a recent running sync run (started 30 minutes ago)
    let thirty_minutes_ago = OffsetDateTime::now_utc() - time::Duration::minutes(30);
    let recent_sync_id = fixture
        .create_sync_run(
            &source_id,
            SyncType::Incremental,
            SyncStatus::Running,
            thirty_minutes_ago,
        )
        .await?;

    // Find all running syncs
    let running_syncs = sync_run_repo.find_all_running().await?;

    // Filter for our test syncs
    let our_running_syncs: Vec<&SyncRun> = running_syncs
        .iter()
        .filter(|s| s.id == stale_sync_id || s.id == recent_sync_id)
        .collect();
    assert_eq!(our_running_syncs.len(), 2);

    // Simulate recovery: mark ALL running syncs as failed (connector restart scenario)
    for sync in &running_syncs {
        if sync.id == stale_sync_id || sync.id == recent_sync_id {
            sync_run_repo
                .mark_failed(&sync.id, "Sync interrupted by connector restart")
                .await?;
        }
    }

    // Verify both syncs are now marked as failed
    let stale_sync = sync_run_repo.find_by_id(&stale_sync_id).await?.unwrap();
    assert_eq!(stale_sync.status, SyncStatus::Failed);
    assert_eq!(
        stale_sync.error_message.as_deref(),
        Some("Sync interrupted by connector restart")
    );
    assert!(stale_sync.completed_at.is_some());

    let recent_sync = sync_run_repo.find_by_id(&recent_sync_id).await?.unwrap();
    assert_eq!(recent_sync.status, SyncStatus::Failed);
    assert!(recent_sync.completed_at.is_some());

    // After recovery, no running syncs should exist for this source
    let running_after_recovery = sync_run_repo.get_running_for_source(&source_id).await?;
    assert!(running_after_recovery.is_none());

    Ok(())
}

/// Tests that we can detect running syncs across multiple sources
#[tokio::test]
async fn test_find_running_syncs_across_sources() -> Result<()> {
    let fixture = GoogleConnectorTestFixture::new().await?;
    let sync_run_repo = fixture.sync_run_repo();

    // Create test user and multiple sources
    let user_id = fixture.create_test_user("test_multi@example.com").await?;
    let source_id_1 = fixture
        .create_test_source("Google Drive Source", SourceType::GoogleDrive, &user_id)
        .await?;
    let source_id_2 = fixture
        .create_test_source("Gmail Source", SourceType::Gmail, &user_id)
        .await?;

    let now = OffsetDateTime::now_utc();

    // Create running syncs for both sources
    let sync_id_1 = fixture
        .create_sync_run(&source_id_1, SyncType::Full, SyncStatus::Running, now)
        .await?;
    let sync_id_2 = fixture
        .create_sync_run(&source_id_2, SyncType::Incremental, SyncStatus::Running, now)
        .await?;

    // Find all running syncs
    let running_syncs = sync_run_repo.find_all_running().await?;

    // Verify we find both syncs
    let our_syncs: Vec<&str> = running_syncs
        .iter()
        .filter(|s| s.id == sync_id_1 || s.id == sync_id_2)
        .map(|s| s.id.as_str())
        .collect();
    assert_eq!(our_syncs.len(), 2);

    // Each source should have its own running sync
    let running_1 = sync_run_repo.get_running_for_source(&source_id_1).await?;
    assert!(running_1.is_some());
    assert_eq!(running_1.unwrap().id, sync_id_1);

    let running_2 = sync_run_repo.get_running_for_source(&source_id_2).await?;
    assert!(running_2.is_some());
    assert_eq!(running_2.unwrap().id, sync_id_2);

    Ok(())
}

/// Tests the sync scheduling logic based on completed sync timing
#[tokio::test]
async fn test_sync_scheduling_after_completion() -> Result<()> {
    let fixture = GoogleConnectorTestFixture::new().await?;
    let sync_run_repo = fixture.sync_run_repo();

    // Create test user and source
    let user_id = fixture.create_test_user("test_schedule@example.com").await?;
    let source_id = fixture
        .create_test_source("Test Source Schedule", SourceType::GoogleDrive, &user_id)
        .await?;

    // No previous sync - get_last_completed_for_source should return None
    let last_sync = sync_run_repo
        .get_last_completed_for_source(&source_id, SyncType::Full)
        .await?;
    assert!(last_sync.is_none(), "Should have no previous sync");

    // Create and complete a sync
    let sync_run = sync_run_repo.create(&source_id, SyncType::Full).await?;
    sync_run_repo.mark_completed(&sync_run.id, 100, 50).await?;

    // Now we should have a completed sync
    let last_sync = sync_run_repo
        .get_last_completed_for_source(&source_id, SyncType::Full)
        .await?;
    assert!(last_sync.is_some());

    let last_sync = last_sync.unwrap();
    assert_eq!(last_sync.id, sync_run.id);
    assert!(last_sync.completed_at.is_some());

    // The sync interval check would happen at the SyncManager level,
    // but we've verified the repository provides the correct data

    Ok(())
}

/// Tests that only syncs of the correct type are returned
#[tokio::test]
async fn test_sync_type_filtering() -> Result<()> {
    let fixture = GoogleConnectorTestFixture::new().await?;
    let sync_run_repo = fixture.sync_run_repo();

    // Create test user and source
    let user_id = fixture.create_test_user("test_filter@example.com").await?;
    let source_id = fixture
        .create_test_source("Test Source Filter", SourceType::GoogleDrive, &user_id)
        .await?;

    // Create and complete a Full sync
    let full_sync = sync_run_repo.create(&source_id, SyncType::Full).await?;
    sync_run_repo.mark_completed(&full_sync.id, 100, 50).await?;

    // Create and complete an Incremental sync
    let incremental_sync = sync_run_repo
        .create(&source_id, SyncType::Incremental)
        .await?;
    sync_run_repo
        .mark_completed(&incremental_sync.id, 10, 5)
        .await?;

    // Query for Full syncs only
    let last_full = sync_run_repo
        .get_last_completed_for_source(&source_id, SyncType::Full)
        .await?;
    assert!(last_full.is_some());
    assert_eq!(last_full.unwrap().sync_type, SyncType::Full);

    // Query for Incremental syncs only
    let last_incremental = sync_run_repo
        .get_last_completed_for_source(&source_id, SyncType::Incremental)
        .await?;
    assert!(last_incremental.is_some());
    assert_eq!(last_incremental.unwrap().sync_type, SyncType::Incremental);

    Ok(())
}

/// Tests that failed syncs don't interfere with running sync detection
#[tokio::test]
async fn test_failed_syncs_dont_block() -> Result<()> {
    let fixture = GoogleConnectorTestFixture::new().await?;
    let sync_run_repo = fixture.sync_run_repo();

    // Create test user and source
    let user_id = fixture.create_test_user("test_failed@example.com").await?;
    let source_id = fixture
        .create_test_source("Test Source Failed", SourceType::GoogleDrive, &user_id)
        .await?;

    // Create a sync and mark it as failed
    let failed_sync = sync_run_repo.create(&source_id, SyncType::Full).await?;
    sync_run_repo
        .mark_failed(&failed_sync.id, "Test failure")
        .await?;

    // Should not show up as running
    let running = sync_run_repo.get_running_for_source(&source_id).await?;
    assert!(running.is_none());

    // Should not be returned as a completed sync
    let completed = sync_run_repo
        .get_last_completed_for_source(&source_id, SyncType::Full)
        .await?;
    assert!(completed.is_none());

    // Create and complete a new sync
    let new_sync = sync_run_repo.create(&source_id, SyncType::Full).await?;
    sync_run_repo.mark_completed(&new_sync.id, 50, 25).await?;

    // Now we should have a completed sync
    let completed = sync_run_repo
        .get_last_completed_for_source(&source_id, SyncType::Full)
        .await?;
    assert!(completed.is_some());
    assert_eq!(completed.unwrap().id, new_sync.id);

    Ok(())
}

mod common;

use anyhow::Result;
use shared::models::{SourceType, SyncStatus, SyncType};

use common::GoogleConnectorTestFixture;

#[tokio::test]
async fn test_sync_run_creation() -> Result<()> {
    let fixture = GoogleConnectorTestFixture::new().await?;
    let sync_run_repo = fixture.sync_run_repo();

    // Create test user and source
    let user_id = fixture.create_test_user("test_creation@example.com").await?;
    let source_id = fixture
        .create_test_source("Test Source", SourceType::GoogleDrive, &user_id)
        .await?;

    // Create a sync run
    let sync_run = sync_run_repo.create(&source_id, SyncType::Full).await?;

    assert_eq!(sync_run.source_id, source_id);
    assert_eq!(sync_run.sync_type, SyncType::Full);
    assert_eq!(sync_run.status, SyncStatus::Running);
    assert_eq!(sync_run.documents_scanned, 0);
    assert_eq!(sync_run.documents_processed, 0);
    assert_eq!(sync_run.documents_updated, 0);

    Ok(())
}

#[tokio::test]
async fn test_sync_run_completion() -> Result<()> {
    let fixture = GoogleConnectorTestFixture::new().await?;
    let sync_run_repo = fixture.sync_run_repo();

    // Create test user and source
    let user_id = fixture.create_test_user("test_completion@example.com").await?;
    let source_id = fixture
        .create_test_source("Test Source Completion", SourceType::GoogleDrive, &user_id)
        .await?;

    // Create and complete a sync run
    let sync_run = sync_run_repo.create(&source_id, SyncType::Full).await?;
    sync_run_repo.mark_completed(&sync_run.id, 100, 50).await?;

    // Verify the sync run was updated
    let updated = sync_run_repo.find_by_id(&sync_run.id).await?;
    assert!(updated.is_some());

    let updated = updated.unwrap();
    assert_eq!(updated.status, SyncStatus::Completed);
    assert_eq!(updated.documents_scanned, 100);
    assert_eq!(updated.documents_updated, 50);
    assert!(updated.completed_at.is_some());

    Ok(())
}

#[tokio::test]
async fn test_sync_run_failure() -> Result<()> {
    let fixture = GoogleConnectorTestFixture::new().await?;
    let sync_run_repo = fixture.sync_run_repo();

    // Create test user and source
    let user_id = fixture.create_test_user("test_failure@example.com").await?;
    let source_id = fixture
        .create_test_source("Test Source Failure", SourceType::GoogleDrive, &user_id)
        .await?;

    // Create and fail a sync run
    let sync_run = sync_run_repo.create(&source_id, SyncType::Full).await?;
    let error_message = "Test error: connection timeout";
    sync_run_repo.mark_failed(&sync_run.id, error_message).await?;

    // Verify the sync run was updated
    let updated = sync_run_repo.find_by_id(&sync_run.id).await?;
    assert!(updated.is_some());

    let updated = updated.unwrap();
    assert_eq!(updated.status, SyncStatus::Failed);
    assert_eq!(updated.error_message.as_deref(), Some(error_message));
    assert!(updated.completed_at.is_some());

    Ok(())
}

#[tokio::test]
async fn test_get_last_completed_sync() -> Result<()> {
    let fixture = GoogleConnectorTestFixture::new().await?;
    let sync_run_repo = fixture.sync_run_repo();

    // Create test user and source
    let user_id = fixture.create_test_user("test_last_completed@example.com").await?;
    let source_id = fixture
        .create_test_source("Test Source Last Completed", SourceType::GoogleDrive, &user_id)
        .await?;

    // Initially, no completed sync should exist
    let last_sync = sync_run_repo
        .get_last_completed_for_source(&source_id, SyncType::Full)
        .await?;
    assert!(last_sync.is_none());

    // Create and complete a sync run
    let sync_run = sync_run_repo.create(&source_id, SyncType::Full).await?;
    sync_run_repo.mark_completed(&sync_run.id, 10, 5).await?;

    // Now we should have a completed sync
    let last_sync = sync_run_repo
        .get_last_completed_for_source(&source_id, SyncType::Full)
        .await?;
    assert!(last_sync.is_some());
    assert_eq!(last_sync.unwrap().id, sync_run.id);

    Ok(())
}

#[tokio::test]
async fn test_get_running_sync_for_source() -> Result<()> {
    let fixture = GoogleConnectorTestFixture::new().await?;
    let sync_run_repo = fixture.sync_run_repo();

    // Create test user and source
    let user_id = fixture.create_test_user("test_running@example.com").await?;
    let source_id = fixture
        .create_test_source("Test Source Running", SourceType::GoogleDrive, &user_id)
        .await?;

    // Initially, no running sync should exist
    let running_sync = sync_run_repo.get_running_for_source(&source_id).await?;
    assert!(running_sync.is_none());

    // Create a running sync
    let sync_run = sync_run_repo.create(&source_id, SyncType::Full).await?;

    // Now we should have a running sync
    let running_sync = sync_run_repo.get_running_for_source(&source_id).await?;
    assert!(running_sync.is_some());
    assert_eq!(running_sync.unwrap().id, sync_run.id);

    // Complete the sync
    sync_run_repo.mark_completed(&sync_run.id, 10, 5).await?;

    // No longer running
    let running_sync = sync_run_repo.get_running_for_source(&source_id).await?;
    assert!(running_sync.is_none());

    Ok(())
}

#[tokio::test]
async fn test_find_all_running_syncs() -> Result<()> {
    let fixture = GoogleConnectorTestFixture::new().await?;
    let sync_run_repo = fixture.sync_run_repo();

    // Create test user and multiple sources
    let user_id = fixture.create_test_user("test_all_running@example.com").await?;
    let source_id_1 = fixture
        .create_test_source("Test Source 1", SourceType::GoogleDrive, &user_id)
        .await?;
    let source_id_2 = fixture
        .create_test_source("Test Source 2", SourceType::Gmail, &user_id)
        .await?;

    // Create running syncs for both sources
    let sync_1 = sync_run_repo.create(&source_id_1, SyncType::Full).await?;
    let sync_2 = sync_run_repo.create(&source_id_2, SyncType::Incremental).await?;

    // Find all running syncs
    let running_syncs = sync_run_repo.find_all_running().await?;

    // Should find at least our 2 syncs (might be more from other tests)
    let our_sync_ids: Vec<&str> = running_syncs
        .iter()
        .filter(|s| s.id == sync_1.id || s.id == sync_2.id)
        .map(|s| s.id.as_str())
        .collect();
    assert_eq!(our_sync_ids.len(), 2);

    Ok(())
}

#[tokio::test]
async fn test_increment_scanned_count() -> Result<()> {
    let fixture = GoogleConnectorTestFixture::new().await?;
    let sync_run_repo = fixture.sync_run_repo();

    // Create test user and source
    let user_id = fixture.create_test_user("test_increment@example.com").await?;
    let source_id = fixture
        .create_test_source("Test Source Increment", SourceType::GoogleDrive, &user_id)
        .await?;

    // Create a sync run
    let sync_run = sync_run_repo.create(&source_id, SyncType::Full).await?;

    // Increment scanned count
    sync_run_repo.increment_scanned(&sync_run.id, 25).await?;
    sync_run_repo.increment_scanned(&sync_run.id, 15).await?;

    // Verify count
    let updated = sync_run_repo.find_by_id(&sync_run.id).await?.unwrap();
    assert_eq!(updated.documents_scanned, 40);

    Ok(())
}

#[tokio::test]
async fn test_increment_progress() -> Result<()> {
    let fixture = GoogleConnectorTestFixture::new().await?;
    let sync_run_repo = fixture.sync_run_repo();

    // Create test user and source
    let user_id = fixture.create_test_user("test_progress@example.com").await?;
    let source_id = fixture
        .create_test_source("Test Source Progress", SourceType::GoogleDrive, &user_id)
        .await?;

    // Create a sync run
    let sync_run = sync_run_repo.create(&source_id, SyncType::Full).await?;

    // Increment progress
    sync_run_repo.increment_progress(&sync_run.id).await?;
    sync_run_repo.increment_progress_by(&sync_run.id, 5).await?;

    // Verify count
    let updated = sync_run_repo.find_by_id(&sync_run.id).await?.unwrap();
    assert_eq!(updated.documents_processed, 6);

    Ok(())
}

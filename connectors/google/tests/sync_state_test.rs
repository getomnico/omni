mod common;

use anyhow::Result;
use omni_google_connector::sync::SyncState;
use std::collections::HashSet;

use common::GoogleConnectorTestFixture;

#[tokio::test]
async fn test_sync_state_set_and_get() -> Result<()> {
    let fixture = GoogleConnectorTestFixture::new().await?;
    let sync_state = SyncState::new(fixture.redis_client());

    let source_id = "test_source";
    let file_id = "test_file";
    let modified_time = "2023-01-01T12:00:00Z";

    // Initially, no state should exist
    assert_eq!(
        sync_state.get_file_sync_state(source_id, file_id).await?,
        None
    );

    // Set the state
    sync_state
        .set_file_sync_state_with_expiry(source_id, file_id, modified_time, 60)
        .await?;

    // State should now exist
    assert_eq!(
        sync_state.get_file_sync_state(source_id, file_id).await?,
        Some(modified_time.to_string())
    );

    // Clean up
    sync_state
        .delete_file_sync_state(source_id, file_id)
        .await?;

    Ok(())
}

#[tokio::test]
async fn test_sync_state_delete() -> Result<()> {
    let fixture = GoogleConnectorTestFixture::new().await?;
    let sync_state = SyncState::new(fixture.redis_client());

    let source_id = "test_source_delete";
    let file_id = "test_file_delete";
    let modified_time = "2023-01-01T12:00:00Z";

    // Set the state
    sync_state
        .set_file_sync_state(source_id, file_id, modified_time)
        .await?;

    // Verify it exists
    assert!(sync_state
        .get_file_sync_state(source_id, file_id)
        .await?
        .is_some());

    // Delete it
    sync_state
        .delete_file_sync_state(source_id, file_id)
        .await?;

    // Verify it's gone
    assert_eq!(
        sync_state.get_file_sync_state(source_id, file_id).await?,
        None
    );

    Ok(())
}

#[tokio::test]
async fn test_get_all_synced_file_ids() -> Result<()> {
    let fixture = GoogleConnectorTestFixture::new().await?;
    let sync_state = SyncState::new(fixture.redis_client());

    let source_id = "test_source_all_files";
    let files = vec![
        ("file1", "2023-01-01T12:00:00Z"),
        ("file2", "2023-01-02T12:00:00Z"),
        ("file3", "2023-01-03T12:00:00Z"),
    ];

    // Set multiple file states
    for (file_id, modified_time) in &files {
        sync_state
            .set_file_sync_state_with_expiry(source_id, file_id, modified_time, 60)
            .await?;
    }

    // Get all synced file IDs
    let synced_files = sync_state.get_all_synced_file_ids(source_id).await?;

    // Should contain all file IDs
    let expected: HashSet<String> = files.iter().map(|(id, _)| id.to_string()).collect();
    assert_eq!(synced_files, expected);

    // Clean up
    for (file_id, _) in &files {
        sync_state
            .delete_file_sync_state(source_id, file_id)
            .await?;
    }

    Ok(())
}

#[tokio::test]
async fn test_thread_sync_state() -> Result<()> {
    let fixture = GoogleConnectorTestFixture::new().await?;
    let sync_state = SyncState::new(fixture.redis_client());

    let source_id = "test_gmail_source";
    let thread_id = "thread123";
    let latest_date = "1704067200000"; // Unix timestamp in ms

    // Initially, no state should exist
    assert_eq!(
        sync_state
            .get_thread_sync_state(source_id, thread_id)
            .await?,
        None
    );

    // Set the state
    sync_state
        .set_thread_sync_state(source_id, thread_id, latest_date)
        .await?;

    // State should now exist
    assert_eq!(
        sync_state
            .get_thread_sync_state(source_id, thread_id)
            .await?,
        Some(latest_date.to_string())
    );

    Ok(())
}

/// Pure unit test for modification time comparison logic
#[test]
fn test_modification_time_comparison_logic() {
    struct TestCase {
        stored_time: Option<&'static str>,
        current_time: &'static str,
        should_process: bool,
        description: &'static str,
    }

    let test_cases = vec![
        TestCase {
            stored_time: None,
            current_time: "2023-01-01T12:00:00Z",
            should_process: true,
            description: "New file should be processed",
        },
        TestCase {
            stored_time: Some("2023-01-01T12:00:00Z"),
            current_time: "2023-01-01T12:00:00Z",
            should_process: false,
            description: "Unchanged file should be skipped",
        },
        TestCase {
            stored_time: Some("2023-01-01T12:00:00Z"),
            current_time: "2023-01-01T13:00:00Z",
            should_process: true,
            description: "Modified file should be processed",
        },
    ];

    for test_case in test_cases {
        let should_process = match test_case.stored_time {
            Some(stored) => stored != test_case.current_time,
            None => true,
        };

        assert_eq!(
            should_process, test_case.should_process,
            "Failed: {}",
            test_case.description
        );
    }
}

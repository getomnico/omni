mod common;

use anyhow::Result;
use omni_google_connector::models::WebhookNotification;
use omni_google_connector::sync::SyncState;
use shared::db::repositories::SyncRunRepository;
use shared::models::SyncStatus;
use std::collections::HashSet;
use std::sync::atomic::Ordering;
use std::time::Duration;

use common::GoogleConnectorTestFixture;

// ============================================================================
// Sync state tests
// ============================================================================

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

// ============================================================================
// Webhook debounce tests
// ============================================================================

#[tokio::test]
async fn test_webhook_debounce_buffers_and_flushes() -> Result<()> {
    let fixture = GoogleConnectorTestFixture::new().await?;
    let source_id = fixture.source_id().to_string();

    // Set debounce to zero so entries expire immediately
    fixture
        .sync_manager
        .debounce_duration_ms
        .store(0, Ordering::Relaxed);

    let states = ["add", "update", "change", "update", "remove"];
    for state in &states {
        let notification = WebhookNotification {
            channel_id: "ch-1".to_string(),
            resource_state: state.to_string(),
            resource_id: Some("res-1".to_string()),
            resource_uri: None,
            changed: None,
            source_id: Some(source_id.clone()),
        };
        fixture
            .sync_manager
            .handle_webhook_notification(notification)
            .await?;
    }

    // All 5 webhooks should be buffered into a single debounce entry
    assert_eq!(fixture.sync_manager.webhook_debounce.len(), 1);
    let entry = fixture
        .sync_manager
        .webhook_debounce
        .get(&source_id)
        .expect("debounce entry should exist");
    assert_eq!(entry.count, 5);
    drop(entry);

    // Spawn the processor briefly — with Duration::ZERO the entry is already expired
    let sm = fixture.sync_manager.clone();
    let processor = tokio::spawn(async move {
        sm.run_webhook_processor().await;
    });
    tokio::time::sleep(Duration::from_millis(200)).await;
    processor.abort();

    // End-to-end: webhook → CM → POST /sync on the real SDK-served connector
    // → GoogleConnector::sync → run_sync → credentials lookup fails (no creds
    // seeded in the test DB) → SDK returns 4xx/5xx → CM's connector_client
    // surfaces that as ClientError → CM marks the sync_run failed. We assert
    // the terminal state rather than just the presence of a running row so a
    // regression that silently drops the sync (or hangs it) fails this test.
    let sync_run_repo = SyncRunRepository::new(fixture.pool());
    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    let terminal_run = loop {
        let latest = sync_run_repo
            .find_latest_for_sources(&[source_id.clone()])
            .await?
            .into_iter()
            .next();
        if let Some(run) = latest {
            if run.status != SyncStatus::Running {
                break run;
            }
        }
        if tokio::time::Instant::now() >= deadline {
            panic!("no terminal sync run for source {} within 5s", source_id);
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    };

    assert_eq!(
        terminal_run.status,
        SyncStatus::Failed,
        "sync run should have failed (no credentials seeded)"
    );
    assert!(
        terminal_run.error_message.is_some(),
        "failed sync run should record an error message"
    );

    Ok(())
}

#[tokio::test]
async fn test_webhook_debounce_retains_unexpired() -> Result<()> {
    let fixture = GoogleConnectorTestFixture::new().await?;
    let source_id = fixture.source_id().to_string();

    // Set debounce to 1 hour so entries never expire during this test
    fixture
        .sync_manager
        .debounce_duration_ms
        .store(3_600_000, Ordering::Relaxed);

    let notification = WebhookNotification {
        channel_id: "ch-2".to_string(),
        resource_state: "update".to_string(),
        resource_id: Some("res-2".to_string()),
        resource_uri: None,
        changed: None,
        source_id: Some(source_id.clone()),
    };
    fixture
        .sync_manager
        .handle_webhook_notification(notification)
        .await?;

    // Spawn processor briefly
    let sm = fixture.sync_manager.clone();
    let processor = tokio::spawn(async move {
        sm.run_webhook_processor().await;
    });
    tokio::time::sleep(Duration::from_millis(200)).await;
    processor.abort();

    // Entry should still be in the debounce map (not expired)
    assert_eq!(
        fixture.sync_manager.webhook_debounce.len(),
        1,
        "debounce entry should be retained when not yet expired"
    );

    // No sync run should have been created
    let sync_run_repo = SyncRunRepository::new(fixture.pool());
    let running = sync_run_repo.get_running_for_source(&source_id).await?;
    assert!(
        running.is_none(),
        "no sync run should be created for unexpired debounce entry"
    );

    Ok(())
}

mod common;
mod mock_slack;

use common::SlackConnectorTestFixture;
use mock_slack::{
    make_test_channels, make_test_messages, make_test_users, MockSlackServer, MockSlackState,
};
use omni_slack_connector::models::{SlackConnectorState, SlackMessage};
use omni_slack_connector::sync::SyncManager;
use shared::models::SyncRequest;

/// Use a fixed base timestamp (2025-01-15 12:00:00 UTC) so all messages
/// fall on the same calendar day.
const BASE_TS: i64 = 1736942400;

async fn setup_full_sync(
    fixture: &SlackConnectorTestFixture,
    mock_url: &str,
) -> (String, String, String) {
    let user_id = fixture
        .create_test_user("slack-test@example.com")
        .await
        .unwrap();
    let source_id = fixture
        .create_test_source("Test Slack", &user_id)
        .await
        .unwrap();
    fixture
        .create_test_credentials(&source_id, "xoxb-test-token-123")
        .await
        .unwrap();
    let sync_run_id = fixture.create_sync_run(&source_id).await.unwrap();

    let sync_manager =
        SyncManager::with_slack_base_url(fixture.sdk_client.clone(), mock_url.to_string());

    let request = SyncRequest {
        sync_run_id: sync_run_id.clone(),
        source_id: source_id.clone(),
        sync_mode: "full".to_string(),
        last_sync_at: None,
    };

    sync_manager
        .sync_source_from_request(request)
        .await
        .unwrap();

    (user_id, source_id, sync_run_id)
}

#[tokio::test]
async fn test_full_sync_creates_events() {
    let fixture = SlackConnectorTestFixture::new().await.unwrap();

    let mock_state = MockSlackState {
        channels: make_test_channels(),
        messages: make_test_messages(BASE_TS),
        users: make_test_users(),
    };
    let mock_server = MockSlackServer::start(mock_state).await;

    let (_user_id, source_id, sync_run_id) = setup_full_sync(&fixture, &mock_server.base_url).await;

    // Verify events were queued
    let events = fixture.get_queued_events(&source_id).await.unwrap();
    assert_eq!(
        events.len(),
        2,
        "Expected 2 events (1 per channel), got {}",
        events.len()
    );

    for event in &events {
        let event_type = event.get("type").and_then(|v| v.as_str()).unwrap_or("");
        assert_eq!(event_type, "document_created");

        let ev_source_id = event
            .get("source_id")
            .and_then(|v| v.as_str())
            .unwrap_or("");
        assert_eq!(ev_source_id, source_id);

        let doc_id = event
            .get("document_id")
            .and_then(|v| v.as_str())
            .unwrap_or("");
        assert!(
            doc_id.starts_with("slack_channel_"),
            "document_id should start with 'slack_channel_', got: {}",
            doc_id
        );

        let title = event
            .get("metadata")
            .and_then(|m| m.get("title"))
            .and_then(|v| v.as_str())
            .unwrap_or("");
        assert!(
            title.starts_with('#'),
            "Title should start with '#', got: {}",
            title
        );
        assert!(
            title.contains("2025-01-15"),
            "Title should contain the date, got: {}",
            title
        );
    }

    // Verify sync run completed
    let sync_run = fixture.get_sync_run(&sync_run_id).await.unwrap().unwrap();
    assert_eq!(
        sync_run.status,
        shared::models::SyncStatus::Completed,
        "Sync run should be completed"
    );
    assert_eq!(sync_run.documents_scanned, 2);

    // Verify connector state
    let state_value = fixture
        .get_connector_state(&source_id)
        .await
        .unwrap()
        .unwrap();
    let state: SlackConnectorState = serde_json::from_value(state_value).unwrap();
    assert_eq!(state.team_id, Some("T_TEST".to_string()));
    assert!(state.channel_timestamps.contains_key("C001"));
    assert!(state.channel_timestamps.contains_key("C002"));
}

#[tokio::test]
async fn test_sync_persists_state_for_incremental() {
    let fixture = SlackConnectorTestFixture::new().await.unwrap();

    let mock_state = MockSlackState {
        channels: make_test_channels(),
        messages: make_test_messages(BASE_TS),
        users: make_test_users(),
    };
    let mock_server = MockSlackServer::start(mock_state).await;

    let (_user_id, source_id, _first_sync_run_id) =
        setup_full_sync(&fixture, &mock_server.base_url).await;

    // Read state after first sync
    let state_value = fixture
        .get_connector_state(&source_id)
        .await
        .unwrap()
        .unwrap();
    let state: SlackConnectorState = serde_json::from_value(state_value).unwrap();
    let c001_ts = state.channel_timestamps.get("C001").cloned().unwrap();
    let _c002_ts = state.channel_timestamps.get("C002").cloned().unwrap();

    // Start a new mock with additional later messages
    let later_ts = BASE_TS + 3600; // 1 hour later, same day
    let mut later_messages = make_test_messages(BASE_TS);
    later_messages
        .get_mut("C001")
        .unwrap()
        .push(omni_slack_connector::models::SlackMessage {
            msg_type: "message".to_string(),
            text: "A new later message".to_string(),
            user: "U002".to_string(),
            ts: format!("{}.000400", later_ts),
            thread_ts: None,
            reply_count: None,
            attachments: None,
            files: None,
        });

    let mock_state2 = MockSlackState {
        channels: make_test_channels(),
        messages: later_messages,
        users: make_test_users(),
    };
    let mock_server2 = MockSlackServer::start(mock_state2).await;

    // Run second sync
    let sync_run_id_2 = fixture.create_sync_run(&source_id).await.unwrap();
    let sync_manager =
        SyncManager::with_slack_base_url(fixture.sdk_client.clone(), mock_server2.base_url.clone());
    let request = SyncRequest {
        sync_run_id: sync_run_id_2.clone(),
        source_id: source_id.clone(),
        sync_mode: "incremental".to_string(),
        last_sync_at: None,
    };
    sync_manager
        .sync_source_from_request(request)
        .await
        .unwrap();

    // Verify updated state
    let state_value = fixture
        .get_connector_state(&source_id)
        .await
        .unwrap()
        .unwrap();
    let state: SlackConnectorState = serde_json::from_value(state_value).unwrap();

    let new_c001_ts = state.channel_timestamps.get("C001").cloned().unwrap();
    assert!(
        new_c001_ts > c001_ts,
        "C001 timestamp should have advanced: {} -> {}",
        c001_ts,
        new_c001_ts
    );

    // C002 should have same or updated timestamp (messages unchanged but re-fetched)
    assert!(state.channel_timestamps.contains_key("C002"));

    // Verify second sync also completed
    let sync_run = fixture.get_sync_run(&sync_run_id_2).await.unwrap().unwrap();
    assert_eq!(sync_run.status, shared::models::SyncStatus::Completed);
}

#[tokio::test]
async fn test_realtime_event_syncs_single_channel() {
    let fixture = SlackConnectorTestFixture::new().await.unwrap();

    let mock_state = MockSlackState {
        channels: make_test_channels(),
        messages: make_test_messages(BASE_TS),
        users: make_test_users(),
    };
    let mock_server = MockSlackServer::start(mock_state).await;

    // Run a full sync first to establish baseline state
    let (_user_id, source_id, _sync_run_id) =
        setup_full_sync(&fixture, &mock_server.base_url).await;

    let events_before = fixture.get_queued_events(&source_id).await.unwrap();
    let events_before_count = events_before.len();

    // Record C002 timestamp before realtime sync
    let state_before: SlackConnectorState = serde_json::from_value(
        fixture
            .get_connector_state(&source_id)
            .await
            .unwrap()
            .unwrap(),
    )
    .unwrap();
    let c002_ts_before = state_before
        .channel_timestamps
        .get("C002")
        .cloned()
        .unwrap();

    // Start a new mock with an additional message in C001 only
    let later_ts = BASE_TS + 3600;
    let mut later_messages = make_test_messages(BASE_TS);
    later_messages.get_mut("C001").unwrap().push(SlackMessage {
        msg_type: "message".to_string(),
        text: "New realtime message".to_string(),
        user: "U001".to_string(),
        ts: format!("{}.000400", later_ts),
        thread_ts: None,
        reply_count: None,
        attachments: None,
        files: None,
    });

    let mock_state2 = MockSlackState {
        channels: make_test_channels(),
        messages: later_messages,
        users: make_test_users(),
    };
    let mock_server2 = MockSlackServer::start(mock_state2).await;

    // Call sync_realtime_event for C001 only
    let sync_manager =
        SyncManager::with_slack_base_url(fixture.sdk_client.clone(), mock_server2.base_url.clone());

    sync_manager
        .sync_realtime_event(&source_id, "C001")
        .await
        .unwrap();

    // Verify: new event emitted only for C001
    let events_after = fixture.get_queued_events(&source_id).await.unwrap();
    let new_events: Vec<_> = events_after[events_before_count..].to_vec();

    assert_eq!(
        new_events.len(),
        1,
        "Expected exactly 1 new event for C001, got {}",
        new_events.len()
    );

    let new_event = &new_events[0];
    let event_type = new_event.get("type").and_then(|v| v.as_str()).unwrap_or("");
    assert_eq!(
        event_type, "document_updated",
        "Realtime event should be document_updated, got {}",
        event_type
    );

    let doc_id = new_event
        .get("document_id")
        .and_then(|v| v.as_str())
        .unwrap_or("");
    assert!(
        doc_id.contains("C001"),
        "Event should be for C001, got document_id: {}",
        doc_id
    );

    // Verify connector state: C001 timestamp updated, C002 unchanged
    let state_after: SlackConnectorState = serde_json::from_value(
        fixture
            .get_connector_state(&source_id)
            .await
            .unwrap()
            .unwrap(),
    )
    .unwrap();

    let c001_ts_before = state_before
        .channel_timestamps
        .get("C001")
        .cloned()
        .unwrap();
    let c001_ts_after = state_after.channel_timestamps.get("C001").cloned().unwrap();
    assert!(
        c001_ts_after > c001_ts_before,
        "C001 timestamp should have advanced: {} -> {}",
        c001_ts_before,
        c001_ts_after
    );

    let c002_ts_after = state_after.channel_timestamps.get("C002").cloned().unwrap();
    assert_eq!(
        c002_ts_before, c002_ts_after,
        "C002 timestamp should be unchanged: {} vs {}",
        c002_ts_before, c002_ts_after
    );
}

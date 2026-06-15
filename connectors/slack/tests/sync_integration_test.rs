mod common;
mod mock_slack;

use common::SlackConnectorTestFixture;
use mock_slack::{
    MockSlackServer, MockSlackState, make_test_channel_members, make_test_channels,
    make_test_messages, make_test_users,
};
use omni_connector_sdk::SyncContext;
use omni_slack_connector::models::{SlackConnectorState, SlackMessage};
use omni_slack_connector::sync::SyncManager;
use shared::models::{SourceType, SyncType};
use sqlx::Row;
use std::sync::Arc;
use std::sync::atomic::AtomicBool;

/// Use a fixed base timestamp (2025-01-15 12:00:00 UTC) so all messages
/// fall on the same calendar day.
const BASE_TS: i64 = 1736942400;

/// Drive a sync via `SyncManager::run_sync`, mirroring what the SDK's `/sync`
/// HTTP handler does: fetch source/credentials/state, register the sync, build
/// a `SyncContext`, and dispatch.
async fn drive_sync(
    fixture: &SlackConnectorTestFixture,
    sync_manager: &SyncManager,
    source_id: &str,
    sync_run_id: &str,
    sync_mode: SyncType,
) {
    let source = fixture.sdk_client.get_source(source_id).await.unwrap();
    let creds = fixture.sdk_client.get_credentials(source_id).await.unwrap();
    let state: Option<SlackConnectorState> = fixture
        .sdk_client
        .get_connector_state(source_id)
        .await
        .unwrap()
        .and_then(|v| serde_json::from_value(v).ok());

    fixture
        .sdk_client
        .register_sync(sync_run_id, sync_mode)
        .await;

    let ctx = SyncContext::new(
        fixture.sdk_client.clone(),
        sync_run_id.to_string(),
        source_id.to_string(),
        SourceType::Slack,
        sync_mode,
        Arc::new(AtomicBool::new(false)),
    );

    sync_manager
        .run_sync(source, creds, state, ctx)
        .await
        .unwrap();
}

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

    drive_sync(
        fixture,
        &sync_manager,
        &source_id,
        &sync_run_id,
        SyncType::Full,
    )
    .await;

    (user_id, source_id, sync_run_id)
}

async fn content_for_event(
    fixture: &SlackConnectorTestFixture,
    event: &serde_json::Value,
) -> String {
    let content_id = event
        .get("content_id")
        .and_then(|v| v.as_str())
        .expect("event should include content_id");
    let row = sqlx::query("SELECT content FROM content_blobs WHERE id = $1")
        .bind(content_id)
        .fetch_one(fixture.pool())
        .await
        .unwrap();
    let bytes: Vec<u8> = row.get("content");
    String::from_utf8(bytes).unwrap()
}

#[tokio::test]
async fn test_full_sync_creates_events() {
    let fixture = SlackConnectorTestFixture::new().await.unwrap();

    let mock_state = MockSlackState {
        channels: make_test_channels(),
        messages: make_test_messages(BASE_TS),
        users: make_test_users(),
        channel_members: make_test_channel_members(),
        thread_replies: std::collections::HashMap::new(),
    };
    let mock_server = MockSlackServer::start(mock_state).await;

    let (_user_id, source_id, sync_run_id) = setup_full_sync(&fixture, &mock_server.base_url).await;

    // Per channel: 1 group_membership_sync + 1 document_created = 2 events.
    // Two channels → 4 events.
    let events = fixture.get_queued_events(&source_id).await.unwrap();
    assert_eq!(events.len(), 4, "Expected 4 events, got {}", events.len());

    let doc_events: Vec<_> = events
        .iter()
        .filter(|e| e.get("type").and_then(|v| v.as_str()) == Some("document_created"))
        .collect();
    let group_events: Vec<_> = events
        .iter()
        .filter(|e| e.get("type").and_then(|v| v.as_str()) == Some("group_membership_sync"))
        .collect();
    assert_eq!(doc_events.len(), 2, "Expected 2 document_created events");
    assert_eq!(
        group_events.len(),
        2,
        "Expected 2 group_membership_sync events"
    );

    // Group-membership events should carry the channel members.
    for event in &group_events {
        let group_email = event
            .get("group_email")
            .and_then(|v| v.as_str())
            .unwrap_or("");
        assert!(
            group_email.starts_with("slack-channel:T_TEST:"),
            "group_email should be slack-channel:T_TEST:<channel_id>, got {}",
            group_email
        );
        let members: Vec<&str> = event
            .get("member_emails")
            .and_then(|v| v.as_array())
            .map(|a| a.iter().filter_map(|v| v.as_str()).collect())
            .unwrap_or_default();
        assert!(
            members.contains(&"alice@example.com") && members.contains(&"bob@example.com"),
            "group members should include alice and bob, got {:?}",
            members
        );
    }

    // Document events: permissions should reference the channel group, NOT
    // inline the user list.
    for event in &doc_events {
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
            title.starts_with('#') && title.contains("2025-01-15"),
            "Title should be '#<channel> - 2025-01-15', got: {}",
            title
        );

        let permissions = event
            .get("permissions")
            .expect("event should have permissions");
        let users = permissions
            .get("users")
            .and_then(|v| v.as_array())
            .expect("permissions should have users array");
        assert!(
            users.is_empty(),
            "permissions.users should be empty (membership goes via groups), got: {:?}",
            users
        );

        let groups: Vec<&str> = permissions
            .get("groups")
            .and_then(|v| v.as_array())
            .map(|a| a.iter().filter_map(|v| v.as_str()).collect())
            .unwrap_or_default();
        assert_eq!(
            groups.len(),
            1,
            "permissions.groups should reference exactly one channel group"
        );
        assert!(
            groups[0].starts_with("slack-channel:T_TEST:"),
            "permissions.groups[0] should be slack-channel:T_TEST:<channel_id>, got {}",
            groups[0]
        );
    }

    // Verify sync run completed
    let sync_run = fixture.get_sync_run(&sync_run_id).await.unwrap().unwrap();
    assert_eq!(
        sync_run.status,
        shared::models::SyncStatus::Completed,
        "Sync run should be completed"
    );
    assert_eq!(sync_run.documents_scanned, 6);

    // Verify connector state
    let state_value = fixture
        .get_connector_state(&source_id)
        .await
        .unwrap()
        .unwrap();
    let state: SlackConnectorState = serde_json::from_value(state_value).unwrap();
    assert!(state.channel_timestamps.contains_key("C001"));
    assert!(state.channel_timestamps.contains_key("C002"));
}

#[tokio::test]
async fn test_full_sync_ignores_saved_channel_timestamps() {
    let fixture = SlackConnectorTestFixture::new().await.unwrap();

    let mock_state = MockSlackState {
        channels: make_test_channels(),
        messages: make_test_messages(BASE_TS),
        users: make_test_users(),
        channel_members: make_test_channel_members(),
        thread_replies: std::collections::HashMap::new(),
    };
    let mock_server = MockSlackServer::start(mock_state).await;

    let (_user_id, source_id, _first_sync_run_id) =
        setup_full_sync(&fixture, &mock_server.base_url).await;

    let events_before = fixture.get_queued_events(&source_id).await.unwrap();
    let events_before_count = events_before.len();

    let older_ts = BASE_TS - 86400;
    let mut messages = make_test_messages(BASE_TS);
    messages.get_mut("C001").unwrap().push(SlackMessage {
        msg_type: "message".to_string(),
        text: "A previous-day message that full sync must fetch".to_string(),
        user: "U001".to_string(),
        ts: format!("{}.000100", older_ts),
        thread_ts: None,
        reply_count: None,
        attachments: None,
        files: None,
    });

    let mock_state2 = MockSlackState {
        channels: make_test_channels(),
        messages,
        users: make_test_users(),
        channel_members: make_test_channel_members(),
        thread_replies: std::collections::HashMap::new(),
    };
    let mock_server2 = MockSlackServer::start(mock_state2).await;

    let sync_run_id_2 = fixture.create_sync_run(&source_id).await.unwrap();
    let sync_manager =
        SyncManager::with_slack_base_url(fixture.sdk_client.clone(), mock_server2.base_url.clone());
    drive_sync(
        &fixture,
        &sync_manager,
        &source_id,
        &sync_run_id_2,
        SyncType::Full,
    )
    .await;

    let events_after = fixture.get_queued_events(&source_id).await.unwrap();
    let new_events = &events_after[events_before_count..];
    let previous_day_docs: Vec<_> = new_events
        .iter()
        .filter(|e| e.get("type").and_then(|v| v.as_str()) == Some("document_created"))
        .filter(|e| {
            e.get("metadata")
                .and_then(|m| m.get("title"))
                .and_then(|v| v.as_str())
                .map(|title| title.contains("2025-01-14"))
                .unwrap_or(false)
        })
        .collect();
    assert_eq!(
        previous_day_docs.len(),
        1,
        "Full sync should ignore saved timestamps and fetch previous-day messages"
    );
}

#[tokio::test]
async fn test_sync_persists_state_for_incremental() {
    let fixture = SlackConnectorTestFixture::new().await.unwrap();

    let mock_state = MockSlackState {
        channels: make_test_channels(),
        messages: make_test_messages(BASE_TS),
        users: make_test_users(),
        channel_members: make_test_channel_members(),
        thread_replies: std::collections::HashMap::new(),
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
    let events_before = fixture.get_queued_events(&source_id).await.unwrap();
    let events_before_count = events_before.len();

    // Start a new mock with additional later messages. It also includes a
    // previous-day message that incremental sync should skip because it uses
    // the saved channel timestamp from the first sync.
    let later_ts = BASE_TS + 3600; // 1 hour later, same day
    let older_ts = BASE_TS - 86400;
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
    later_messages.get_mut("C001").unwrap().push(SlackMessage {
        msg_type: "message".to_string(),
        text: "An older message incremental should skip".to_string(),
        user: "U001".to_string(),
        ts: format!("{}.000100", older_ts),
        thread_ts: None,
        reply_count: None,
        attachments: None,
        files: None,
    });

    let mock_state2 = MockSlackState {
        channels: make_test_channels(),
        messages: later_messages,
        users: make_test_users(),
        channel_members: make_test_channel_members(),
        thread_replies: std::collections::HashMap::new(),
    };
    let mock_server2 = MockSlackServer::start(mock_state2).await;

    // Run second sync
    let sync_run_id_2 = fixture.create_sync_run(&source_id).await.unwrap();
    let sync_manager =
        SyncManager::with_slack_base_url(fixture.sdk_client.clone(), mock_server2.base_url.clone());
    drive_sync(
        &fixture,
        &sync_manager,
        &source_id,
        &sync_run_id_2,
        SyncType::Incremental,
    )
    .await;

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

    let events_after = fixture.get_queued_events(&source_id).await.unwrap();
    let new_events = &events_after[events_before_count..];
    let previous_day_docs: Vec<_> = new_events
        .iter()
        .filter(|e| e.get("type").and_then(|v| v.as_str()) == Some("document_created"))
        .filter(|e| {
            e.get("metadata")
                .and_then(|m| m.get("title"))
                .and_then(|v| v.as_str())
                .map(|title| title.contains("2025-01-14"))
                .unwrap_or(false)
        })
        .collect();
    assert!(
        previous_day_docs.is_empty(),
        "Incremental sync should use saved timestamps and skip previous-day messages"
    );

    // Verify second sync also completed
    let sync_run = fixture.get_sync_run(&sync_run_id_2).await.unwrap().unwrap();
    assert_eq!(sync_run.status, shared::models::SyncStatus::Completed);

    let events_after = fixture.get_queued_events(&source_id).await.unwrap();
    let c001_day_update = events_after[events_before_count..]
        .iter()
        .find(|event| {
            event.get("document_id").and_then(|v| v.as_str())
                == Some("slack_channel_C001_2025-01-15")
        })
        .expect("second sync should emit a full C001 day replacement");
    let content = content_for_event(&fixture, c001_day_update).await;
    assert!(
        content.contains("Hello from general!"),
        "full-day replacement should preserve earlier same-day messages: {}",
        content
    );
    assert!(
        content.contains("A new later message"),
        "full-day replacement should include the new same-day message: {}",
        content
    );
}

#[tokio::test]
async fn test_realtime_event_syncs_single_channel() {
    let fixture = SlackConnectorTestFixture::new().await.unwrap();

    let mock_state = MockSlackState {
        channels: make_test_channels(),
        messages: make_test_messages(BASE_TS),
        users: make_test_users(),
        channel_members: make_test_channel_members(),
        thread_replies: std::collections::HashMap::new(),
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
        channel_members: make_test_channel_members(),
        thread_replies: std::collections::HashMap::new(),
    };
    let mock_server2 = MockSlackServer::start(mock_state2).await;

    // Call sync_realtime_event for C001 only
    let sync_manager =
        SyncManager::with_slack_base_url(fixture.sdk_client.clone(), mock_server2.base_url.clone());

    sync_manager
        .sync_realtime_event(&source_id, "C001")
        .await
        .unwrap();

    // Verify: 2 new events for C001 — one group_membership_sync, one
    // document_updated for the message.
    let events_after = fixture.get_queued_events(&source_id).await.unwrap();
    let new_events: Vec<_> = events_after[events_before_count..].to_vec();

    let updated: Vec<_> = new_events
        .iter()
        .filter(|e| e.get("type").and_then(|v| v.as_str()) == Some("document_updated"))
        .collect();
    let group_syncs: Vec<_> = new_events
        .iter()
        .filter(|e| e.get("type").and_then(|v| v.as_str()) == Some("group_membership_sync"))
        .collect();
    assert_eq!(
        updated.len(),
        1,
        "Expected 1 document_updated event for C001"
    );
    assert_eq!(
        group_syncs.len(),
        1,
        "Expected 1 group_membership_sync for C001"
    );

    let doc_id = updated[0]
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

#[tokio::test]
async fn test_sync_fetches_thread_replies() {
    let fixture = SlackConnectorTestFixture::new().await.unwrap();

    // Add a thread parent in C001 with reply_count > 0. Replies are not
    // returned by `conversations.history` — only by `conversations.replies`,
    // which the sync must call per parent.
    let mut messages = make_test_messages(BASE_TS);
    let parent_ts = format!("{}.000500", BASE_TS);
    messages.get_mut("C001").unwrap().push(SlackMessage {
        msg_type: "message".to_string(),
        text: "Thread parent".to_string(),
        user: "U001".to_string(),
        ts: parent_ts.clone(),
        thread_ts: Some(parent_ts.clone()),
        reply_count: Some(2),
        attachments: None,
        files: None,
    });

    let mut thread_replies = std::collections::HashMap::new();
    thread_replies.insert(
        ("C001".to_string(), parent_ts.clone()),
        vec![
            SlackMessage {
                msg_type: "message".to_string(),
                text: "First reply on thread".to_string(),
                user: "U002".to_string(),
                ts: format!("{}.000600", BASE_TS),
                thread_ts: Some(parent_ts.clone()),
                reply_count: None,
                attachments: None,
                files: None,
            },
            SlackMessage {
                msg_type: "message".to_string(),
                text: "Second reply on thread".to_string(),
                user: "U001".to_string(),
                ts: format!("{}.000700", BASE_TS),
                thread_ts: Some(parent_ts.clone()),
                reply_count: None,
                attachments: None,
                files: None,
            },
        ],
    );

    let mock_state = MockSlackState {
        channels: make_test_channels(),
        messages,
        users: make_test_users(),
        channel_members: make_test_channel_members(),
        thread_replies,
    };
    let mock_server = MockSlackServer::start(mock_state).await;

    let (_user_id, source_id, sync_run_id) = setup_full_sync(&fixture, &mock_server.base_url).await;

    let events = fixture.get_queued_events(&source_id).await.unwrap();

    // Find a slack_thread_* document and verify its content includes both
    // replies (proving conversations.replies was called and merged).
    let thread_event = events
        .iter()
        .find(|e| {
            e.get("document_id")
                .and_then(|v| v.as_str())
                .map(|d| d.starts_with("slack_thread_"))
                .unwrap_or(false)
        })
        .expect("expected a slack_thread_* document_created event");

    let title = thread_event
        .get("metadata")
        .and_then(|m| m.get("title"))
        .and_then(|v| v.as_str())
        .unwrap_or("");
    assert!(
        title.starts_with("Thread in #"),
        "thread doc title should start with 'Thread in #', got {}",
        title
    );

    let url = thread_event
        .get("metadata")
        .and_then(|m| m.get("url"))
        .and_then(|v| v.as_str())
        .unwrap_or("");
    assert_eq!(
        url,
        format!(
            "https://test-team.slack.com/archives/C001/p{}",
            parent_ts.replace('.', "")
        ),
        "thread doc should use Slack web permalink"
    );
    assert!(
        !url.starts_with("slack://"),
        "thread doc should not use synthetic slack:// URL, got {}",
        url
    );

    // The doc's `slack.message_count` extra should be 3 (parent + 2 replies)
    // even though `conversations.history` only returned the parent.
    let message_count = thread_event
        .get("metadata")
        .and_then(|m| m.get("extra"))
        .and_then(|e| e.get("slack"))
        .and_then(|s| s.get("message_count"))
        .and_then(|v| v.as_u64())
        .expect("slack.message_count present");
    assert_eq!(
        message_count, 3,
        "thread should contain parent + 2 replies, got {}",
        message_count
    );

    let sync_run = fixture.get_sync_run(&sync_run_id).await.unwrap().unwrap();
    assert_eq!(
        sync_run.documents_scanned, 10,
        "sync with one thread should scan fetched channel messages plus fetched thread messages"
    );
}

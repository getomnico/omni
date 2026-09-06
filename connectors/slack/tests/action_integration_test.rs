mod mock_slack;

use mock_slack::{MockSlackServer, MockSlackState};
use omni_connector_sdk::Connector;
use omni_connector_sdk::SdkClient;
use omni_slack_connector::connector::SlackConnector;
use omni_slack_connector::models::{SlackChannel, SlackMessage, SlackUser};
use omni_slack_connector::socket::SocketModeManager;
use omni_slack_connector::sync::SyncManager;
use serde_json::json;
use shared::models::{ActionMode, AuthType, ServiceCredential, ServiceProvider};
use std::collections::HashMap;
use std::sync::Arc;
use time::OffsetDateTime;

fn empty_mock_state() -> MockSlackState {
    MockSlackState {
        channels: Vec::<SlackChannel>::new(),
        messages: HashMap::<String, Vec<SlackMessage>>::new(),
        users: Vec::<SlackUser>::new(),
        channel_members: HashMap::new(),
        thread_replies: HashMap::new(),
    }
}

fn user_credentials(auth_type: AuthType, token: &str, user_id: Option<&str>) -> ServiceCredential {
    let now = OffsetDateTime::now_utc();
    ServiceCredential {
        id: "user-credential".to_string(),
        source_id: "source-1".to_string(),
        user_id: user_id.map(str::to_string),
        provider: ServiceProvider::Slack,
        auth_type,
        principal_email: Some("caller@example.com".to_string()),
        credentials: json!({ "access_token": token }),
        config: json!({}),
        expires_at: None,
        last_validated_at: None,
        created_at: now,
        updated_at: now,
    }
}

async fn connector(base_url: String) -> SlackConnector {
    SlackConnector::with_slack_base_url(
        Arc::new(SyncManager::new(SdkClient::new("http://127.0.0.1:0"))),
        Arc::new(SocketModeManager::new()),
        base_url,
    )
}

async fn response_json(response: axum::response::Response) -> serde_json::Value {
    let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    serde_json::from_slice(&bytes).unwrap()
}

#[test]
fn slack_actions_are_writes_and_require_user_oauth_manifest() {
    let connector = SlackConnector::new(
        Arc::new(SyncManager::new(SdkClient::new("http://127.0.0.1:0"))),
        Arc::new(SocketModeManager::new()),
    );
    let actions = connector.actions();
    assert_eq!(actions.len(), 2);
    assert!(actions
        .iter()
        .all(|action| action.mode == ActionMode::Write));
    assert!(actions
        .iter()
        .all(|action| action.required_scopes == Some(vec!["chat:write".to_string()])));
    let oauth = connector.oauth_config().expect("Slack OAuth manifest");
    assert_eq!(oauth.scope_parameter, "user_scope");
    assert!(oauth.user_auth_for_writes_only);
    assert_eq!(oauth.scopes["slack"].write, vec!["chat:write"]);
}

#[tokio::test]
async fn post_message_uses_delegated_user_token() {
    let server = MockSlackServer::start(empty_mock_state()).await;
    let connector = connector(server.base_url).await;
    let response = connector
        .execute_action(
            "post_message",
            json!({"channel_id": "C001", "text": "hello"}),
            Some(user_credentials(
                AuthType::OAuth,
                "xoxp-user-token",
                Some("user-1"),
            )),
            None,
            Some("forged@example.com".to_string()),
        )
        .await
        .unwrap();
    let body = response_json(response).await;
    assert_eq!(body["status"], "success");
    assert_eq!(body["result"]["channel"], "C001");
    assert_eq!(body["result"]["message"]["text"], "hello");
}

#[tokio::test]
async fn reply_to_thread_posts_thread_timestamp() {
    let server = MockSlackServer::start(empty_mock_state()).await;
    let connector = connector(server.base_url).await;
    let response = connector
        .execute_action(
            "reply_to_thread",
            json!({"channel_id": "C001", "thread_ts": "1736942400.000100", "text": "reply"}),
            Some(user_credentials(
                AuthType::OAuth,
                "xoxp-user-token",
                Some("user-1"),
            )),
            None,
            None,
        )
        .await
        .unwrap();
    let body = response_json(response).await;
    assert_eq!(body["status"], "success");
    assert_eq!(body["result"]["message"]["thread_ts"], "1736942400.000100");
}

#[tokio::test]
async fn validation_api_errors_and_bot_fallback_are_rejected() {
    let server = MockSlackServer::start(empty_mock_state()).await;
    let connector = connector(server.base_url).await;

    let missing = connector
        .execute_action(
            "post_message",
            json!({"channel_id": "C001"}),
            Some(user_credentials(
                AuthType::OAuth,
                "xoxp-user-token",
                Some("user-1"),
            )),
            None,
            None,
        )
        .await
        .unwrap();
    let missing_body = response_json(missing).await;
    assert_eq!(missing_body["status"], "error");
    assert!(missing_body["error"].as_str().unwrap().contains("text"));

    let denied = connector
        .execute_action(
            "post_message",
            json!({"channel_id": "C_DENIED", "text": "nope"}),
            Some(user_credentials(
                AuthType::OAuth,
                "xoxp-user-token",
                Some("user-1"),
            )),
            None,
            None,
        )
        .await
        .unwrap();
    let denied_body = response_json(denied).await;
    assert_eq!(denied_body["status"], "error");
    assert!(denied_body["error"]
        .as_str()
        .unwrap()
        .contains("not_in_channel"));

    let bot = connector
        .execute_action(
            "post_message",
            json!({"channel_id": "C001", "text": "must not send"}),
            Some(user_credentials(
                AuthType::BotToken,
                "xoxb-bot-token",
                Some("user-1"),
            )),
            None,
            None,
        )
        .await
        .unwrap();
    let bot_body = response_json(bot).await;
    assert_eq!(bot_body["status"], "error");
    assert!(bot_body["error"]
        .as_str()
        .unwrap()
        .contains("delegated OAuth"));
}

use axum::{
    extract::{Query, State},
    response::Json,
    routing::{get, post},
    Router,
};
use omni_slack_connector::models::{
    AuthTestResponse, ConversationInfoResponse, ConversationsHistoryResponse,
    ConversationsListResponse, ResponseMetadata, SlackChannel, SlackMessage, SlackUser,
    UsersListResponse,
};
use serde::Deserialize;
use std::collections::HashMap;
use std::sync::Arc;
use tokio::net::TcpListener;

pub struct MockSlackState {
    pub channels: Vec<SlackChannel>,
    pub messages: HashMap<String, Vec<SlackMessage>>,
    pub users: Vec<SlackUser>,
}

pub struct MockSlackServer {
    pub base_url: String,
    _handle: tokio::task::JoinHandle<()>,
}

impl MockSlackServer {
    pub async fn start(state: MockSlackState) -> Self {
        let state = Arc::new(state);

        let app = Router::new()
            .route("/auth.test", post(auth_test))
            .route("/conversations.list", get(conversations_list))
            .route("/conversations.info", get(conversations_info))
            .route("/conversations.history", get(conversations_history))
            .route("/users.list", get(users_list))
            .route("/conversations.join", post(conversations_join))
            .with_state(state);

        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();

        let handle = tokio::spawn(async move {
            axum::serve(listener, app).await.ok();
        });

        tokio::time::sleep(tokio::time::Duration::from_millis(50)).await;

        Self {
            base_url: format!("http://{}", addr),
            _handle: handle,
        }
    }
}

async fn auth_test() -> Json<AuthTestResponse> {
    Json(AuthTestResponse {
        ok: true,
        url: "https://test-team.slack.com/".to_string(),
        team: "Test Team".to_string(),
        user: "testbot".to_string(),
        team_id: "T_TEST".to_string(),
        user_id: "U_BOT".to_string(),
        bot_id: Some("B_BOT".to_string()),
        is_enterprise_install: false,
        error: None,
    })
}

#[derive(Deserialize)]
#[allow(dead_code)]
struct ConversationsListParams {
    cursor: Option<String>,
}

async fn conversations_list(
    State(state): State<Arc<MockSlackState>>,
    Query(_params): Query<ConversationsListParams>,
) -> Json<ConversationsListResponse> {
    Json(ConversationsListResponse {
        ok: true,
        channels: state.channels.clone(),
        response_metadata: Some(ResponseMetadata { next_cursor: None }),
        error: None,
    })
}

#[derive(Deserialize)]
struct ConversationInfoParams {
    channel: String,
}

async fn conversations_info(
    State(state): State<Arc<MockSlackState>>,
    Query(params): Query<ConversationInfoParams>,
) -> Json<ConversationInfoResponse> {
    let channel = state
        .channels
        .iter()
        .find(|c| c.id == params.channel)
        .cloned()
        .unwrap_or_else(|| SlackChannel {
            id: params.channel.clone(),
            name: "unknown".to_string(),
            is_public: true,
            is_private: false,
            is_member: false,
            num_members: Some(0),
        });

    Json(ConversationInfoResponse {
        ok: true,
        channel,
        error: None,
    })
}

#[derive(Deserialize)]
#[allow(dead_code)]
struct ConversationsHistoryParams {
    channel: String,
    cursor: Option<String>,
    oldest: Option<String>,
    latest: Option<String>,
}

async fn conversations_history(
    State(state): State<Arc<MockSlackState>>,
    Query(params): Query<ConversationsHistoryParams>,
) -> Json<ConversationsHistoryResponse> {
    let messages = state
        .messages
        .get(&params.channel)
        .cloned()
        .unwrap_or_default();

    let mut filtered: Vec<SlackMessage> = messages
        .into_iter()
        .filter(|m| {
            if let Some(oldest) = &params.oldest {
                if m.ts <= *oldest {
                    return false;
                }
            }
            if let Some(latest) = &params.latest {
                if m.ts > *latest {
                    return false;
                }
            }
            true
        })
        .collect();

    // Slack returns messages in reverse chronological order (newest first)
    filtered.sort_by(|a, b| b.ts.cmp(&a.ts));

    Json(ConversationsHistoryResponse {
        ok: true,
        messages: filtered,
        has_more: false,
        response_metadata: Some(ResponseMetadata { next_cursor: None }),
        error: None,
    })
}

#[derive(Deserialize)]
#[allow(dead_code)]
struct UsersListParams {
    cursor: Option<String>,
}

async fn users_list(
    State(state): State<Arc<MockSlackState>>,
    Query(_params): Query<UsersListParams>,
) -> Json<UsersListResponse> {
    Json(UsersListResponse {
        ok: true,
        members: state.users.clone(),
        response_metadata: Some(ResponseMetadata { next_cursor: None }),
        error: None,
    })
}

async fn conversations_join() -> Json<serde_json::Value> {
    Json(serde_json::json!({ "ok": true }))
}

pub fn make_test_channels() -> Vec<SlackChannel> {
    vec![
        SlackChannel {
            id: "C001".to_string(),
            name: "general".to_string(),
            is_public: true,
            is_private: false,
            is_member: true,
            num_members: Some(10),
        },
        SlackChannel {
            id: "C002".to_string(),
            name: "engineering".to_string(),
            is_public: true,
            is_private: false,
            is_member: true,
            num_members: Some(5),
        },
    ]
}

pub fn make_test_users() -> Vec<SlackUser> {
    vec![
        SlackUser {
            id: "U001".to_string(),
            name: "alice".to_string(),
            real_name: Some("Alice Smith".to_string()),
            is_bot: false,
        },
        SlackUser {
            id: "U002".to_string(),
            name: "bob".to_string(),
            real_name: Some("Bob Jones".to_string()),
            is_bot: false,
        },
    ]
}

pub fn make_test_messages(base_ts: i64) -> HashMap<String, Vec<SlackMessage>> {
    let mut messages = HashMap::new();

    messages.insert(
        "C001".to_string(),
        vec![
            SlackMessage {
                msg_type: "message".to_string(),
                text: "Hello from general!".to_string(),
                user: "U001".to_string(),
                ts: format!("{}.000100", base_ts),
                thread_ts: None,
                reply_count: None,
                attachments: None,
                files: None,
            },
            SlackMessage {
                msg_type: "message".to_string(),
                text: "How is everyone doing?".to_string(),
                user: "U002".to_string(),
                ts: format!("{}.000200", base_ts),
                thread_ts: None,
                reply_count: None,
                attachments: None,
                files: None,
            },
            SlackMessage {
                msg_type: "message".to_string(),
                text: "Great, thanks!".to_string(),
                user: "U001".to_string(),
                ts: format!("{}.000300", base_ts),
                thread_ts: None,
                reply_count: None,
                attachments: None,
                files: None,
            },
        ],
    );

    messages.insert(
        "C002".to_string(),
        vec![
            SlackMessage {
                msg_type: "message".to_string(),
                text: "Sprint planning today".to_string(),
                user: "U001".to_string(),
                ts: format!("{}.000100", base_ts),
                thread_ts: None,
                reply_count: None,
                attachments: None,
                files: None,
            },
            SlackMessage {
                msg_type: "message".to_string(),
                text: "Will review the PRs".to_string(),
                user: "U002".to_string(),
                ts: format!("{}.000200", base_ts),
                thread_ts: None,
                reply_count: None,
                attachments: None,
                files: None,
            },
            SlackMessage {
                msg_type: "message".to_string(),
                text: "Deployment is ready".to_string(),
                user: "U001".to_string(),
                ts: format!("{}.000300", base_ts),
                thread_ts: None,
                reply_count: None,
                attachments: None,
                files: None,
            },
        ],
    );

    messages
}

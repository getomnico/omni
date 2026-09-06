use anyhow::Result;
use async_trait::async_trait;
use omni_connector_sdk::{
    ActionDefinition, ActionMode, AuthType, Connector, OAuthManifestConfig, OAuthScopeSet,
    SearchOperator, ServiceCredential, ServiceProvider, Source, SourceType, SyncContext,
    SyncRequestValidationError, SyncType,
};
use serde_json::{json, Value as JsonValue};
use std::collections::HashMap;
use std::result::Result as StdResult;
use std::sync::Arc;
use std::time::Duration;
use tracing::{info, warn};

use crate::client::SlackClient;
use crate::models::{SlackConnectorState, SlackCredentials};
use crate::socket::SocketModeManager;
use crate::sync::SyncManager;

/// Cadence for `ctx.heartbeat()` calls inside the realtime watcher. Must be
/// well below the connector-manager's `stale_sync_timeout_minutes` so a quiet
/// Socket Mode connection isn't swept as a dead sync.
const REALTIME_HEARTBEAT_INTERVAL: Duration = Duration::from_secs(30);

pub struct SlackConnector {
    sync_manager: Arc<SyncManager>,
    socket_manager: Arc<SocketModeManager>,
    slack_client: SlackClient,
}

impl SlackConnector {
    pub fn new(sync_manager: Arc<SyncManager>, socket_manager: Arc<SocketModeManager>) -> Self {
        Self {
            sync_manager,
            socket_manager,
            slack_client: SlackClient::new(),
        }
    }

    pub fn with_slack_base_url(
        sync_manager: Arc<SyncManager>,
        socket_manager: Arc<SocketModeManager>,
        base_url: String,
    ) -> Self {
        Self {
            sync_manager,
            socket_manager,
            slack_client: SlackClient::with_base_url(base_url),
        }
    }

    /// Long-running watcher invoked when the connector-manager triggers a
    /// `Realtime` sync. Maintains a Socket Mode connection for the source
    /// until the sync is cancelled; per-channel work is driven from the
    /// Socket Mode handler (see `socket::handle_event`).
    async fn run_realtime(&self, creds: ServiceCredential, ctx: SyncContext) -> Result<()> {
        let source_id = ctx.source_id().to_string();
        let creds: SlackCredentials = serde_json::from_value(creds.credentials)
            .map_err(|e| anyhow::anyhow!("Failed to decode Slack credentials: {}", e))?;
        let app_token = creds
            .app_token
            .filter(|token| !token.trim().is_empty())
            .ok_or_else(|| {
                anyhow::anyhow!("Slack realtime sync requires `app_token` in service credentials")
            })?;

        info!(source_id, "Starting Slack realtime watcher");
        self.socket_manager
            .start_connection(
                source_id.clone(),
                app_token,
                self.sync_manager.sdk_client().clone(),
                Some(self.sync_manager.clone()),
            )
            .await;

        let mut heartbeat_ticker = tokio::time::interval(REALTIME_HEARTBEAT_INTERVAL);
        heartbeat_ticker.tick().await;
        while !ctx.is_cancelled() {
            tokio::select! {
                _ = heartbeat_ticker.tick() => {
                    if let Err(e) = ctx.heartbeat().await {
                        warn!(source_id, error = %e, "Realtime heartbeat failed");
                    }
                }
                _ = tokio::time::sleep(Duration::from_millis(500)) => {}
            }
        }

        self.socket_manager.stop_connection(&source_id).await;
        info!(source_id, "Slack realtime watcher stopped");
        ctx.cancel().await?;
        Ok(())
    }
}

#[async_trait]
impl Connector for SlackConnector {
    type Config = JsonValue;
    type Credentials = SlackCredentials;
    type State = SlackConnectorState;

    fn name(&self) -> &'static str {
        "slack"
    }

    fn version(&self) -> &'static str {
        env!("CARGO_PKG_VERSION")
    }

    fn display_name(&self) -> String {
        "Slack".to_string()
    }

    fn description(&self) -> Option<String> {
        Some("Connect to Slack messages and files".to_string())
    }

    fn source_types(&self) -> Vec<SourceType> {
        vec![SourceType::Slack]
    }

    fn sync_modes(&self) -> Vec<SyncType> {
        vec![SyncType::Full, SyncType::Incremental, SyncType::Realtime]
    }

    fn search_operators(&self) -> Vec<SearchOperator> {
        vec![SearchOperator {
            operator: "channel".to_string(),
            attribute_key: "channel_name".to_string(),
            value_type: "text".to_string(),
        }]
    }

    fn actions(&self) -> Vec<ActionDefinition> {
        let write_scope = Some(vec!["chat:write".to_string()]);
        vec![
            ActionDefinition {
                name: "post_message".to_string(),
                description: "Post a message to a Slack channel as the requesting user".to_string(),
                input_schema: json!({
                    "type": "object",
                    "properties": {
                        "channel_id": { "type": "string", "description": "Slack channel ID" },
                        "text": { "type": "string", "description": "Message text" }
                    },
                    "required": ["channel_id", "text"]
                }),
                mode: ActionMode::Write,
                required_scopes: write_scope.clone(),
                source_types: vec![SourceType::Slack],
                admin_only: false,
                hidden: false,
            },
            ActionDefinition {
                name: "reply_to_thread".to_string(),
                description: "Reply to an existing Slack channel thread as the requesting user"
                    .to_string(),
                input_schema: json!({
                    "type": "object",
                    "properties": {
                        "channel_id": { "type": "string", "description": "Slack channel ID" },
                        "thread_ts": { "type": "string", "description": "Timestamp of the thread parent" },
                        "text": { "type": "string", "description": "Reply text" }
                    },
                    "required": ["channel_id", "thread_ts", "text"]
                }),
                mode: ActionMode::Write,
                required_scopes: write_scope,
                source_types: vec![SourceType::Slack],
                admin_only: false,
                hidden: false,
            },
        ]
    }

    fn oauth_config(&self) -> Option<OAuthManifestConfig> {
        let mut scopes = HashMap::new();
        scopes.insert(
            "slack".to_string(),
            OAuthScopeSet {
                read: vec![],
                write: vec!["chat:write".to_string()],
            },
        );
        Some(OAuthManifestConfig {
            provider: "slack".to_string(),
            auth_endpoint: "https://slack.com/oauth/v2/authorize".to_string(),
            token_endpoint: "https://slack.com/api/oauth.v2.access".to_string(),
            // Slack OAuth v2 returns the delegated token but not an email.
            // The callback binds it to the already-authenticated Omni user.
            userinfo_endpoint: "".to_string(),
            userinfo_email_field: "email".to_string(),
            identity_scopes: vec![],
            scopes,
            extra_auth_params: HashMap::new(),
            scope_separator: " ".to_string(),
            scope_parameter: "user_scope".to_string(),
            user_auth_for_writes_only: true,
            enrich_endpoint: None,
            registration_endpoint: None,
            token_endpoint_auth_method:
                omni_connector_sdk::OAuthTokenEndpointAuthMethod::ClientSecretPost,
            resource: None,
        })
    }

    async fn validate_sync_request(
        &self,
        _source: &Source,
        credentials: Option<&ServiceCredential>,
        sync_type: SyncType,
    ) -> StdResult<(), SyncRequestValidationError> {
        if sync_type != SyncType::Realtime {
            return Ok(());
        }

        let Some(credentials) = credentials else {
            return Err(SyncRequestValidationError::BadRequest(
                "Slack realtime sync requires credentials".to_string(),
            ));
        };
        let creds: SlackCredentials =
            serde_path_to_error::deserialize(credentials.credentials.clone()).map_err(|e| {
                SyncRequestValidationError::BadRequest(format!(
                    "Failed to decode Slack credentials: {}",
                    e
                ))
            })?;

        if creds
            .app_token
            .as_deref()
            .is_some_and(|token| !token.trim().is_empty())
        {
            Ok(())
        } else {
            Err(SyncRequestValidationError::Unavailable(
                "Slack realtime sync is not available because no app token is configured"
                    .to_string(),
            ))
        }
    }

    async fn execute_action(
        &self,
        action: &str,
        params: JsonValue,
        credentials: Option<ServiceCredential>,
        source: Option<Source>,
        _actor_email: Option<String>,
    ) -> Result<axum::response::Response> {
        let thread_required = match action {
            "post_message" => false,
            "reply_to_thread" => true,
            other => {
                return Ok(omni_connector_sdk::ActionResponse::not_supported(other)
                    .into_response_with_status(axum::http::StatusCode::NOT_FOUND));
            }
        };

        if source
            .as_ref()
            .and_then(|source| source.config.get("read_only"))
            .and_then(JsonValue::as_bool)
            == Some(true)
        {
            return Ok(omni_connector_sdk::ActionResponse::failure(
                "Slack action is not allowed: source is read-only",
            )
            .into_response());
        }

        let channel_id = match required_action_string(&params, "channel_id") {
            Ok(value) => value,
            Err(error) => {
                return Ok(
                    omni_connector_sdk::ActionResponse::failure(error.to_string()).into_response(),
                )
            }
        };
        let text = match required_action_string(&params, "text") {
            Ok(value) => value,
            Err(error) => {
                return Ok(
                    omni_connector_sdk::ActionResponse::failure(error.to_string()).into_response(),
                )
            }
        };
        let thread_ts = if thread_required {
            match required_action_string(&params, "thread_ts") {
                Ok(value) => Some(value),
                Err(error) => {
                    return Ok(
                        omni_connector_sdk::ActionResponse::failure(error.to_string())
                            .into_response(),
                    )
                }
            }
        } else {
            None
        };
        let token = match delegated_user_token(credentials) {
            Ok(value) => value,
            Err(error) => {
                return Ok(
                    omni_connector_sdk::ActionResponse::failure(error.to_string()).into_response(),
                )
            }
        };
        let response = self
            .slack_client
            .post_message(&token, &channel_id, &text, thread_ts.as_deref())
            .await;

        match response {
            Ok(response) => Ok(
                omni_connector_sdk::ActionResponse::success(serde_json::to_value(response)?)
                    .into_response(),
            ),
            Err(error) => {
                Ok(omni_connector_sdk::ActionResponse::failure(error.to_string()).into_response())
            }
        }
    }

    async fn sync(
        &self,
        source: Source,
        credentials: Option<ServiceCredential>,
        state: Option<Self::State>,
        ctx: SyncContext,
    ) -> Result<()> {
        let creds =
            credentials.ok_or_else(|| anyhow::anyhow!("Slack sync requires credentials"))?;

        match ctx.sync_mode() {
            SyncType::Full | SyncType::Incremental => {
                self.sync_manager.run_sync(source, creds, state, ctx).await
            }
            SyncType::Realtime => self.run_realtime(creds, ctx).await,
        }
    }

    async fn cancel(&self, _sync_run_id: &str) -> bool {
        // SDK owns the cancellation flag (exposed via SyncContext); just ack.
        true
    }
}

fn required_action_string(params: &JsonValue, name: &str) -> Result<String> {
    params
        .get(name)
        .and_then(JsonValue::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToString::to_string)
        .ok_or_else(|| anyhow::anyhow!("Missing required parameter: {name}"))
}

fn delegated_user_token(credentials: Option<ServiceCredential>) -> Result<String> {
    let credentials =
        credentials.ok_or_else(|| anyhow::anyhow!("Slack write action requires credentials"))?;
    if credentials.provider != ServiceProvider::Slack {
        return Err(anyhow::anyhow!(
            "Slack write action requires Slack credentials"
        ));
    }
    if credentials.auth_type != AuthType::OAuth || credentials.user_id.is_none() {
        return Err(anyhow::anyhow!(
            "Slack write action requires a per-user delegated OAuth credential"
        ));
    }
    let token = credentials
        .credentials
        .get("access_token")
        .and_then(JsonValue::as_str)
        .ok_or_else(|| anyhow::anyhow!("Slack OAuth credential is missing access_token"))?;
    if !token.starts_with("xoxp-") {
        return Err(anyhow::anyhow!(
            "Slack OAuth credential does not contain a delegated user token"
        ));
    }
    Ok(token.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use omni_connector_sdk::SdkClient;
    use serde_json::json;
    use shared::models::{AuthType, IntegrationType, ServiceProvider, SourceScope, UserFilterMode};
    use time::OffsetDateTime;

    fn connector() -> SlackConnector {
        SlackConnector::new(
            Arc::new(SyncManager::new(SdkClient::new("http://127.0.0.1:0"))),
            Arc::new(SocketModeManager::new()),
        )
    }

    fn source() -> Source {
        let now = OffsetDateTime::now_utc();
        Source {
            id: "source-1".to_string(),
            name: "Slack".to_string(),
            source_type: SourceType::Slack.to_string(),
            integration_type: IntegrationType::Connector,
            config: json!({}),
            is_active: true,
            is_deleted: false,
            scope: SourceScope::Org,
            user_filter_mode: UserFilterMode::All,
            user_whitelist: None,
            user_blacklist: None,
            connector_state: None,
            checkpoint: None,
            sync_interval_seconds: Some(3600),
            created_at: now,
            updated_at: now,
            created_by: "01JGF7V3E0Y2R1X8P5Q7W9T4N6".to_string(),
        }
    }

    fn credentials(credentials: serde_json::Value) -> ServiceCredential {
        let now = OffsetDateTime::now_utc();
        ServiceCredential {
            id: "creds-1".to_string(),
            source_id: "source-1".to_string(),
            user_id: None,
            provider: ServiceProvider::Slack,
            auth_type: AuthType::BotToken,
            principal_email: None,
            credentials,
            config: json!({}),
            expires_at: None,
            last_validated_at: None,
            created_at: now,
            updated_at: now,
        }
    }

    #[test]
    fn delegated_token_rejects_shared_bot_credentials() {
        let creds = credentials(json!({ "bot_token": "xoxb-test", "access_token": "xoxp-user" }));
        assert!(delegated_user_token(Some(creds)).is_err());
    }

    #[tokio::test]
    async fn write_action_respects_read_only_source_config() {
        let connector = connector();
        let mut source = source();
        source.config = json!({ "read_only": true });
        let response = connector
            .execute_action(
                "post_message",
                json!({ "channel_id": "C001", "text": "blocked" }),
                None,
                Some(source),
                None,
            )
            .await
            .unwrap();
        let body = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        let body: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(body["status"], "error");
        assert!(body["error"].as_str().unwrap().contains("read-only"));
    }

    #[tokio::test]
    async fn realtime_requires_app_token() {
        let connector = connector();
        let source = source();
        let creds = credentials(json!({ "bot_token": "xoxb-test" }));

        assert!(
            connector
                .validate_sync_request(&source, Some(&creds), SyncType::Realtime)
                .await
                .is_err()
        );
        assert!(
            connector
                .validate_sync_request(&source, Some(&creds), SyncType::Full)
                .await
                .is_ok()
        );
    }

    #[tokio::test]
    async fn malformed_credentials_are_bad_request() {
        let connector = connector();
        let source = source();
        let creds = credentials(json!({ "bot_token": 123 }));

        let error = connector
            .validate_sync_request(&source, Some(&creds), SyncType::Realtime)
            .await
            .expect_err("malformed credentials should fail before starting");

        assert!(matches!(error, SyncRequestValidationError::BadRequest(_)));
    }
}

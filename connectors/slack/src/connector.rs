use std::collections::HashMap;
use std::result::Result as StdResult;
use std::sync::Arc;
use std::time::Duration;

use anyhow::Result;
use async_trait::async_trait;
use omni_connector_sdk::{
    AuthType, Connector, HttpMcpServer, McpCredentials, McpServer, OAuthManifestConfig,
    OAuthScopeSet, OAuthTokenEndpointAuthMethod, SearchOperator, ServiceCredential, Source,
    SourceType, SyncContext, SyncRequestValidationError, SyncType,
};
use serde_json::Value as JsonValue;
use tracing::{info, warn};

use crate::models::{SlackConnectorState, SlackCredentials};
use crate::socket::SocketModeManager;
use crate::sync::SyncManager;

/// Cadence for `ctx.heartbeat()` calls inside the realtime watcher. Must be
/// well below the connector-manager's `stale_sync_timeout_minutes` so a quiet
/// Socket Mode connection isn't swept as a dead sync.
const REALTIME_HEARTBEAT_INTERVAL: Duration = Duration::from_secs(30);

/// Slack's official hosted MCP server. AI actions (search, read, send, …) run
/// through it under each user's delegated OAuth identity; the org bot/app
/// tokens below only power indexing and realtime sync.
const SLACK_MCP_URL: &str = "https://mcp.slack.com/mcp";

pub struct SlackConnector {
    sync_manager: Arc<SyncManager>,
    socket_manager: Arc<SocketModeManager>,
}

impl SlackConnector {
    pub fn new(sync_manager: Arc<SyncManager>, socket_manager: Arc<SocketModeManager>) -> Self {
        Self {
            sync_manager,
            socket_manager,
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

    /// User-token scopes required by Slack's hosted MCP tools. Slack gates
    /// `tools/list` on the token's granted scopes, so the requested set must
    /// cover every tool Omni exposes (see the scope table in Slack's MCP docs:
    /// https://docs.slack.dev/ai/slack-mcp-server). Read scopes unlock the
    /// search/read tools; write scopes unlock send/conversation/canvas tools.
    fn slack_scopes() -> OAuthScopeSet {
        OAuthScopeSet {
            read: vec![
                "search:read.public".to_string(),
                "search:read.private".to_string(),
                "search:read.im".to_string(),
                "search:read.mpim".to_string(),
                "search:read.files".to_string(),
                "search:read.users".to_string(),
                "files:read".to_string(),
                "emoji:read".to_string(),
                "users:read".to_string(),
                "users:read.email".to_string(),
                "channels:history".to_string(),
                "groups:history".to_string(),
                "im:history".to_string(),
                "mpim:history".to_string(),
                "channels:read".to_string(),
                "groups:read".to_string(),
                "im:read".to_string(),
                "mpim:read".to_string(),
                "canvases:read".to_string(),
                "reactions:read".to_string(),
                "lists:read".to_string(),
            ],
            write: vec![
                "chat:write".to_string(),
                "channels:write".to_string(),
                "groups:write".to_string(),
                "im:write".to_string(),
                "mpim:write".to_string(),
                "reactions:write".to_string(),
                "canvases:write".to_string(),
                "files:write".to_string(),
                "lists:write".to_string(),
            ],
        }
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

    fn mcp_server(&self) -> Option<McpServer> {
        Some(McpServer::Http(HttpMcpServer::new(SLACK_MCP_URL.to_string())))
    }

    async fn prepare_mcp_headers(
        &self,
        credentials: &McpCredentials,
    ) -> Result<HashMap<String, String>> {
        if credentials.auth_type != Some(AuthType::OAuth) {
            return Err(anyhow::anyhow!(
                "Slack MCP requires a per-user delegated OAuth credential; bot/app tokens only power sync"
            ));
        }
        let token = credentials
            .credentials
            .get("access_token")
            .and_then(JsonValue::as_str)
            .ok_or_else(|| anyhow::anyhow!("Slack OAuth credential is missing access_token"))?;
        if !token.starts_with("xoxp-") {
            return Err(anyhow::anyhow!(
                "Slack OAuth credential does not contain a delegated user token (xoxp-*)"
            ));
        }
        Ok(HashMap::from([(
            "Authorization".to_string(),
            format!("Bearer {token}"),
        )]))
    }

    fn oauth_config(&self) -> Option<OAuthManifestConfig> {
        let mut scopes = HashMap::new();
        scopes.insert("slack".to_string(), Self::slack_scopes());
        Some(OAuthManifestConfig {
            provider: "slack".to_string(),
            // Slack's user-token-only endpoints (the hosted MCP server does not
            // support dynamic client registration; admins configure the OAuth
            // client under Settings → Integrations → OAuth Apps).
            auth_endpoint: "https://slack.com/oauth/v2_user/authorize".to_string(),
            token_endpoint: "https://slack.com/api/oauth.v2.user.access".to_string(),
            // Slack returns no usable userinfo; the callback binds the
            // credential to the already-authenticated Omni user.
            userinfo_endpoint: None,
            userinfo_email_field: "email".to_string(),
            identity_scopes: vec![],
            scopes,
            extra_auth_params: HashMap::new(),
            // Slack's authorize endpoint expects a comma-separated scope list;
            // token responses echo granted scopes back comma-separated.
            scope_separator: ",".to_string(),
            enrich_endpoint: None,
            registration_endpoint: None,
            registration_requires_initial_access_token: false,
            token_response_fields: Vec::new(),
            token_endpoint_auth_method: OAuthTokenEndpointAuthMethod::ClientSecretPost,
            resource: None,
            issuer_source_config_key: None,
            client_config_provider_template: None,
            pkce_required: false,
            grant_types: None,
            validate_endpoint_urls: false,
            supports_org_oauth: true,
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

#[cfg(test)]
mod tests {
    use super::*;
    use omni_connector_sdk::SdkClient;
    use serde_json::json;
    use shared::models::{IntegrationType, ServiceProvider, SourceScope, UserFilterMode};
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

    fn mcp_credentials(auth_type: Option<AuthType>, credentials: serde_json::Value) -> McpCredentials {
        McpCredentials {
            credentials,
            config: json!({}),
            auth_type,
            principal_email: Some("caller@example.com".to_string()),
        }
    }

    #[test]
    fn hosted_mcp_and_user_oauth_manifest_are_declared() {
        let connector = connector();

        let server = connector.mcp_server().expect("Slack must expose its hosted MCP server");
        match server {
            McpServer::Http(http) => assert_eq!(http.url, "https://mcp.slack.com/mcp"),
            McpServer::Stdio(_) => panic!("Slack MCP must be reached over HTTP"),
        }

        // No native actions: the whole AI tool surface comes from Slack's MCP.
        assert!(connector.actions().is_empty());

        let oauth = connector.oauth_config().expect("Slack OAuth manifest");
        assert_eq!(
            oauth.auth_endpoint,
            "https://slack.com/oauth/v2_user/authorize"
        );
        assert_eq!(
            oauth.token_endpoint,
            "https://slack.com/api/oauth.v2.user.access"
        );
        assert!(oauth.registration_endpoint.is_none());
        assert_eq!(
            oauth.token_endpoint_auth_method,
            OAuthTokenEndpointAuthMethod::ClientSecretPost
        );
        assert_eq!(oauth.scope_separator, ",");
        assert_eq!(oauth.provider, "slack");

        let scopes = &oauth.scopes["slack"];
        assert!(scopes.write.contains(&"chat:write".to_string()));
        assert!(scopes.write.contains(&"channels:write".to_string()));
        assert!(scopes.read.contains(&"channels:history".to_string()));
        assert!(scopes.read.contains(&"search:read.public".to_string()));
        // No overlap between the partitions; writes include every mutating tool.
        for scope in &scopes.write {
            assert!(!scopes.read.contains(scope), "scope listed in both sets: {scope}");
        }
    }

    #[tokio::test]
    async fn mcp_headers_require_delegated_user_oauth() {
        let connector = connector();

        let headers = connector
            .prepare_mcp_headers(&mcp_credentials(
                Some(AuthType::OAuth),
                json!({ "access_token": "xoxp-user-token" }),
            ))
            .await
            .expect("delegated user token must authenticate");
        assert_eq!(
            headers.get("Authorization").map(String::as_str),
            Some("Bearer xoxp-user-token")
        );

        // Bot/app tokens (org sync credentials) must never reach Slack's MCP.
        for (auth_type, credentials) in [
            (Some(AuthType::BotToken), json!({ "access_token": "xoxp-ignored" })),
            (Some(AuthType::OAuth), json!({ "access_token": "xoxb-bot-token" })),
            (Some(AuthType::OAuth), json!({ "access_token": "xapp-app-token" })),
            (Some(AuthType::OAuth), json!({ "access_token": "not-a-slack-token" })),
            (Some(AuthType::OAuth), json!({ "bot_token": "xoxb-bot" })),
            (None, json!({ "access_token": "xoxp-user-token" })),
        ] {
            assert!(
                connector
                    .prepare_mcp_headers(&mcp_credentials(auth_type, credentials.clone()))
                    .await
                    .is_err(),
                "expected rejection for auth_type={auth_type:?} credentials={credentials}"
            );
        }
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

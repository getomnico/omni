use anyhow::Result;
use async_trait::async_trait;
use omni_connector_sdk::{
    Connector, SearchOperator, ServiceCredential, Source, SourceType, SyncContext,
    SyncRequestValidationError, SyncType,
};
use serde_json::Value as JsonValue;
use std::result::Result as StdResult;
use std::sync::Arc;
use std::time::Duration;
use tracing::{info, warn};

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
    use shared::models::{AuthType, ServiceProvider, SourceScope, UserFilterMode};
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
            source_type: SourceType::Slack,
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

    #[tokio::test]
    async fn realtime_requires_app_token() {
        let connector = connector();
        let source = source();
        let creds = credentials(json!({ "bot_token": "xoxb-test" }));

        assert!(connector
            .validate_sync_request(&source, Some(&creds), SyncType::Realtime)
            .await
            .is_err());
        assert!(connector
            .validate_sync_request(&source, Some(&creds), SyncType::Full)
            .await
            .is_ok());
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

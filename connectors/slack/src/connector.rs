use anyhow::{anyhow, Result};
use async_trait::async_trait;
use omni_connector_sdk::{
    Connector, SearchOperator, ServiceCredential, Source, SourceType, SyncContext, SyncType,
};
use serde_json::Value as JsonValue;
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
            .map_err(|e| anyhow!("Failed to decode Slack credentials: {}", e))?;
        let app_token = creds.app_token.ok_or_else(|| {
            anyhow!("Slack realtime sync requires `app_token` in service credentials")
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

    async fn sync(
        &self,
        source: Source,
        credentials: Option<ServiceCredential>,
        state: Option<Self::State>,
        ctx: SyncContext,
    ) -> Result<()> {
        let creds = credentials.ok_or_else(|| anyhow!("Slack sync requires credentials"))?;

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

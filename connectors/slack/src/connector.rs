use anyhow::{anyhow, Result};
use async_trait::async_trait;
use omni_connector_sdk::{
    Connector, SearchOperator, ServiceCredential, Source, SourceType, SyncContext, SyncType,
};
use serde_json::Value as JsonValue;
use shared::SdkClient;
use std::sync::Arc;
use tracing::{debug, info};

use crate::models::SlackConnectorState;
use crate::socket::SocketModeManager;
use crate::sync::SyncManager;

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
}

#[async_trait]
impl Connector for SlackConnector {
    type Config = JsonValue;
    type Credentials = JsonValue;
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
        vec![SyncType::Full, SyncType::Incremental]
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
        let source_id = ctx.source_id().to_string();
        self.sync_manager
            .run_sync(source, creds, state, ctx)
            .await?;

        // After a successful sync, kick off Socket Mode for live updates if not
        // already connected. Failures here only log — the sync itself succeeded.
        if !self.socket_manager.is_connected(&source_id).await {
            start_socket_for_source(
                &source_id,
                self.sync_manager.sdk_client(),
                &self.socket_manager,
                Some(self.sync_manager.clone()),
            )
            .await;
        }
        Ok(())
    }

    async fn cancel(&self, _sync_run_id: &str) -> bool {
        // SDK owns the cancellation flag (exposed via SyncContext); just ack.
        true
    }
}

/// Start a Socket Mode connection for a source, if it has an `app_token`
/// configured. Used both on connector startup (to reconnect existing sources)
/// and after a successful sync.
pub async fn start_socket_for_source(
    source_id: &str,
    sdk_client: &SdkClient,
    socket_manager: &SocketModeManager,
    sync_manager: Option<Arc<SyncManager>>,
) {
    let app_token = match get_app_token(source_id, sdk_client).await {
        Some(token) => token,
        None => return,
    };

    info!(source_id, "Starting Socket Mode connection");
    socket_manager
        .start_connection(
            source_id.to_string(),
            app_token,
            sdk_client.clone(),
            sync_manager,
        )
        .await;
}

async fn get_app_token(source_id: &str, sdk_client: &SdkClient) -> Option<String> {
    let creds = match sdk_client.get_credentials(source_id).await {
        Ok(c) => c,
        Err(e) => {
            debug!("Could not fetch credentials for {}: {}", source_id, e);
            return None;
        }
    };

    creds
        .credentials
        .get("app_token")
        .and_then(|v| v.as_str())
        .map(|s| s.to_string())
}

use anyhow::Result;
use async_trait::async_trait;
use omni_connector_sdk::{Connector, SourceType, SyncContext, SyncType};
use serde::{Deserialize, Serialize};
use serde_json::Value as JsonValue;

use crate::client::FirefliesClient;
use crate::sync::run_sync;

#[derive(Debug, Deserialize)]
pub struct FirefliesCredentials {
    pub api_key: String,
}

#[derive(Debug, Default, Deserialize, Serialize)]
pub struct FirefliesState {
    pub last_sync_time: Option<String>,
}

pub struct FirefliesConnector {
    client: FirefliesClient,
}

impl FirefliesConnector {
    pub fn new() -> Self {
        Self {
            client: FirefliesClient::new(),
        }
    }
}

impl Default for FirefliesConnector {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl Connector for FirefliesConnector {
    type Config = JsonValue;
    type Credentials = FirefliesCredentials;
    type State = FirefliesState;

    fn name(&self) -> &'static str {
        "fireflies"
    }

    fn version(&self) -> &'static str {
        env!("CARGO_PKG_VERSION")
    }

    fn display_name(&self) -> String {
        "Fireflies".to_string()
    }

    fn description(&self) -> Option<String> {
        Some("Index meeting transcripts from Fireflies.ai".to_string())
    }

    fn source_types(&self) -> Vec<SourceType> {
        vec![SourceType::Fireflies]
    }

    fn sync_modes(&self) -> Vec<SyncType> {
        vec![SyncType::Full, SyncType::Incremental]
    }

    async fn sync(
        &self,
        _source_config: Self::Config,
        credentials: Self::Credentials,
        state: Option<Self::State>,
        ctx: SyncContext,
    ) -> Result<()> {
        run_sync(&self.client, &credentials.api_key, state, ctx).await
    }

    async fn cancel(&self, _sync_run_id: &str) -> bool {
        // SDK owns the cancellation flag (exposed via SyncContext); just ack.
        true
    }
}

use anyhow::Result;
use async_trait::async_trait;
use omni_connector_sdk::{Connector, SdkClient, SourceType, SyncContext, SyncMode};
use serde_json::Value as JsonValue;
use std::sync::Arc;

use crate::config::WebSourceConfig;
use crate::models::WebConnectorState;
use crate::sync::{PageSource, SyncManager};

pub struct WebConnector {
    sync_manager: SyncManager,
}

impl WebConnector {
    pub fn new(sdk_client: SdkClient) -> Self {
        Self {
            sync_manager: SyncManager::new(sdk_client),
        }
    }

    pub fn with_page_source(sdk_client: SdkClient, page_source: Arc<dyn PageSource>) -> Self {
        Self {
            sync_manager: SyncManager::with_page_source(sdk_client, page_source),
        }
    }
}

#[async_trait]
impl Connector for WebConnector {
    type Config = WebSourceConfig;
    type Credentials = JsonValue;
    type State = WebConnectorState;

    fn name(&self) -> &'static str {
        "web"
    }

    fn version(&self) -> &'static str {
        env!("CARGO_PKG_VERSION")
    }

    fn source_types(&self) -> Vec<SourceType> {
        vec![SourceType::Web]
    }

    fn display_name(&self) -> String {
        "Web".to_string()
    }

    fn description(&self) -> Option<String> {
        Some("Index content from websites and documentation sites".to_string())
    }

    fn sync_modes(&self) -> Vec<SyncMode> {
        vec![SyncMode::Full, SyncMode::Incremental]
    }

    fn requires_credentials(&self) -> bool {
        false
    }

    async fn sync(
        &self,
        source_config: WebSourceConfig,
        _credentials: JsonValue,
        state: Option<WebConnectorState>,
        ctx: SyncContext,
    ) -> Result<()> {
        self.sync_manager.run_sync(source_config, state, ctx).await
    }
}

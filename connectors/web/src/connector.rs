use anyhow::Result;
use async_trait::async_trait;
use omni_connector_sdk::{Connector, SourceType, SyncContext};
use redis::Client as RedisClient;
use serde_json::Value as JsonValue;
use std::sync::Arc;

use crate::config::WebSourceConfig;
use crate::models::WebConnectorState;
use crate::sync::{PageSource, SyncManager};

pub struct WebConnector {
    sync_manager: SyncManager,
}

impl WebConnector {
    pub fn new(redis_client: RedisClient, sdk_client: shared::SdkClient) -> Self {
        Self {
            sync_manager: SyncManager::new(redis_client, sdk_client),
        }
    }

    pub fn with_page_source(
        redis_client: RedisClient,
        sdk_client: shared::SdkClient,
        page_source: Arc<dyn PageSource>,
    ) -> Self {
        Self {
            sync_manager: SyncManager::with_page_source(redis_client, sdk_client, page_source),
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

    fn sync_modes(&self) -> Vec<String> {
        vec!["full".to_string(), "incremental".to_string()]
    }

    fn requires_credentials(&self) -> bool {
        false
    }

    async fn sync(
        &self,
        source_config: WebSourceConfig,
        _credentials: JsonValue,
        _state: Option<WebConnectorState>,
        ctx: SyncContext,
    ) -> Result<()> {
        self.sync_manager.run_sync(source_config, ctx).await
    }
}

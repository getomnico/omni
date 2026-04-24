use crate::context::SyncContext;
use crate::models::ActionResult;
use anyhow::Result;
use async_trait::async_trait;
use serde::de::DeserializeOwned;
use serde::Serialize;
use serde_json::Value as JsonValue;
use shared::models::{
    ActionDefinition, ConnectorManifest, McpPromptDefinition, McpResourceDefinition,
    SearchOperator, SourceType, SyncType,
};

#[async_trait]
pub trait Connector: Send + Sync + 'static {
    type Config: DeserializeOwned + Send + 'static;
    type Credentials: DeserializeOwned + Send + 'static;
    type State: DeserializeOwned + Serialize + Send + 'static;

    fn name(&self) -> &'static str;
    fn version(&self) -> &'static str;
    fn source_types(&self) -> Vec<SourceType>;

    fn display_name(&self) -> String {
        self.name().to_string()
    }

    fn description(&self) -> Option<String> {
        None
    }

    fn sync_modes(&self) -> Vec<SyncType> {
        vec![SyncType::Full]
    }

    fn actions(&self) -> Vec<ActionDefinition> {
        vec![]
    }

    fn search_operators(&self) -> Vec<SearchOperator> {
        vec![]
    }

    fn read_only(&self) -> bool {
        false
    }

    fn requires_credentials(&self) -> bool {
        true
    }

    fn extra_schema(&self) -> Option<JsonValue> {
        None
    }

    fn attributes_schema(&self) -> Option<JsonValue> {
        None
    }

    fn mcp_enabled(&self) -> bool {
        false
    }

    fn mcp_resources(&self) -> Vec<McpResourceDefinition> {
        vec![]
    }

    fn mcp_prompts(&self) -> Vec<McpPromptDefinition> {
        vec![]
    }

    async fn sync(
        &self,
        source_config: Self::Config,
        credentials: Self::Credentials,
        state: Option<Self::State>,
        ctx: SyncContext,
    ) -> Result<()>;

    async fn cancel(&self, _sync_run_id: &str) -> bool {
        false
    }

    async fn execute_action(
        &self,
        action: &str,
        _params: JsonValue,
        _credentials: JsonValue,
    ) -> Result<ActionResult> {
        Ok(ActionResult::not_supported(action))
    }

    async fn build_manifest(&self, connector_url: String) -> ConnectorManifest {
        ConnectorManifest {
            name: self.name().to_string(),
            display_name: self.display_name(),
            version: self.version().to_string(),
            sync_modes: self.sync_modes(),
            connector_id: self.name().to_string(),
            connector_url,
            source_types: self.source_types(),
            description: self.description(),
            actions: self.actions(),
            search_operators: self.search_operators(),
            read_only: self.read_only(),
            extra_schema: self.extra_schema(),
            attributes_schema: self.attributes_schema(),
            mcp_enabled: self.mcp_enabled(),
            resources: self.mcp_resources(),
            prompts: self.mcp_prompts(),
        }
    }
}

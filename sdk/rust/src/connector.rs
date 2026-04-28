use crate::context::SyncContext;
use crate::models::ActionResponse;
use crate::models::OAuthManifestConfig;
use anyhow::Result;
use async_trait::async_trait;
use axum::http::StatusCode;
use axum::response::Response;
use serde::de::DeserializeOwned;
use serde::Serialize;
use serde_json::Value as JsonValue;
use shared::models::{
    ActionDefinition, ConnectorManifest, McpPromptDefinition, McpResourceDefinition,
    SearchOperator, ServiceCredential, Source, SourceType, SyncType,
};

#[async_trait]
pub trait Connector: Send + Sync + 'static {
    /// Shape of `source.config`. Used by the SDK to validate the config blob
    /// at `/sync` dispatch — a decode failure rejects the request with 400
    /// before any sync run is recorded. The decoded value is discarded; the
    /// connector receives the full `Source` and decodes its own typed view
    /// inside `sync()` if it needs one. Use `serde_json::Value` for connectors
    /// that don't want validation.
    type Config: DeserializeOwned + Send + 'static;
    /// Shape of `service_credentials.credentials`. Validated the same way as
    /// `Config` — see above. Use `serde_json::Value` to opt out.
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

    /// Declarative OAuth2 config consumed by the web app's generic OAuth
    /// service. Override on connectors that authenticate via OAuth; the
    /// default returns `None` for connectors that use service accounts,
    /// API keys, or other auth schemes.
    fn oauth_config(&self) -> Option<OAuthManifestConfig> {
        None
    }

    async fn sync(
        &self,
        source: Source,
        credentials: Option<ServiceCredential>,
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
        _credentials: Option<ServiceCredential>,
    ) -> Result<Response> {
        Ok(ActionResponse::not_supported(action).into_response_with_status(StatusCode::NOT_FOUND))
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
            oauth: self
                .oauth_config()
                .and_then(|c| serde_json::to_value(c).ok()),
        }
    }
}

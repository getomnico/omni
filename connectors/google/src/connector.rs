use std::sync::Arc;

use anyhow::Result;
use async_trait::async_trait;
use omni_connector_sdk::{
    ActionDefinition, ActionResponse, Connector, SearchOperator, SourceType, SyncContext, SyncType,
};
use serde_json::{json, Value as JsonValue};

use crate::admin::AdminClient;
use crate::models::GoogleConnectorState;
use crate::sync::SyncManager;

pub struct GoogleConnector {
    pub sync_manager: Arc<SyncManager>,
    pub admin_client: Arc<AdminClient>,
}

impl GoogleConnector {
    pub fn new(sync_manager: Arc<SyncManager>, admin_client: Arc<AdminClient>) -> Self {
        Self {
            sync_manager,
            admin_client,
        }
    }
}

#[async_trait]
impl Connector for GoogleConnector {
    type Config = JsonValue;
    type Credentials = JsonValue;
    type State = GoogleConnectorState;

    fn name(&self) -> &'static str {
        "google"
    }

    fn version(&self) -> &'static str {
        "1.0.0"
    }

    fn display_name(&self) -> String {
        "Google Workspace".to_string()
    }

    fn description(&self) -> Option<String> {
        Some("Connect to Google Drive, Docs, Gmail, and more".to_string())
    }

    fn source_types(&self) -> Vec<SourceType> {
        vec![SourceType::GoogleDrive, SourceType::Gmail]
    }

    fn sync_modes(&self) -> Vec<SyncType> {
        vec![SyncType::Full, SyncType::Incremental]
    }

    fn actions(&self) -> Vec<ActionDefinition> {
        vec![ActionDefinition {
            name: "fetch_file".to_string(),
            description:
                "Download a file from Google Drive. Exports Google Workspace files to Office format."
                    .to_string(),
            mode: "read".to_string(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "file_id": {
                        "type": "string",
                        "description": "The Google Drive file ID"
                    }
                },
                "required": ["file_id"]
            }),
        }]
    }

    fn search_operators(&self) -> Vec<SearchOperator> {
        vec![
            SearchOperator {
                operator: "from".to_string(),
                attribute_key: "sender".to_string(),
                value_type: "person".to_string(),
            },
            SearchOperator {
                operator: "label".to_string(),
                attribute_key: "labels".to_string(),
                value_type: "text".to_string(),
            },
        ]
    }

    /// `fetch_file` returns raw file bytes (not JSON), so the connector
    /// mounts its own `/action` handler via `routes.rs` and the SDK skips
    /// its default JSON-only handler.
    fn owns_action_route(&self) -> bool {
        true
    }

    async fn sync(
        &self,
        source_config: Self::Config,
        credentials: Self::Credentials,
        state: Option<Self::State>,
        ctx: SyncContext,
    ) -> Result<()> {
        self.sync_manager
            .run_sync(source_config, credentials, state, ctx)
            .await
    }

    async fn execute_action(
        &self,
        action: &str,
        _params: JsonValue,
        _credentials: JsonValue,
    ) -> Result<ActionResponse> {
        // Unreachable in practice: owns_action_route() is true, so the SDK
        // never mounts the default /action route that would dispatch here.
        Ok(ActionResponse::not_supported(action))
    }

    async fn cancel(&self, _sync_run_id: &str) -> bool {
        // The SDK's own cancellation flag (exposed via SyncContext) is the
        // source of truth; we just acknowledge the request.
        true
    }
}

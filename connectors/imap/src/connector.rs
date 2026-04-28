use anyhow::{anyhow, Context, Result};
use async_trait::async_trait;
use axum::http::StatusCode;
use axum::response::Response;
use omni_connector_sdk::{
    ActionDefinition, ActionResponse, Connector, SearchOperator, ServiceCredentials, Source,
    SourceType, SyncContext, SyncType,
};
use serde_json::{json, Value as JsonValue};
use std::sync::Arc;

use crate::client::ImapSession;
use crate::config::{ImapAccountConfig, ImapCredentials};
use crate::models::ImapConnectorState;
use crate::sync::SyncManager;

pub struct ImapConnector {
    sync_manager: Arc<SyncManager>,
}

impl ImapConnector {
    pub fn new(sync_manager: Arc<SyncManager>) -> Self {
        Self { sync_manager }
    }

    async fn run_action(
        action: &str,
        config: &ImapAccountConfig,
        credentials: &ImapCredentials,
    ) -> Result<JsonValue> {
        let mut session =
            ImapSession::connect(config, &credentials.username, &credentials.password).await?;
        let result = match action {
            "validate_credentials" => Ok(json!({ "authenticated": true })),
            "list_folders" => session
                .list_folders()
                .await
                .map(|f| json!({ "folders": f })),
            other => Err(anyhow!("Action not supported: {}", other)),
        };
        session.logout().await;
        result
    }
}

#[async_trait]
impl Connector for ImapConnector {
    type Config = ImapAccountConfig;
    type Credentials = ImapCredentials;
    type State = ImapConnectorState;

    fn name(&self) -> &'static str {
        "imap"
    }

    fn version(&self) -> &'static str {
        env!("CARGO_PKG_VERSION")
    }

    fn display_name(&self) -> String {
        "IMAP".to_string()
    }

    fn description(&self) -> Option<String> {
        Some("Index emails from any IMAP-compatible mailbox".to_string())
    }

    fn source_types(&self) -> Vec<SourceType> {
        vec![SourceType::Imap]
    }

    fn sync_modes(&self) -> Vec<SyncType> {
        vec![SyncType::Full, SyncType::Incremental]
    }

    fn read_only(&self) -> bool {
        true
    }

    fn actions(&self) -> Vec<ActionDefinition> {
        vec![
            ActionDefinition {
                name: "validate_credentials".to_string(),
                description: "Test IMAP connection with the provided credentials".to_string(),
                mode: omni_connector_sdk::ActionMode::Read,
                input_schema: json!({
                    "type": "object",
                    "properties": {
                        "host": { "type": "string" },
                        "port": { "type": "integer" },
                        "encryption": { "type": "string" }
                    },
                    "required": ["host"]
                }),
            },
            ActionDefinition {
                name: "list_folders".to_string(),
                description: "List accessible IMAP mailbox folders".to_string(),
                mode: omni_connector_sdk::ActionMode::Read,
                input_schema: json!({
                    "type": "object",
                    "properties": {
                        "host": { "type": "string" },
                        "port": { "type": "integer" },
                        "encryption": { "type": "string" }
                    },
                    "required": ["host"]
                }),
            },
        ]
    }

    fn search_operators(&self) -> Vec<SearchOperator> {
        vec![
            SearchOperator {
                operator: "from".to_string(),
                attribute_key: "from".to_string(),
                value_type: "person".to_string(),
            },
            SearchOperator {
                operator: "folder".to_string(),
                attribute_key: "folder".to_string(),
                value_type: "text".to_string(),
            },
            SearchOperator {
                operator: "subject".to_string(),
                attribute_key: "subject".to_string(),
                value_type: "text".to_string(),
            },
        ]
    }

    async fn sync(
        &self,
        source: Source,
        credentials: Option<ServiceCredentials>,
        state: Option<Self::State>,
        ctx: SyncContext,
    ) -> Result<()> {
        let config = ImapAccountConfig::from_source_config(&source.config)?;
        let creds = credentials.ok_or_else(|| anyhow!("IMAP sync requires credentials"))?;
        let typed_creds: ImapCredentials = serde_json::from_value(creds.credentials)
            .context("Failed to decode IMAP credentials")?;
        self.sync_manager
            .run_sync(config, typed_creds, state, ctx)
            .await
    }

    async fn execute_action(
        &self,
        action: &str,
        params: JsonValue,
        credentials: Option<ServiceCredentials>,
    ) -> Result<Response> {
        match action {
            "validate_credentials" | "list_folders" => {
                let config = match ImapAccountConfig::from_source_config(&params) {
                    Ok(c) => c,
                    Err(e) => return Ok(ActionResponse::failure(e.to_string()).into_response()),
                };
                let creds = match credentials {
                    Some(c) => c,
                    None => {
                        return Ok(ActionResponse::failure(
                            "IMAP action requires credentials".to_string(),
                        )
                        .into_response())
                    }
                };
                let typed_creds: ImapCredentials = match serde_json::from_value(creds.credentials)
                    .context("Failed to decode IMAP credentials")
                {
                    Ok(c) => c,
                    Err(e) => return Ok(ActionResponse::failure(e.to_string()).into_response()),
                };
                match Self::run_action(action, &config, &typed_creds).await {
                    Ok(result) => Ok(ActionResponse::success(result).into_response()),
                    Err(e) => Ok(ActionResponse::failure(e.to_string()).into_response()),
                }
            }
            _ => Ok(ActionResponse::not_supported(action)
                .into_response_with_status(StatusCode::NOT_FOUND)),
        }
    }

    async fn cancel(&self, _sync_run_id: &str) -> bool {
        // The SDK's own cancellation flag (exposed via SyncContext) is the
        // source of truth; we just acknowledge the request.
        true
    }
}

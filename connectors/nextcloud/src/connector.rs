use anyhow::{anyhow, Result};
use async_trait::async_trait;
use axum::response::Response;
use omni_connector_sdk::{
    ActionDefinition, ActionResponse, Connector, ServiceCredentials, Source, SourceType,
    SyncContext, SyncType,
};
use serde::Deserialize;
use serde_json::{json, Value as JsonValue};

use crate::client::NextcloudClient;
use crate::config::NextcloudConfig;
use crate::models::NextcloudConnectorState;
use crate::sync::run_sync;

#[derive(Debug, Deserialize)]
pub struct NextcloudCredentials {
    pub username: String,
    pub password: String,
}

pub struct NextcloudConnector;

impl NextcloudConnector {
    pub fn new() -> Self {
        Self
    }
}

impl Default for NextcloudConnector {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl Connector for NextcloudConnector {
    type Config = NextcloudConfig;
    type Credentials = NextcloudCredentials;
    type State = NextcloudConnectorState;

    fn name(&self) -> &'static str {
        "nextcloud"
    }

    fn version(&self) -> &'static str {
        env!("CARGO_PKG_VERSION")
    }

    fn display_name(&self) -> String {
        "Nextcloud".to_string()
    }

    fn description(&self) -> Option<String> {
        Some("Index files and documents from a Nextcloud instance via WebDAV".to_string())
    }

    fn source_types(&self) -> Vec<SourceType> {
        vec![SourceType::Nextcloud]
    }

    fn sync_modes(&self) -> Vec<SyncType> {
        vec![SyncType::Full, SyncType::Incremental]
    }

    fn read_only(&self) -> bool {
        true
    }

    fn actions(&self) -> Vec<ActionDefinition> {
        vec![ActionDefinition {
            name: "validate_credentials".into(),
            description: "Verify that the provided Nextcloud credentials are valid".into(),
            input_schema: json!({}),
            mode: omni_connector_sdk::ActionMode::Read,
        }]
    }

    async fn sync(
        &self,
        source: Source,
        credentials: Option<ServiceCredentials>,
        state: Option<Self::State>,
        ctx: SyncContext,
    ) -> Result<()> {
        let source_config = NextcloudConfig::from_source_config(&source.config)?;
        let creds = credentials.ok_or_else(|| anyhow!("Nextcloud credentials are required"))?;
        let nextcloud_creds: NextcloudCredentials = serde_json::from_value(creds.credentials)?;
        run_sync(source_config, nextcloud_creds, state, ctx).await
    }

    async fn cancel(&self, _sync_run_id: &str) -> bool {
        // SDK owns the cancellation flag (exposed via SyncContext); just ack.
        true
    }

    async fn execute_action(
        &self,
        action: &str,
        params: JsonValue,
        credentials: Option<ServiceCredentials>,
    ) -> Result<Response> {
        match action {
            "validate_credentials" => {
                let config = NextcloudConfig::from_source_config(&params)?;
                let creds =
                    credentials.ok_or_else(|| anyhow!("Nextcloud credentials are required"))?;
                let nextcloud_creds: NextcloudCredentials =
                    serde_json::from_value(creds.credentials)?;
                let client =
                    NextcloudClient::new(&nextcloud_creds.username, &nextcloud_creds.password);
                let base_url = config.webdav_base_url(&nextcloud_creds.username);
                let authenticated = client.validate_credentials(&base_url).await?;
                Ok(
                    ActionResponse::success(json!({ "authenticated": authenticated }))
                        .into_response(),
                )
            }
            other => Err(anyhow!("Action not supported: {}", other)),
        }
    }
}

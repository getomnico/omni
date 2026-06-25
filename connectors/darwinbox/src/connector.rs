use anyhow::{anyhow, Context, Result};
use async_trait::async_trait;
use axum::response::Response;
use omni_connector_sdk::{
    ActionDefinition, Connector, SearchOperator, ServiceCredential, Source, SourceType,
    SyncContext, SyncType,
};
use serde_json::Value as JsonValue;

use crate::actions;
use crate::client::DarwinboxClient;
use crate::config::DarwinboxSourceConfig;
use crate::credentials::DarwinboxCredentials;
use crate::models::DarwinboxCheckpoint;
use crate::sync::run_sync;

pub struct DarwinboxConnector;

impl DarwinboxConnector {
    pub fn new() -> Self {
        Self
    }
}

impl Default for DarwinboxConnector {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl Connector for DarwinboxConnector {
    type Config = DarwinboxSourceConfig;
    type Credentials = DarwinboxCredentials;
    type State = DarwinboxCheckpoint;

    fn name(&self) -> &'static str {
        "darwinbox"
    }

    fn version(&self) -> &'static str {
        env!("CARGO_PKG_VERSION")
    }

    fn display_name(&self) -> String {
        "Darwinbox".to_string()
    }

    fn description(&self) -> Option<String> {
        Some("Index Darwinbox employee directory data and expose HR workflow actions".to_string())
    }

    fn source_types(&self) -> Vec<SourceType> {
        vec![SourceType::Darwinbox]
    }

    fn sync_modes(&self) -> Vec<SyncType> {
        vec![SyncType::Full, SyncType::Incremental]
    }

    fn actions(&self) -> Vec<ActionDefinition> {
        actions::action_definitions()
    }

    fn search_operators(&self) -> Vec<SearchOperator> {
        vec![
            SearchOperator {
                operator: "employee".to_string(),
                attribute_key: "employee_id".to_string(),
                value_type: "text".to_string(),
            },
            SearchOperator {
                operator: "department".to_string(),
                attribute_key: "department".to_string(),
                value_type: "text".to_string(),
            },
            SearchOperator {
                operator: "designation".to_string(),
                attribute_key: "designation".to_string(),
                value_type: "text".to_string(),
            },
            SearchOperator {
                operator: "location".to_string(),
                attribute_key: "location".to_string(),
                value_type: "text".to_string(),
            },
            SearchOperator {
                operator: "manager".to_string(),
                attribute_key: "manager_employee_id".to_string(),
                value_type: "text".to_string(),
            },
            SearchOperator {
                operator: "employee_type".to_string(),
                attribute_key: "employee_type".to_string(),
                value_type: "text".to_string(),
            },
        ]
    }

    async fn sync(
        &self,
        source: Source,
        credentials: Option<ServiceCredential>,
        state: Option<Self::State>,
        ctx: SyncContext,
    ) -> Result<()> {
        let config: DarwinboxSourceConfig = serde_json::from_value(source.config.clone())
            .context("failed to decode Darwinbox source config")?;
        let creds = credentials.ok_or_else(|| anyhow!("Darwinbox credentials are required"))?;
        let darwinbox_creds: DarwinboxCredentials = serde_json::from_value(creds.credentials)
            .context("failed to decode Darwinbox credentials")?;
        let client = DarwinboxClient::new(&config, darwinbox_creds)?;
        run_sync(&client, &config, state, ctx).await
    }

    async fn cancel(&self, _sync_run_id: &str) -> bool {
        true
    }

    async fn execute_action(
        &self,
        action: &str,
        params: JsonValue,
        credentials: Option<ServiceCredential>,
    ) -> Result<Response> {
        actions::execute_action(action, params, credentials).await
    }
}

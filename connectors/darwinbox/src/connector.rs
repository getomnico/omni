use anyhow::{Context, Result, anyhow};
use async_trait::async_trait;
use axum::response::Response;
use omni_connector_sdk::{
    ActionDefinition, Connector, SearchOperator, ServiceCredential, Source, SourceType,
    SyncContext, SyncRequestValidationError, SyncType,
};
use serde_json::Value as JsonValue;
use std::result::Result as StdResult;

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

    fn extra_schema(&self) -> Option<JsonValue> {
        // Capability and action-group controls were removed: the Darwinbox
        // dataset key is the access control, and the connector attempts every
        // module with provider-side denial (4xx) tolerance.
        None
    }

    fn search_operators(&self) -> Vec<SearchOperator> {
        vec![
            SearchOperator {
                operator: "location".to_string(),
                attribute_key: "office_location".to_string(),
                value_type: "text".to_string(),
            },
            SearchOperator {
                operator: "position".to_string(),
                attribute_key: "position".to_string(),
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
                operator: "division".to_string(),
                attribute_key: "division".to_string(),
                value_type: "text".to_string(),
            },
            SearchOperator {
                operator: "business_unit".to_string(),
                attribute_key: "business_unit".to_string(),
                value_type: "text".to_string(),
            },
            SearchOperator {
                operator: "cost_center".to_string(),
                attribute_key: "cost_center".to_string(),
                value_type: "text".to_string(),
            },
            SearchOperator {
                operator: "company".to_string(),
                attribute_key: "group_company".to_string(),
                value_type: "text".to_string(),
            },
            SearchOperator {
                operator: "job".to_string(),
                attribute_key: "job_title".to_string(),
                value_type: "text".to_string(),
            },
        ]
    }

    fn read_only(&self) -> bool {
        // The connector defers read-only enforcement to the source config.
        // Connector-manager reads `source.config.read_only` at dispatch time.
        false
    }

    /// Validate source configuration before sync. Returns bad-request for
    /// unsafe or contradictory policies so the sync run is never started.
    async fn validate_sync_request(
        &self,
        source: &Source,
        _credentials: Option<&ServiceCredential>,
        _sync_type: SyncType,
    ) -> StdResult<(), SyncRequestValidationError> {
        // Decode the config to validate it
        let config: DarwinboxSourceConfig =
            serde_json::from_value(source.config.clone()).map_err(|e| {
                SyncRequestValidationError::BadRequest(format!(
                    "invalid Darwinbox source config: {e}"
                ))
            })?;

        // Validate the config semantically
        config.validate().map_err(|errors| {
            SyncRequestValidationError::BadRequest(format!(
                "Darwinbox config validation failed: {}",
                errors.join("; ")
            ))
        })?;

        Ok(())
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
        source: Option<Source>,
        actor_email: Option<String>,
    ) -> Result<Response> {
        actions::execute_action(action, params, credentials, source, actor_email).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn manifest_advertises_search_operators_for_indexed_entities() {
        let operators = DarwinboxConnector::new().search_operators();
        let by_operator: std::collections::HashMap<_, _> = operators
            .iter()
            .map(|op| (op.operator.as_str(), op.attribute_key.as_str()))
            .collect();
        assert_eq!(by_operator.get("location"), Some(&"office_location"));
        assert_eq!(by_operator.get("position"), Some(&"position"));
        assert_eq!(by_operator.get("department"), Some(&"department"));
        assert_eq!(by_operator.get("company"), Some(&"group_company"));
    }
}

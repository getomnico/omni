use anyhow::{anyhow, Context, Result};
use async_trait::async_trait;
use axum::response::Response;
use omni_connector_sdk::{
    ActionDefinition, ActionMode, Connector, SearchOperator, ServiceCredential, Source, SourceType,
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
        // Expose action groups derived from the internal action policy table,
        // so the setup UI can filter actions by module without hard-coding
        // action names or relying on shared ActionDefinition.category.
        use std::collections::BTreeMap;
        let mut groups: BTreeMap<&str, Vec<&str>> = BTreeMap::new();
        for policy in actions::action_policies() {
            groups.entry(policy.module).or_default().push(policy.name);
        }
        let mut map = serde_json::Map::new();
        for (module, names) in &groups {
            let mut reads = Vec::new();
            let mut writes = Vec::new();
            for name in names {
                let policy = actions::action_policies().iter().find(|p| p.name == *name);
                if let Some(p) = policy {
                    match p.mode {
                        ActionMode::Read => reads.push(name.to_string()),
                        ActionMode::Write => writes.push(name.to_string()),
                    }
                }
            }
            let group = serde_json::json!({
                "read": reads,
                "write": writes,
            });
            map.insert(module.to_string(), group);
        }
        Some(serde_json::json!({ "action_groups": serde_json::Value::Object(map) }))
    }

    fn read_only(&self) -> bool {
        // The connector defers read-only enforcement to the source config.
        // Connector-manager reads `source.config.read_only` at dispatch time.
        false
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

        // If read_only is false (writes allowed), ensure the config is not
        // contradictory: action modules must be explicitly enabled and at
        // least one write action must be authorized.
        if !config.read_only {
            if !config.action_modules.employee_self_service
                && !config.action_modules.manager_workflows
                && !config.action_modules.hr_operations
                && !config.action_modules.ats
            {
                return Err(SyncRequestValidationError::BadRequest(
                    "read-only mode disabled but no write action modules are enabled".to_string(),
                ));
            }

            // Validate that report/ATS HR admin lists are non-empty when those
            // modules are enabled
            if config.action_modules.reports && config.authorization.hr_admin_emails.is_empty() {
                return Err(SyncRequestValidationError::BadRequest(
                    "reports enabled but hr_admin_emails is empty".to_string(),
                ));
            }

            if config.action_modules.ats
                && config.authorization.recruiter_emails.is_empty()
                && config.authorization.hr_admin_emails.is_empty()
            {
                return Err(SyncRequestValidationError::BadRequest(
                    "ATS actions enabled but both hr_admin_emails and recruiter_emails are empty"
                        .to_string(),
                ));
            }
        }

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

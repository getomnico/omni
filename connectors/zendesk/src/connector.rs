use anyhow::{Result, anyhow};
use async_trait::async_trait;
use axum::response::Response;
use omni_connector_sdk::{
    ActionCredentialScope, ActionDefinition, ActionMode, ActionResponse, Connector, SearchOperator,
    ServiceCredential, Source, SourceType, SyncContext, SyncType,
};
use serde_json::{Value as JsonValue, json};

use crate::client::{ZendeskClient, is_auth_rejection};
use crate::config::ZendeskConfig;
use crate::credentials::ZendeskCredentials;
use crate::models::ZendeskCheckpoint;
use crate::sync::run_sync;

pub struct ZendeskConnector;

impl ZendeskConnector {
    pub fn new() -> Self {
        Self
    }
}

impl Default for ZendeskConnector {
    fn default() -> Self {
        Self::new()
    }
}

fn operator(operator: &str, attribute_key: &str, value_type: &str) -> SearchOperator {
    SearchOperator {
        operator: operator.to_string(),
        attribute_key: attribute_key.to_string(),
        value_type: value_type.to_string(),
    }
}

/// Checks the credential against `GET /users/me.json`; an anonymous or non-admin answer is not authenticated.
pub async fn validate_credentials(client: &ZendeskClient) -> Result<JsonValue> {
    let user = match client.me().await {
        Ok(response) => response.user,
        Err(error) if is_auth_rejection(&error) => {
            return Ok(json!({ "authenticated": false, "email": null, "role": null }));
        }
        Err(error) => return Err(error),
    };
    // The incremental ticket export is admin-only.
    let authenticated = user.id.is_some() && user.role.as_deref() == Some("admin");
    Ok(json!({ "authenticated": authenticated, "email": user.email, "role": user.role }))
}

#[async_trait]
impl Connector for ZendeskConnector {
    type Config = ZendeskConfig;
    type Credentials = ZendeskCredentials;
    type State = ZendeskCheckpoint;

    fn name(&self) -> &'static str {
        "zendesk"
    }

    fn version(&self) -> &'static str {
        env!("CARGO_PKG_VERSION")
    }

    fn display_name(&self) -> String {
        "Zendesk".to_string()
    }

    fn description(&self) -> Option<String> {
        Some("Index Zendesk Support tickets and Help Center articles".to_string())
    }

    fn source_types(&self) -> Vec<SourceType> {
        vec![SourceType::Zendesk]
    }

    fn sync_modes(&self) -> Vec<SyncType> {
        vec![SyncType::Full, SyncType::Incremental]
    }

    fn read_only(&self) -> bool {
        true
    }

    fn search_operators(&self) -> Vec<SearchOperator> {
        vec![
            operator("status", "status", "text"),
            operator("priority", "priority", "text"),
            operator("type", "ticket_type", "text"),
            operator("tag", "tags", "text"),
            operator("assignee", "assignee", "person"),
            operator("requester", "requester", "person"),
        ]
    }

    fn actions(&self) -> Vec<ActionDefinition> {
        vec![ActionDefinition {
            origin: Default::default(),
            name: "validate_credentials".into(),
            description: "Verify that the Zendesk API token belongs to an admin".into(),
            input_schema: json!({}),
            mode: ActionMode::Read,
            credential_scope: ActionCredentialScope::Org,
            required_scopes: None,
            source_types: Vec::new(),
            admin_only: true,
            actor_scoped: false,
            hidden: true,
        }]
    }

    async fn sync(
        &self,
        source: Source,
        credentials: Option<ServiceCredential>,
        state: Option<Self::State>,
        ctx: SyncContext,
    ) -> Result<()> {
        let config = ZendeskConfig::from_source_config(&source.config)?;
        let credentials = credentials.ok_or_else(|| anyhow!("Zendesk credentials are required"))?;
        let credentials: ZendeskCredentials = serde_json::from_value(credentials.credentials)?;
        let client = ZendeskClient::new(&config, credentials)?;
        run_sync(&client, &config, state, ctx).await
    }

    async fn cancel(&self, _sync_run_id: &str) -> bool {
        // The SDK owns the cancellation flag exposed via SyncContext.
        true
    }

    async fn execute_action(
        &self,
        action: &str,
        _params: JsonValue,
        credentials: Option<ServiceCredential>,
        source: Option<Source>,
        _actor_email: Option<String>,
    ) -> Result<Response> {
        match action {
            "validate_credentials" => {
                let source =
                    source.ok_or_else(|| anyhow!("validate_credentials requires a source"))?;
                let config = ZendeskConfig::from_source_config(&source.config)?;
                let credentials =
                    credentials.ok_or_else(|| anyhow!("Zendesk credentials are required"))?;
                let credentials: ZendeskCredentials =
                    serde_json::from_value(credentials.credentials)?;
                let client = ZendeskClient::new(&config, credentials)?;
                Ok(ActionResponse::success(validate_credentials(&client).await?).into_response())
            }
            other => Err(anyhow!("Action not supported: {other}")),
        }
    }
}

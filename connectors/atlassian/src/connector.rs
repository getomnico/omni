use std::sync::Arc;

use anyhow::Result;
use async_trait::async_trait;
use axum::http::StatusCode;
use axum::response::Response;
use omni_connector_sdk::{
    ActionDefinition, ActionMode, ActionResponse, Connector, SearchOperator, ServiceCredential,
    Source, SourceType, SyncContext, SyncType,
};
use serde_json::{json, Value as JsonValue};
use tracing::info;

use crate::auth::AtlassianCredentials;
use crate::client::{AtlassianApi, AtlassianClient};
use crate::models::AtlassianConnectorState;
use crate::sync::SyncManager;

pub struct AtlassianConnector {
    pub sync_manager: Arc<SyncManager>,
}

impl AtlassianConnector {
    pub fn new(sync_manager: Arc<SyncManager>) -> Self {
        Self { sync_manager }
    }
}

#[async_trait]
impl Connector for AtlassianConnector {
    type Config = JsonValue;
    type Credentials = JsonValue;
    type State = AtlassianConnectorState;

    fn name(&self) -> &'static str {
        "atlassian"
    }

    fn version(&self) -> &'static str {
        "1.0.0"
    }

    fn display_name(&self) -> String {
        "Atlassian".to_string()
    }

    fn description(&self) -> Option<String> {
        Some("Connect to Confluence and Jira using an API token".to_string())
    }

    fn source_types(&self) -> Vec<SourceType> {
        vec![SourceType::Confluence, SourceType::Jira]
    }

    fn sync_modes(&self) -> Vec<SyncType> {
        vec![SyncType::Full, SyncType::Incremental]
    }

    fn actions(&self) -> Vec<ActionDefinition> {
        vec![ActionDefinition {
            name: "search_spaces".to_string(),
            description: "Search Confluence spaces or Jira projects".to_string(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "query": {
                        "type": "string",
                        "description": "Search query to filter by name or key"
                    },
                    "type": {
                        "type": "string",
                        "description": "Whether to search Confluence spaces or Jira projects"
                    }
                },
                "required": ["type"]
            }),
            mode: ActionMode::Read,
        }]
    }

    fn search_operators(&self) -> Vec<SearchOperator> {
        vec![
            SearchOperator {
                operator: "status".to_string(),
                attribute_key: "status".to_string(),
                value_type: "text".to_string(),
            },
            SearchOperator {
                operator: "label".to_string(),
                attribute_key: "labels".to_string(),
                value_type: "text".to_string(),
            },
            SearchOperator {
                operator: "project".to_string(),
                attribute_key: "project_key".to_string(),
                value_type: "text".to_string(),
            },
            SearchOperator {
                operator: "assignee".to_string(),
                attribute_key: "assignee".to_string(),
                value_type: "person".to_string(),
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
        self.sync_manager
            .run_sync(source, credentials, state, ctx)
            .await
    }

    async fn execute_action(
        &self,
        action: &str,
        params: JsonValue,
        credentials: Option<ServiceCredential>,
    ) -> Result<Response> {
        info!("Action requested: {}", action);
        match action {
            "search_spaces" => Ok(handle_search_spaces(params, credentials)
                .await
                .into_response()),
            _ => Ok(ActionResponse::not_supported(action)
                .into_response_with_status(StatusCode::NOT_FOUND)),
        }
    }

    async fn cancel(&self, _sync_run_id: &str) -> bool {
        // SDK's own cancellation flag (via SyncContext) is the source of truth.
        true
    }
}

pub async fn handle_search_spaces(
    params: JsonValue,
    credentials: Option<ServiceCredential>,
) -> ActionResponse {
    let query = params
        .get("query")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_lowercase();
    let search_type = match params.get("type").and_then(|v| v.as_str()) {
        Some(t) => t.to_string(),
        None => return ActionResponse::failure("Missing required parameter: type"),
    };

    let creds = match credentials {
        Some(c) => c,
        None => return ActionResponse::failure("Atlassian action requires credentials"),
    };

    let base_url = match creds.config.get("base_url").and_then(|v| v.as_str()) {
        Some(u) => u.to_string(),
        None => return ActionResponse::failure("Missing base_url in credentials config"),
    };
    let user_email = match creds.principal_email.as_deref() {
        Some(e) => e.to_string(),
        None => return ActionResponse::failure("Missing principal_email in credentials"),
    };
    let api_token = match creds.credentials.get("api_token").and_then(|v| v.as_str()) {
        Some(t) => t.to_string(),
        None => return ActionResponse::failure("Missing api_token in credentials"),
    };

    let creds = AtlassianCredentials::new(base_url, user_email, api_token);
    let client = AtlassianClient::new();

    match search_type.as_str() {
        "confluence" => match client.get_confluence_spaces(&creds).await {
            Ok(spaces) => {
                let results: Vec<JsonValue> = spaces
                    .into_iter()
                    .filter(|s| {
                        s.r#type != "personal"
                            && (query.is_empty()
                                || s.key.to_lowercase().contains(&query)
                                || s.name.to_lowercase().contains(&query))
                    })
                    .map(|s| {
                        json!({
                            "key": s.key,
                            "name": s.name,
                            "type": "confluence"
                        })
                    })
                    .collect();
                ActionResponse::success(json!(results))
            }
            Err(e) => ActionResponse::failure(format!("Failed to fetch Confluence spaces: {}", e)),
        },
        "jira" => match client.get_jira_projects(&creds, &[]).await {
            Ok(projects) => {
                let results: Vec<JsonValue> = projects
                    .into_iter()
                    .filter(|p| {
                        if query.is_empty() {
                            return true;
                        }
                        let key = p
                            .get("key")
                            .and_then(|v| v.as_str())
                            .unwrap_or("")
                            .to_lowercase();
                        let name = p
                            .get("name")
                            .and_then(|v| v.as_str())
                            .unwrap_or("")
                            .to_lowercase();
                        key.contains(&query) || name.contains(&query)
                    })
                    .map(|p| {
                        json!({
                            "key": p.get("key").and_then(|v| v.as_str()).unwrap_or(""),
                            "name": p.get("name").and_then(|v| v.as_str()).unwrap_or(""),
                            "type": "jira"
                        })
                    })
                    .collect();
                ActionResponse::success(json!(results))
            }
            Err(e) => ActionResponse::failure(format!("Failed to fetch Jira projects: {}", e)),
        },
        _ => ActionResponse::failure(format!(
            "Invalid type: {}. Must be 'confluence' or 'jira'",
            search_type
        )),
    }
}

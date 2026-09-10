use std::collections::HashMap;
use std::sync::Arc;

use anyhow::{Result, anyhow};
use async_trait::async_trait;
use axum::http::StatusCode;
use axum::response::Response;
use omni_connector_sdk::{
    ActionCredentialScope, ActionDefinition, ActionMode, ActionResponse, Connector, HttpMcpServer,
    McpCredentials,
    McpServer, OAuthManifestConfig, OAuthScopeSet, OAuthTokenEndpointAuthMethod, SearchOperator,
    ServiceCredential, Source, SourceType, SyncContext, SyncType,
};
use serde_json::{Value as JsonValue, json};
use tracing::info;

use crate::auth::{AtlassianCredentials, AuthManager};
use crate::client::{AtlassianApi, AtlassianClient};
use crate::models::AtlassianSyncCheckpoint;
use crate::sync::SyncManager;

pub struct AtlassianConnector {
    pub sync_manager: Arc<SyncManager>,
}

impl AtlassianConnector {
    pub const ROVO_MCP_URL: &'static str = "https://mcp.atlassian.com/v1/mcp";
    pub const ROVO_AUTH_URL: &'static str = "https://mcp.atlassian.com/v1/authorize";
    pub const ROVO_TOKEN_URL: &'static str = "https://mcp.atlassian.com/v1/token";
    pub const ROVO_REGISTER_URL: &'static str = "https://mcp.atlassian.com/v1/register";
    pub const ROVO_PROVIDER: &'static str = "atlassian";

    pub fn new(sync_manager: Arc<SyncManager>) -> Self {
        Self { sync_manager }
    }

    fn rovo_scopes() -> HashMap<String, OAuthScopeSet> {
        HashMap::from([
            (
                "confluence".to_string(),
                OAuthScopeSet {
                    read: vec!["read:confluence-content.all".to_string()],
                    write: vec![
                        "read:confluence-content.all".to_string(),
                        "write:confluence-content".to_string(),
                    ],
                },
            ),
            (
                "jira".to_string(),
                OAuthScopeSet {
                    read: vec!["read:jira-work".to_string()],
                    write: vec!["read:jira-work".to_string(), "write:jira-work".to_string()],
                },
            ),
        ])
    }

    fn rovo_oauth_config() -> OAuthManifestConfig {
        OAuthManifestConfig {
            provider: Self::ROVO_PROVIDER.to_string(),
            auth_endpoint: Self::ROVO_AUTH_URL.to_string(),
            token_endpoint: Self::ROVO_TOKEN_URL.to_string(),
            userinfo_endpoint: None,
            userinfo_email_field: "email".to_string(),
            identity_scopes: vec![],
            scopes: Self::rovo_scopes(),
            extra_auth_params: HashMap::from([(
                "resource".to_string(),
                Self::ROVO_MCP_URL.to_string(),
            )]),
            scope_separator: " ".to_string(),
            enrich_endpoint: None,
            registration_endpoint: Some(Self::ROVO_REGISTER_URL.to_string()),
            registration_requires_initial_access_token: false,
            token_response_fields: Vec::new(),
            token_endpoint_auth_method: OAuthTokenEndpointAuthMethod::None,
            resource: Some(Self::ROVO_MCP_URL.to_string()),
            issuer_source_config_key: None,
            client_config_provider_template: None,
            pkce_required: false,
            grant_types: Some(vec![
                "authorization_code".to_string(),
                "refresh_token".to_string(),
            ]),
            validate_endpoint_urls: false,
            supports_org_oauth: true,
        }
    }

    fn rovo_headers(credentials: &McpCredentials) -> Result<HashMap<String, String>> {
        let access_token = credentials
            .credentials
            .get("access_token")
            .and_then(|value| value.as_str())
            .filter(|token| !token.is_empty())
            .ok_or_else(|| anyhow!("Rovo MCP requires a per-user OAuth access_token"))?;
        Ok(HashMap::from([(
            "Authorization".to_string(),
            format!("Bearer {access_token}"),
        )]))
    }
}

#[async_trait]
impl Connector for AtlassianConnector {
    type Config = JsonValue;
    type Credentials = JsonValue;
    type State = AtlassianSyncCheckpoint;

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

    fn mcp_server(&self) -> Option<McpServer> {
        Some(McpServer::Http(HttpMcpServer::new(Self::ROVO_MCP_URL)))
    }

    async fn prepare_mcp_headers(
        &self,
        credentials: &McpCredentials,
    ) -> Result<HashMap<String, String>> {
        AtlassianConnector::rovo_headers(credentials)
    }

    fn oauth_config(&self) -> Option<OAuthManifestConfig> {
        Some(Self::rovo_oauth_config())
    }

    fn actions(&self) -> Vec<ActionDefinition> {
        vec![ActionDefinition {
            origin: Default::default(),
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
            credential_scope: ActionCredentialScope::Org,
            required_scopes: None,
            source_types: Vec::new(),
            admin_only: true,
            actor_scoped: false,
            hidden: false,
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
        _source: Option<Source>,
        _actor_email: Option<String>,
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

    let domain = match creds.config.get("domain").and_then(|v| v.as_str()) {
        Some(u) => u.to_string(),
        None => return ActionResponse::failure("Missing domain in credentials config"),
    };
    let sa_token = match creds.credentials.get("sa_token").and_then(|v| v.as_str()) {
        Some(t) => t.to_string(),
        None => return ActionResponse::failure("Missing sa_token in credentials"),
    };

    let cloud_id = match AuthManager::new().fetch_cloud_id(&domain).await {
        Ok(id) => id,
        Err(e) => return ActionResponse::failure(format!("Failed to resolve cloud_id: {}", e)),
    };

    let creds = AtlassianCredentials::new(domain, cloud_id, sa_token);
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rovo_oauth_uses_direct_public_client_endpoints() {
        let config = AtlassianConnector::rovo_oauth_config();
        assert_eq!(config.provider, "atlassian");
        assert_eq!(config.auth_endpoint, AtlassianConnector::ROVO_AUTH_URL);
        assert_eq!(config.token_endpoint, AtlassianConnector::ROVO_TOKEN_URL);
        assert_eq!(
            config.registration_endpoint.as_deref(),
            Some(AtlassianConnector::ROVO_REGISTER_URL)
        );
        assert_eq!(
            config.resource.as_deref(),
            Some(AtlassianConnector::ROVO_MCP_URL)
        );
        assert_eq!(config.userinfo_endpoint, None);
        assert_eq!(
            config.token_endpoint_auth_method,
            OAuthTokenEndpointAuthMethod::None
        );
        assert_eq!(
            config.extra_auth_params.get("resource").map(String::as_str),
            Some(AtlassianConnector::ROVO_MCP_URL)
        );
    }

    #[test]
    fn rovo_headers_require_only_the_per_user_access_token() {
        let headers = AtlassianConnector::rovo_headers(&McpCredentials {
            credentials: json!({"access_token": "user-token", "sa_token": "must-not-be-used"}),
            ..Default::default()
        })
        .unwrap();
        assert_eq!(headers.get("Authorization").unwrap(), "Bearer user-token");
        assert!(
            AtlassianConnector::rovo_headers(&McpCredentials {
                credentials: json!({"sa_token": "service-token"}),
                ..Default::default()
            })
            .unwrap_err()
            .to_string()
            .contains("per-user OAuth access_token")
        );
    }
}

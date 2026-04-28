use std::sync::Arc;

use crate::admin::AdminClient;
use crate::auth::{create_service_auth, get_domain_from_credentials, GoogleAuth};
use crate::drive::DriveClient;
use crate::models::{GoogleConnectorState, GoogleDirectoryUser, SearchUsersResponse};
use crate::sync::SyncManager;
use anyhow::{anyhow, Result};
use async_trait::async_trait;
use axum::response::Response;
use omni_connector_sdk::{
    ActionDefinition, ActionResponse, Connector, OAuthManifestConfig, OAuthScopeSet,
    SearchOperator, ServiceCredential, Source, SourceType, SyncContext, SyncType,
};
use serde_json::{json, Value as JsonValue};
use std::collections::HashMap;

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

    async fn execute_fetch_file(
        &self,
        params: JsonValue,
        creds: &ServiceCredential,
    ) -> Result<Response> {
        let file_id = params
            .get("file_id")
            .and_then(|v| v.as_str())
            .ok_or_else(|| anyhow!("Missing required parameter: file_id"))?;

        let principal_email = creds
            .principal_email
            .as_deref()
            .ok_or_else(|| anyhow!("Missing principal_email in credentials"))?;

        let auth = create_service_auth(creds, SourceType::GoogleDrive)?;
        let google_auth = GoogleAuth::ServiceAccount(auth);
        let drive_client = DriveClient::new();

        let file_meta = drive_client
            .get_file_metadata(&google_auth, principal_email, file_id)
            .await?;

        let mime_type = &file_meta.mime_type;
        let file_name = &file_meta.name;

        let export_mapping: Option<(&str, &str)> = match mime_type.as_str() {
            "application/vnd.google-apps.spreadsheet" => Some((
                "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet",
                ".xlsx",
            )),
            "application/vnd.google-apps.document" => Some((
                "application/vnd.openxmlformats-officedocument.wordprocessingml.document",
                ".docx",
            )),
            "application/vnd.google-apps.presentation" => Some((
                "application/vnd.openxmlformats-officedocument.presentationml.presentation",
                ".pptx",
            )),
            _ => None,
        };

        let (bytes, content_type) = if let Some((export_mime, _ext)) = export_mapping {
            let bytes = drive_client
                .export_file(&google_auth, principal_email, file_id, export_mime)
                .await?;
            (bytes, export_mime.to_string())
        } else {
            let bytes = drive_client
                .download_file_binary(&google_auth, principal_email, file_id)
                .await?;
            (bytes, mime_type.clone())
        };

        let mut resp = Response::builder()
            .status(200)
            .header("Content-Type", content_type)
            .header("Content-Length", bytes.len())
            .header("X-File-Name", file_name);
        let body = axum::body::Body::from(bytes);
        resp.body(body)
            .map_err(|e| anyhow::anyhow!("Failed to build response: {}", e))
    }

    async fn execute_search_users(
        &self,
        params: JsonValue,
        creds: &ServiceCredential,
    ) -> Result<axum::response::Response> {
        let limit = params
            .get("limit")
            .and_then(|v| v.as_u64())
            .unwrap_or(50)
            .min(100) as u32;
        let query = params.get("q").and_then(|v| v.as_str());
        let page_token = params.get("page_token").and_then(|v| v.as_str());

        let principal_email = creds
            .principal_email
            .as_deref()
            .ok_or_else(|| anyhow!("Missing principal_email in credentials"))?;
        let domain = get_domain_from_credentials(creds)?;

        let auth = create_service_auth(creds, SourceType::GoogleDrive)?;
        let token = auth.get_access_token(principal_email).await?;

        let response = self
            .admin_client
            .search_users(&token, &domain, query, Some(limit), page_token)
            .await?;

        let has_more = response.next_page_token.is_some();

        let users: Vec<GoogleDirectoryUser> = response
            .users
            .unwrap_or_default()
            .into_iter()
            .map(|user| GoogleDirectoryUser {
                id: user.id,
                email: user.primary_email,
                name: user
                    .name
                    .and_then(|n| n.full_name)
                    .unwrap_or_else(|| "Unknown".to_string()),
                org_unit: user.org_unit_path.unwrap_or_else(|| "/".to_string()),
                suspended: user.suspended.unwrap_or(false),
                is_admin: user.is_admin.unwrap_or(false),
            })
            .collect();

        let result = SearchUsersResponse {
            users,
            next_page_token: response.next_page_token,
            has_more,
        };

        Ok(ActionResponse::success(serde_json::to_value(result)?).into_response())
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
        vec![
            ActionDefinition {
                name: "fetch_file".to_string(),
                description:
                    "Download a file from Google Drive. Exports Google Workspace files to Office format."
                        .to_string(),
                mode: omni_connector_sdk::ActionMode::Read,
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
            },
            ActionDefinition {
                name: "search_users".to_string(),
                description: "Search Google Admin directory users".to_string(),
                mode: omni_connector_sdk::ActionMode::Read,
                input_schema: json!({
                    "type": "object",
                    "properties": {
                        "q": { "type": "string", "description": "Search query" },
                        "limit": { "type": "integer", "default": 50 },
                        "page_token": { "type": "string" }
                    },
                    "required": []
                }),
            },
        ]
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

    fn oauth_config(&self) -> Option<OAuthManifestConfig> {
        let mut scopes = HashMap::new();
        scopes.insert(
            "google_drive".to_string(),
            OAuthScopeSet {
                read: vec!["https://www.googleapis.com/auth/drive.readonly".to_string()],
                // drive.file scopes the grant to files the app creates/opens — the
                // safe default for MCP write tools.
                write: vec!["https://www.googleapis.com/auth/drive.file".to_string()],
            },
        );
        scopes.insert(
            "gmail".to_string(),
            OAuthScopeSet {
                read: vec!["https://www.googleapis.com/auth/gmail.readonly".to_string()],
                write: vec![
                    "https://www.googleapis.com/auth/gmail.send".to_string(),
                    "https://www.googleapis.com/auth/gmail.modify".to_string(),
                ],
            },
        );

        let mut extra_auth_params = HashMap::new();
        extra_auth_params.insert("access_type".to_string(), "offline".to_string());
        extra_auth_params.insert("prompt".to_string(), "consent".to_string());

        Some(OAuthManifestConfig {
            provider: "google".to_string(),
            auth_endpoint: "https://accounts.google.com/o/oauth2/v2/auth".to_string(),
            token_endpoint: "https://oauth2.googleapis.com/token".to_string(),
            userinfo_endpoint: "https://www.googleapis.com/oauth2/v3/userinfo".to_string(),
            userinfo_email_field: "email".to_string(),
            identity_scopes: vec!["email".to_string(), "profile".to_string()],
            scopes,
            extra_auth_params,
            scope_separator: " ".to_string(),
            enrich_endpoint: None,
        })
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
    ) -> Result<axum::response::Response> {
        let creds = match credentials {
            Some(c) => c,
            None => {
                return Ok(ActionResponse::failure(
                    "Google action requires credentials".to_string(),
                )
                .into_response())
            }
        };
        match action {
            "fetch_file" => self.execute_fetch_file(params, &creds).await,
            "search_users" => self.execute_search_users(params, &creds).await,
            _ => {
                use axum::http::StatusCode;
                Ok(ActionResponse::not_supported(action)
                    .into_response_with_status(StatusCode::NOT_FOUND))
            }
        }
    }

    async fn cancel(&self, _sync_run_id: &str) -> bool {
        // The SDK's own cancellation flag (exposed via SyncContext) is the
        // source of truth; we just acknowledge the request.
        true
    }
}

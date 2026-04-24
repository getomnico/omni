use std::sync::Arc;

use anyhow::Result;
use async_trait::async_trait;
use omni_connector_sdk::{
    ActionDefinition, ActionResult, Connector, SearchOperator, SourceType, SyncContext, SyncType,
};
use serde_json::{json, Value as JsonValue};
use shared::models::ServiceProvider;

use crate::admin::AdminClient;
use crate::auth::{GoogleAuth, ServiceAccountAuth};
use crate::drive::DriveClient;
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

    async fn execute_fetch_file(
        &self,
        params: JsonValue,
        credentials: JsonValue,
    ) -> Result<ActionResult> {
        let file_id = params
            .get("file_id")
            .and_then(|v| v.as_str())
            .ok_or_else(|| anyhow::anyhow!("Missing required parameter: file_id"))?;

        let service_account_key = credentials
            .get("credentials")
            .and_then(|c| c.get("service_account_key"))
            .and_then(|v| v.as_str())
            .ok_or_else(|| anyhow::anyhow!("Missing service_account_key in credentials"))?;

        let principal_email = credentials
            .get("principal_email")
            .and_then(|v| v.as_str())
            .ok_or_else(|| anyhow::anyhow!("Missing principal_email in credentials"))?;

        let scopes = crate::auth::get_scopes_for_source_type(SourceType::GoogleDrive);
        let auth = ServiceAccountAuth::new(service_account_key, scopes)?;
        let google_auth = GoogleAuth::ServiceAccount(auth);
        let drive_client = DriveClient::new();

        let file_meta = drive_client
            .get_file_metadata(&google_auth, principal_email, file_id)
            .await?;

        let mime_type = &file_meta.mime_type;
        let _file_name = &file_meta.name;

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

        Ok(ActionResult::binary(bytes, content_type))
    }

    async fn execute_search_users(
        &self,
        params: JsonValue,
        _credentials: JsonValue,
    ) -> Result<ActionResult> {
        let source_id = params
            .get("source_id")
            .and_then(|v| v.as_str())
            .ok_or_else(|| anyhow::anyhow!("Missing required parameter: source_id"))?;

        let limit = params
            .get("limit")
            .and_then(|v| v.as_u64())
            .unwrap_or(50)
            .min(100) as u32;
        let query = params.get("q").and_then(|v| v.as_str());
        let page_token = params.get("page_token").and_then(|v| v.as_str());

        let creds = self
            .sync_manager
            .sdk_client
            .get_credentials(source_id)
            .await?;
        if creds.provider != ServiceProvider::Google {
            anyhow::bail!(
                "Expected Google credentials for source {}, found {:?}",
                source_id,
                creds.provider
            );
        }

        let service_account_key = creds
            .credentials
            .get("service_account_key")
            .and_then(|v| v.as_str())
            .ok_or_else(|| anyhow::anyhow!("Missing service_account_key in credentials"))?;

        let domain = creds
            .config
            .get("domain")
            .and_then(|v| v.as_str())
            .ok_or_else(|| anyhow::anyhow!("Missing domain in credentials config"))?;

        let principal_email = self
            .sync_manager
            .sdk_client
            .get_user_email_for_source(source_id)
            .await?;

        let admin_scopes = crate::auth::get_scopes_for_source_type(SourceType::GoogleDrive);
        let auth = ServiceAccountAuth::new(service_account_key, admin_scopes)?;
        let token = auth.get_access_token(&principal_email).await?;

        let response = self
            .admin_client
            .search_users(&token, domain, query, Some(limit), page_token)
            .await?;

        let users: Vec<JsonValue> = response
            .users
            .unwrap_or_default()
            .into_iter()
            .map(|user| {
                json!({
                    "id": user.id,
                    "email": user.primary_email,
                    "name": user.name.and_then(|n| n.full_name).unwrap_or_else(|| "Unknown".to_string()),
                    "org_unit": user.org_unit_path.unwrap_or_else(|| "/".to_string()),
                    "suspended": user.suspended.unwrap_or(false),
                    "is_admin": user.is_admin.unwrap_or(false),
                })
            })
            .collect();

        Ok(ActionResult::json(json!({
            "users": users,
            "next_page_token": response.next_page_token,
            "has_more": response.next_page_token.is_some(),
        })))
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
            },
            ActionDefinition {
                name: "search_users".to_string(),
                description: "Search Google Admin directory users".to_string(),
                mode: "read".to_string(),
                input_schema: json!({
                    "type": "object",
                    "properties": {
                        "source_id": { "type": "string" },
                        "q": { "type": "string", "description": "Search query" },
                        "limit": { "type": "integer", "default": 50 },
                        "page_token": { "type": "string" }
                    },
                    "required": ["source_id"]
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
        params: JsonValue,
        credentials: JsonValue,
    ) -> Result<ActionResult> {
        match action {
            "fetch_file" => self.execute_fetch_file(params, credentials).await,
            "search_users" => self.execute_search_users(params, credentials).await,
            _ => Err(anyhow::anyhow!("Action not supported: {}", action)),
        }
    }

    async fn cancel(&self, _sync_run_id: &str) -> bool {
        // The SDK's own cancellation flag (exposed via SyncContext) is the
        // source of truth; we just acknowledge the request.
        true
    }
}

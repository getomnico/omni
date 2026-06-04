use anyhow::{anyhow, Result};
use async_trait::async_trait;
use axum::response::Response;
use omni_connector_sdk::{
    ActionDefinition, ActionResponse, Connector, ServiceCredential, Source, SourceType,
    SyncContext, SyncType,
};
use serde::Deserialize;
use serde_json::{json, Value as JsonValue};

fn guess_mime_type(filename: &str) -> &'static str {
    match filename.rsplit('.').next().unwrap_or("").to_lowercase().as_str() {
        // Office Open XML
        "docx" => "application/vnd.openxmlformats-officedocument.wordprocessingml.document",
        "xlsx" => "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet",
        "pptx" => "application/vnd.openxmlformats-officedocument.presentationml.presentation",
        // Legacy MS Office
        "doc" => "application/msword",
        "xls" => "application/vnd.ms-excel",
        "ppt" => "application/vnd.ms-powerpoint",
        // OpenDocument (common on Nextcloud)
        "odt" => "application/vnd.oasis.opendocument.text",
        "ods" => "application/vnd.oasis.opendocument.spreadsheet",
        "odp" => "application/vnd.oasis.opendocument.presentation",
        "odg" => "application/vnd.oasis.opendocument.graphics",
        // PDF
        "pdf" => "application/pdf",
        // Markup / text
        "md" | "markdown" => "text/markdown",
        "html" | "htm" => "text/html",
        "xml" => "application/xml",
        "csv" => "text/csv",
        "txt" => "text/plain",
        "json" => "application/json",
        // Calendar / email
        "ics" => "text/calendar",
        "eml" => "message/rfc822",
        "msg" => "application/vnd.ms-outlook",
        // Images
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "gif" => "image/gif",
        "webp" => "image/webp",
        "svg" => "image/svg+xml",
        // Archives
        "zip" => "application/zip",
        _ => "application/octet-stream",
    }
}

use crate::client::NextcloudClient;
use crate::config::NextcloudConfig;
use crate::models::NextcloudConnectorState;
use crate::sync::{build_download_url, run_sync};

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
        vec![
            ActionDefinition {
                name: "validate_credentials".into(),
                description: "Verify that the provided Nextcloud credentials are valid".into(),
                input_schema: json!({}),
                mode: omni_connector_sdk::ActionMode::Read,
                source_types: Vec::new(),
                admin_only: false,
            },
            ActionDefinition {
                name: "fetch_file".into(),
                description: "Download a file from Nextcloud by its document ID".into(),
                input_schema: json!({
                    "type": "object",
                    "properties": {
                        "file_id": {
                            "type": "string",
                            "description": "External file ID (nextcloud:{source_id}:{key})"
                        }
                    },
                    "required": ["file_id"]
                }),
                mode: omni_connector_sdk::ActionMode::Read,
                source_types: Vec::new(),
                admin_only: false,
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
        credentials: Option<ServiceCredential>,
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
            "fetch_file" => {
                // Source config (server_url etc.) is merged into params by the
                // connector-manager before dispatch, so from_source_config works here.
                let config = NextcloudConfig::from_source_config(&params)?;
                let creds =
                    credentials.ok_or_else(|| anyhow!("Nextcloud credentials are required"))?;
                let nextcloud_creds: NextcloudCredentials =
                    serde_json::from_value(creds.credentials)?;
                let client =
                    NextcloudClient::new(&nextcloud_creds.username, &nextcloud_creds.password);

                let file_id = params
                    .get("file_id")
                    .and_then(|v| v.as_str())
                    .ok_or_else(|| anyhow!("Missing required parameter: file_id"))?;

                // external_id format: "nextcloud:{source_id}:{url_encoded_key}"
                // The key is either a WebDAV href (starts with '/') or a
                // Nextcloud oc:fileid (numeric/alphanumeric string).
                let parts: Vec<&str> = file_id.splitn(3, ':').collect();
                if parts.len() < 3 || parts[0] != "nextcloud" {
                    anyhow::bail!("Invalid file_id format: {}", file_id);
                }
                let key = urlencoding::decode(parts[2])
                    .map_err(|e| anyhow!("Failed to URL-decode file key: {}", e))?
                    .into_owned();

                // Resolve key to a canonical WebDAV href.
                let href = if key.starts_with('/') {
                    key
                } else {
                    // Numeric oc:fileid — resolve to path via PROPFIND meta endpoint.
                    client
                        .get_href_by_file_id(&config.server_url, &key)
                        .await?
                };

                // build_download_url handles both relative paths and absolute URLs
                // (some Nextcloud instances return absolute d:href values).
                let download_url = build_download_url(&config.server_url, &href);

                // Extract and decode the filename from the last path segment.
                let raw_name = href.rsplit('/').next().unwrap_or("document");
                let filename = urlencoding::decode(raw_name)
                    .unwrap_or_else(|_| std::borrow::Cow::Borrowed(raw_name))
                    .into_owned();

                let bytes = client.download_file(&download_url).await?;
                let content_type = guess_mime_type(&filename);
                let encoded_name = urlencoding::encode(&filename).into_owned();

                Response::builder()
                    .status(200)
                    .header("Content-Type", content_type)
                    .header("Content-Length", bytes.len())
                    .header("X-File-Name", encoded_name)
                    .body(axum::body::Body::from(bytes))
                    .map_err(|e| anyhow!("Failed to build response: {}", e))
            }
            other => Err(anyhow!("Action not supported: {}", other)),
        }
    }
}

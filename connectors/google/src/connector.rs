use std::sync::Arc;

use crate::admin::AdminClient;
use crate::auth::{create_service_auth, get_domain_from_credentials, GoogleAuth};
use crate::drive::DriveClient;
use crate::gmail::{MessageFormat, MessagePart};
use crate::models::{GoogleConnectorState, GoogleDirectoryUser, SearchUsersResponse};
use crate::sync::SyncManager;
use anyhow::{anyhow, Context, Result};
use async_trait::async_trait;
use axum::response::Response;
use omni_connector_sdk::{
    ActionDefinition, ActionResponse, Connector, OAuthManifestConfig, OAuthScopeSet,
    SearchOperator, ServiceCredential, Source, SourceType, SyncContext, SyncType,
};
use serde_json::{json, Value as JsonValue};
use std::collections::HashMap;
use tracing::debug;

/// Build the composite external_id we use for a Gmail attachment document.
///
/// Format: `{thread_id}:att:{message_id}:{url_encoded_filename}:{size}`.
/// Last two segments are stable across syncs; Gmail's per-request
/// `attachmentId` is intentionally NOT in the id because it changes between
/// `messages.get` calls and would cause the indexer to mint a fresh row on
/// every sync.
pub fn build_attachment_doc_id(
    thread_id: &str,
    message_id: &str,
    filename: &str,
    size: u64,
) -> String {
    format!(
        "{}:att:{}:{}:{}",
        thread_id,
        message_id,
        urlencoding::encode(filename),
        size,
    )
}

pub struct ParsedAttachmentDocId {
    pub message_id: String,
    pub filename: String,
    pub size: u64,
}

fn parse_attachment_doc_id(composite: &str) -> Result<ParsedAttachmentDocId> {
    let (_, right) = composite
        .split_once(":att:")
        .ok_or_else(|| anyhow!("Invalid attachment id (missing ':att:'): {}", composite))?;

    // Right side is `{message_id}:{enc_filename}:{size}`. Peel from the right —
    // the size segment is unambiguous (no colons), the filename is
    // url-encoded so it has no colons either, and message_id is whatever's left.
    let (left_of_size, size_str) = right.rsplit_once(':').ok_or_else(|| {
        anyhow!(
            "Invalid attachment id (expected message_id:filename:size after ':att:'): {}",
            composite
        )
    })?;
    let (message_id, enc_filename) = left_of_size.rsplit_once(':').ok_or_else(|| {
        anyhow!(
            "Invalid attachment id (expected message_id:filename:size after ':att:'): {}",
            composite
        )
    })?;
    if message_id.is_empty() || enc_filename.is_empty() || size_str.is_empty() {
        return Err(anyhow!(
            "Invalid attachment id (empty message_id, filename, or size): {}",
            composite
        ));
    }
    let size = size_str
        .parse::<u64>()
        .with_context(|| format!("Invalid attachment id (size not a number): {}", composite))?;
    let filename = urlencoding::decode(enc_filename)
        .with_context(|| {
            format!(
                "Invalid attachment id (filename not url-decodable): {}",
                composite
            )
        })?
        .into_owned();
    Ok(ParsedAttachmentDocId {
        message_id: message_id.to_string(),
        filename,
        size,
    })
}

fn find_attachment_part_by_name<'a>(
    part: &'a MessagePart,
    filename: &str,
    size: u64,
) -> Option<&'a MessagePart> {
    if let Some(body) = &part.body {
        if part.filename.as_deref() == Some(filename) && body.size == Some(size) {
            return Some(part);
        }
    }
    if let Some(parts) = &part.parts {
        for child in parts {
            if let Some(found) = find_attachment_part_by_name(child, filename, size) {
                return Some(found);
            }
        }
    }
    None
}

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
        debug!("Executing fetch_file with params: {:?}", params);
        let file_id = params
            .get("file_id")
            .and_then(|v| v.as_str())
            .ok_or_else(|| anyhow!("Missing required parameter: file_id"))?;

        // Gmail attachment external_ids carry a `:att:` marker. Drive file IDs
        // never contain colons, so the substring check is a safe dispatcher.
        if file_id.contains(":att:") {
            return self.execute_fetch_attachment(params, creds).await;
        }

        let principal_email = creds
            .principal_email
            .as_deref()
            .ok_or_else(|| anyhow!("Missing principal_email in credentials"))?;

        // TODO: connector impl shouldn't depend on sync_manager for auth wiring.
        // Move `create_auth` (and the per-creds dispatch) into `auth.rs` and call
        // it from here directly.
        let google_auth = self
            .sync_manager
            .create_auth(creds, SourceType::GoogleDrive)
            .await?;
        let drive_client = DriveClient::new();

        let file_meta = drive_client
            .get_file_metadata(&google_auth, principal_email, file_id)
            .await
            .context("Failed to read file metadata")?;
        debug!("Retrieved file metadata: {:?}", file_meta);

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
            debug!(
                "Using export_file to fetch file contents for file_id: {}",
                file_id
            );
            let bytes = drive_client
                .export_file(&google_auth, principal_email, file_id, export_mime)
                .await?;
            (bytes, export_mime.to_string())
        } else {
            debug!(
                "Using download_file_binary to fetch file contents for file_id: {}",
                file_id
            );
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

    async fn execute_fetch_attachment(
        &self,
        params: JsonValue,
        creds: &ServiceCredential,
    ) -> Result<Response> {
        debug!("Executing fetch_attachment with params: {:?}", params);
        let composite_id = params
            .get("file_id")
            .and_then(|v| v.as_str())
            .ok_or_else(|| anyhow!("Missing required parameter: file_id"))?;

        let ParsedAttachmentDocId {
            message_id,
            filename,
            size,
        } = parse_attachment_doc_id(composite_id)?;

        let principal_email = creds
            .principal_email
            .as_deref()
            .ok_or_else(|| anyhow!("Missing principal_email in credentials"))?;

        let google_auth = self
            .sync_manager
            .create_auth(creds, SourceType::Gmail)
            .await?;
        let gmail = self.sync_manager.gmail_client();

        let message = gmail
            .get_message(
                &google_auth,
                principal_email,
                &message_id,
                MessageFormat::Full,
            )
            .await
            .context("Failed to read message metadata")?;

        let payload = message
            .payload
            .as_ref()
            .ok_or_else(|| anyhow!("Message {} has no payload", message_id))?;
        let part = find_attachment_part_by_name(payload, &filename, size).ok_or_else(|| {
            anyhow!(
                "Attachment '{}' (size {}) not found in message {}",
                filename,
                size,
                message_id
            )
        })?;
        let live_attachment_id = part
            .body
            .as_ref()
            .and_then(|b| b.attachment_id.as_deref())
            .ok_or_else(|| {
                anyhow!(
                    "Attachment '{}' in message {} has no attachmentId",
                    filename,
                    message_id
                )
            })?;
        let mime_type = part
            .mime_type
            .clone()
            .unwrap_or_else(|| "application/octet-stream".to_string());

        let bytes = gmail
            .download_attachment(
                &google_auth,
                principal_email,
                &message_id,
                live_attachment_id,
            )
            .await?;

        let resp = Response::builder()
            .status(200)
            .header("Content-Type", &mime_type)
            .header("Content-Length", bytes.len())
            .header("X-File-Name", &filename);
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
                    "Download a file from Google Drive (Workspace files exported to Office format) or a Gmail attachment."
                        .to_string(),
                mode: omni_connector_sdk::ActionMode::Read,
                input_schema: json!({
                    "type": "object",
                    "properties": {
                        "file_id": {
                            "type": "string",
                            "description": "Drive file ID, or a Gmail attachment composite ID (thread_id:att:message_id:attachment_id)"
                        }
                    },
                    "required": ["file_id"]
                }),
                source_types: vec![SourceType::GoogleDrive, SourceType::Gmail],
                admin_only: false,
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
                source_types: vec![SourceType::GoogleDrive],
                admin_only: true,
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

#[cfg(test)]
mod tests {
    use super::{build_attachment_doc_id, parse_attachment_doc_id};

    #[test]
    fn round_trips_simple_filename() {
        let id = build_attachment_doc_id("thread123", "msg456", "report.pdf", 12345);
        let parsed = parse_attachment_doc_id(&id).unwrap();
        assert_eq!(parsed.message_id, "msg456");
        assert_eq!(parsed.filename, "report.pdf");
        assert_eq!(parsed.size, 12345);
    }

    #[test]
    fn round_trips_filename_with_special_chars() {
        // Colons, slashes, spaces, unicode all need to survive encode/decode.
        for filename in [
            "weird:name.pdf",
            "path/with slashes.docx",
            "résumé final.pdf",
            "name with spaces (1).pdf",
        ] {
            let id = build_attachment_doc_id("t", "m", filename, 42);
            let parsed = parse_attachment_doc_id(&id).unwrap();
            assert_eq!(parsed.message_id, "m");
            assert_eq!(parsed.filename, filename);
            assert_eq!(parsed.size, 42);
        }
    }

    #[test]
    fn rejects_missing_att_marker() {
        assert!(parse_attachment_doc_id("thread123:msg456:report.pdf:1234").is_err());
    }

    #[test]
    fn rejects_too_few_segments() {
        assert!(parse_attachment_doc_id("thread123:att:msg456").is_err());
        assert!(parse_attachment_doc_id("thread123:att:msg456:report.pdf").is_err());
    }

    #[test]
    fn rejects_non_numeric_size() {
        assert!(parse_attachment_doc_id("thread123:att:msg456:report.pdf:notanumber").is_err());
    }

    #[test]
    fn rejects_empty_segments() {
        assert!(parse_attachment_doc_id("thread123:att::report.pdf:1234").is_err());
        assert!(parse_attachment_doc_id("thread123:att:msg456::1234").is_err());
        assert!(parse_attachment_doc_id("thread123:att:msg456:report.pdf:").is_err());
    }
}

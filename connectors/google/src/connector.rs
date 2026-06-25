use std::sync::Arc;
use std::time::Duration;

use crate::admin::AdminClient;
use crate::auth::{
    create_service_auth, get_domain_from_credentials, GoogleCredentialPayload,
    GoogleOAuthCredentials,
};
use crate::drive::DriveClient;
use crate::gmail::{MessageFormat, MessagePart};
use crate::models::{GoogleDirectoryUser, GoogleSyncCheckpoint, SearchUsersResponse};
use crate::sync::SyncManager;
use anyhow::{anyhow, Context, Result};
use async_trait::async_trait;
use axum::response::Response;
use omni_connector_sdk::{
    ActionDefinition, ActionResponse, AuthType, Connector, OAuthManifestConfig, OAuthScopeSet,
    SearchOperator, ServiceCredential, ServiceProvider, Source, SourceType, SyncContext,
    SyncRequestValidationError, SyncType,
};
use serde::Deserialize;
use serde_json::{json, Value as JsonValue};
use std::collections::HashMap;
use tokio::process::Command;
use tokio::time::timeout;
use tracing::debug;

const GWS_COMMAND: &str = "gws";
const GWS_TIMEOUT: Duration = Duration::from_secs(60);
const GOOGLE_WORKSPACE_CLI_TOKEN: &str = "GOOGLE_WORKSPACE_CLI_TOKEN";
const GOOGLE_DRIVE_READ_SCOPE: &str = "https://www.googleapis.com/auth/drive.readonly";
const GOOGLE_DRIVE_WRITE_SCOPE: &str = "https://www.googleapis.com/auth/drive.file";
const GMAIL_READ_SCOPE: &str = "https://www.googleapis.com/auth/gmail.readonly";
const GMAIL_SEND_SCOPE: &str = "https://www.googleapis.com/auth/gmail.send";
const GMAIL_MODIFY_SCOPE: &str = "https://www.googleapis.com/auth/gmail.modify";
const GOOGLE_WORKSPACE_SCOPES: &[&str] = &[
    "https://www.googleapis.com/auth/admin.directory.user.readonly",
    "https://www.googleapis.com/auth/admin.directory.group.readonly",
    GOOGLE_DRIVE_READ_SCOPE,
    GMAIL_READ_SCOPE,
];

#[derive(Debug, Deserialize)]
struct GwsSchemaRequest {
    schema: String,
    #[serde(default = "default_resolve_refs")]
    resolve_refs: bool,
}

#[derive(Debug, Deserialize)]
struct GwsCallRequest {
    service: String,
    resource: String,
    #[serde(default)]
    sub_resource: Option<String>,
    method: String,
    #[serde(default)]
    params: Option<JsonValue>,
    #[serde(default)]
    body: Option<JsonValue>,
    #[serde(default)]
    api_version: Option<String>,
    #[serde(default)]
    page_all: bool,
    #[serde(default)]
    page_limit: Option<u64>,
}

fn default_resolve_refs() -> bool {
    true
}

fn file_name_with_extension(file_name: &str, extension: &str) -> String {
    if file_name
        .to_ascii_lowercase()
        .ends_with(&extension.to_ascii_lowercase())
    {
        file_name.to_string()
    } else {
        format!("{file_name}{extension}")
    }
}

/// Build the composite external_id we use for a Gmail attachment document.
///
/// Format: `{url_encoded_rfc822_msgid}:att:{url_encoded_filename}:{size}`.
///
/// The `rfc822_msgid` is the canonical "Message-ID" header value of the
/// message that holds the attachment, with surrounding `<>` stripped. It is
/// stable across mailboxes (set by the sender), unlike Gmail's per-mailbox
/// `messageId` and `attachmentId`. At fetch time we resolve it to the
/// requesting user's local Gmail message id via
/// `messages.list?q=rfc822msgid:<id>`, then walk parts to find the matching
/// attachment by `(filename, size)` and use that part's live `attachmentId`.
pub fn build_attachment_doc_id(rfc822_msgid: &str, filename: &str, size: u64) -> String {
    format!(
        "{}:att:{}:{}",
        urlencoding::encode(rfc822_msgid),
        urlencoding::encode(filename),
        size,
    )
}

pub struct ParsedAttachmentDocId {
    pub rfc822_msgid: String,
    pub filename: String,
    pub size: u64,
}

fn parse_attachment_doc_id(composite: &str) -> Result<ParsedAttachmentDocId> {
    let (enc_msgid, right) = composite
        .split_once(":att:")
        .ok_or_else(|| anyhow!("Invalid attachment id (missing ':att:'): {}", composite))?;

    // Right side is `{enc_filename}:{size}`. Filename is url-encoded so it
    // contains no colons; size is a clean integer.
    let (enc_filename, size_str) = right.rsplit_once(':').ok_or_else(|| {
        anyhow!(
            "Invalid attachment id (expected filename:size after ':att:'): {}",
            composite
        )
    })?;
    if enc_msgid.is_empty() || enc_filename.is_empty() || size_str.is_empty() {
        return Err(anyhow!(
            "Invalid attachment id (empty rfc822_msgid, filename, or size): {}",
            composite
        ));
    }
    let size = size_str
        .parse::<u64>()
        .with_context(|| format!("Invalid attachment id (size not a number): {}", composite))?;
    let rfc822_msgid = urlencoding::decode(enc_msgid)
        .with_context(|| {
            format!(
                "Invalid attachment id (rfc822_msgid not url-decodable): {}",
                composite
            )
        })?
        .into_owned();
    let filename = urlencoding::decode(enc_filename)
        .with_context(|| {
            format!(
                "Invalid attachment id (filename not url-decodable): {}",
                composite
            )
        })?
        .into_owned();
    Ok(ParsedAttachmentDocId {
        rfc822_msgid,
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

fn validate_gws_token(value: &str, label: &str) -> Result<()> {
    if value.is_empty()
        || !value
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'))
    {
        return Err(anyhow!(
            "{} must contain only ASCII letters, numbers, hyphen, underscore, or dot",
            label
        ));
    }
    Ok(())
}

fn push_gws_path(args: &mut Vec<String>, value: &str, label: &str) -> Result<()> {
    for segment in value.split('.') {
        validate_gws_token(segment, label)?;
        args.push(segment.to_string());
    }
    Ok(())
}

fn build_gws_schema_args(request: &GwsSchemaRequest) -> Result<Vec<String>> {
    if request.schema.is_empty() {
        return Err(anyhow!("schema is required"));
    }
    for segment in request.schema.split('.') {
        validate_gws_token(segment, "schema")?;
    }

    let mut args = vec!["schema".to_string(), request.schema.clone()];
    if request.resolve_refs {
        args.push("--resolve-refs".to_string());
    }
    Ok(args)
}

fn build_gws_call_args(request: &GwsCallRequest) -> Result<Vec<String>> {
    validate_gws_token(&request.service, "service")?;
    validate_gws_token(&request.method, "method")?;

    let mut args = vec![request.service.clone()];
    push_gws_path(&mut args, &request.resource, "resource")?;
    if let Some(sub_resource) = &request.sub_resource {
        push_gws_path(&mut args, sub_resource, "sub_resource")?;
    }
    args.push(request.method.clone());

    if let Some(params) = &request.params {
        if !params.is_object() {
            return Err(anyhow!("params must be a JSON object"));
        }
        args.push("--params".to_string());
        args.push(serde_json::to_string(params)?);
    }
    if let Some(body) = &request.body {
        if !body.is_object() {
            return Err(anyhow!("body must be a JSON object"));
        }
        args.push("--json".to_string());
        args.push(serde_json::to_string(body)?);
    }
    if let Some(api_version) = &request.api_version {
        validate_gws_token(api_version, "api_version")?;
        args.push("--api-version".to_string());
        args.push(api_version.clone());
    }
    if request.page_all {
        args.push("--page-all".to_string());
    }
    if let Some(page_limit) = request.page_limit {
        args.push("--page-limit".to_string());
        args.push(page_limit.to_string());
    }

    Ok(args)
}

fn gws_action_response(
    success: bool,
    exit_code: Option<i32>,
    stdout: &str,
    stderr: &str,
) -> ActionResponse {
    if !success {
        return ActionResponse::failure(
            json!({
                "exit_code": exit_code,
                "stdout": stdout,
                "stderr": stderr,
            })
            .to_string(),
        );
    }

    let result = if stdout.is_empty() {
        json!({})
    } else {
        serde_json::from_str(stdout).unwrap_or_else(|_| json!({ "content": stdout }))
    };

    ActionResponse::success(result)
}

fn gws_required_action_scopes(service: &str) -> Result<(&'static str, &'static [&'static str])> {
    match service {
        "drive" => Ok((
            "google_drive",
            &[GOOGLE_DRIVE_READ_SCOPE, GOOGLE_DRIVE_WRITE_SCOPE],
        )),
        "gmail" => Ok((
            "gmail",
            &[GMAIL_READ_SCOPE, GMAIL_SEND_SCOPE, GMAIL_MODIFY_SCOPE],
        )),
        _ => Err(anyhow!(
            "Unsupported Google Workspace service for OAuth action bridge: {}",
            service
        )),
    }
}

fn granted_scopes(credentials: &ServiceCredential) -> Vec<&str> {
    credentials
        .config
        .get("granted_scopes")
        .and_then(|v| v.as_array())
        .map(|scopes| scopes.iter().filter_map(|s| s.as_str()).collect())
        .unwrap_or_default()
}

fn missing_gws_call_scopes(
    request: &GwsCallRequest,
    credentials: &ServiceCredential,
) -> Result<Vec<String>> {
    if credentials.auth_type != AuthType::OAuth || credentials.user_id.is_none() {
        return Ok(Vec::new());
    }

    let (_, required_scopes) = gws_required_action_scopes(&request.service)?;
    let granted_scopes = granted_scopes(credentials);
    Ok(required_scopes
        .iter()
        .filter(|scope| !granted_scopes.contains(scope))
        .map(|scope| (*scope).to_string())
        .collect())
}

fn missing_scope_response(
    credentials: &ServiceCredential,
    source_type: &str,
    missing_scopes: Vec<String>,
) -> Result<Response> {
    use axum::http::StatusCode;

    let body = json!({
        "error": "needs_user_auth",
        "reason": "missing_scopes",
        "source_id": credentials.source_id,
        "source_type": source_type,
        "provider": credentials.provider,
        "oauth_start_url": format!(
            "/api/oauth/start?source_id={}",
            urlencoding::encode(&credentials.source_id),
        ),
        "missing_scopes": missing_scopes,
    });

    Response::builder()
        .status(StatusCode::PRECONDITION_FAILED)
        .header("content-type", "application/json")
        .body(axum::body::Body::from(body.to_string()))
        .map_err(|e| anyhow!("Failed to build missing-scope response: {}", e))
}

impl GoogleConnector {
    pub fn new(sync_manager: Arc<SyncManager>, admin_client: Arc<AdminClient>) -> Self {
        Self {
            sync_manager,
            admin_client,
        }
    }

    fn gws_service_credential(&self, credentials: &ServiceCredential) -> ServiceCredential {
        let config = if credentials.auth_type == AuthType::Jwt
            && credentials.config.get("scopes").is_none()
        {
            let mut config = credentials.config.as_object().cloned().unwrap_or_default();
            config.insert("scopes".to_string(), json!(GOOGLE_WORKSPACE_SCOPES));
            JsonValue::Object(config)
        } else {
            credentials.config.clone()
        };

        ServiceCredential {
            id: credentials.id.clone(),
            source_id: credentials.source_id.clone(),
            user_id: credentials.user_id.clone(),
            provider: ServiceProvider::Google,
            auth_type: credentials.auth_type,
            principal_email: credentials.principal_email.clone(),
            credentials: credentials.credentials.clone(),
            config,
            expires_at: credentials.expires_at,
            last_validated_at: credentials.last_validated_at,
            created_at: credentials.created_at,
            updated_at: credentials.updated_at,
        }
    }

    async fn gws_token(&self, credentials: &ServiceCredential) -> Result<String> {
        let service_credential = self.gws_service_credential(credentials);
        let auth = self
            .sync_manager
            .create_auth(&service_credential, SourceType::GoogleDrive)
            .await?;
        let principal_email = auth
            .oauth_user_email()
            .or(service_credential.principal_email.as_deref())
            .ok_or_else(|| anyhow!("Missing principal_email in Google Workspace credentials"))?;

        auth.get_fresh_token(principal_email).await
    }

    async fn execute_gws_args(
        &self,
        args: Vec<String>,
        creds: &ServiceCredential,
    ) -> Result<Response> {
        let token = self.gws_token(creds).await?;
        debug!("Executing gws with args: {:?}", args);

        let mut command = Command::new(GWS_COMMAND);
        command
            .args(&args)
            .env(GOOGLE_WORKSPACE_CLI_TOKEN, token)
            .kill_on_drop(true);

        let output = timeout(GWS_TIMEOUT, command.output())
            .await
            .map_err(|_| {
                anyhow!(
                    "gws command timed out after {} seconds",
                    GWS_TIMEOUT.as_secs()
                )
            })?
            .context("Failed to execute gws command")?;

        let stdout = String::from_utf8_lossy(&output.stdout).trim().to_string();
        let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
        Ok(gws_action_response(
            output.status.success(),
            output.status.code(),
            &stdout,
            &stderr,
        )
        .into_response())
    }

    async fn execute_gws_schema(
        &self,
        params: JsonValue,
        creds: &ServiceCredential,
    ) -> Result<Response> {
        let request: GwsSchemaRequest =
            serde_json::from_value(params).context("Invalid google_workspace_schema params")?;
        let args = build_gws_schema_args(&request)?;
        self.execute_gws_args(args, creds).await
    }

    async fn execute_gws_call(
        &self,
        params: JsonValue,
        creds: &ServiceCredential,
    ) -> Result<Response> {
        let request: GwsCallRequest =
            serde_json::from_value(params).context("Invalid google_workspace_call params")?;
        let args = build_gws_call_args(&request)?;
        let (source_type, _) = gws_required_action_scopes(&request.service)?;
        let missing_scopes = missing_gws_call_scopes(&request, creds)?;
        if !missing_scopes.is_empty() {
            return missing_scope_response(creds, source_type, missing_scopes);
        }
        self.execute_gws_args(args, creds).await
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

        let (bytes, content_type, response_file_name) =
            if let Some((export_mime, ext)) = export_mapping {
                debug!(
                    "Using export_file to fetch file contents for file_id: {}",
                    file_id
                );
                let bytes = drive_client
                    .export_file(&google_auth, principal_email, file_id, export_mime)
                    .await?;
                (
                    bytes,
                    export_mime.to_string(),
                    file_name_with_extension(file_name, ext),
                )
            } else {
                debug!(
                    "Using download_file_binary to fetch file contents for file_id: {}",
                    file_id
                );
                let bytes = drive_client
                    .download_file_binary(&google_auth, principal_email, file_id)
                    .await?;
                (bytes, mime_type.clone(), file_name.clone())
            };

        let resp = Response::builder()
            .status(200)
            .header("Content-Type", content_type)
            .header("Content-Length", bytes.len())
            .header("X-File-Name", response_file_name);
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
            rfc822_msgid,
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

        // Hop 1: resolve the requesting user's local Gmail message_id via the
        // canonical RFC 822 Message-ID. Gmail's own message_ids are per-mailbox,
        // so we never persist them; only the rfc822 id is stable across users.
        let query = format!("rfc822msgid:{}", rfc822_msgid);
        let list = gmail
            .list_messages(
                &google_auth,
                principal_email,
                Some(&query),
                Some(1),
                None,
                None,
            )
            .await
            .context("Failed to search for message by rfc822msgid")?;
        let local_msg_id = list
            .messages
            .as_ref()
            .and_then(|m| m.first())
            .map(|m| m.id.clone())
            .ok_or_else(|| {
                anyhow!(
                    "Attachment '{}' not found in {}'s mailbox (rfc822msgid: {})",
                    filename,
                    principal_email,
                    rfc822_msgid
                )
            })?;

        // Hop 2: fetch the resolved message and walk parts to find the
        // attachment matching (filename, size).
        let message = gmail
            .get_message(
                &google_auth,
                principal_email,
                &local_msg_id,
                MessageFormat::Full,
            )
            .await
            .context("Failed to read message metadata")?;
        let payload = message
            .payload
            .as_ref()
            .ok_or_else(|| anyhow!("Message {} has no payload", local_msg_id))?;
        let part = find_attachment_part_by_name(payload, &filename, size).ok_or_else(|| {
            anyhow!(
                "Attachment '{}' (size {}) not found in resolved message {}",
                filename,
                size,
                local_msg_id
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
                    local_msg_id
                )
            })?;
        let mime_type = part
            .mime_type
            .clone()
            .unwrap_or_else(|| "application/octet-stream".to_string());

        // Hop 3: download bytes using the part's live attachmentId.
        let bytes = gmail
            .download_attachment(
                &google_auth,
                principal_email,
                &local_msg_id,
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
    type Credentials = GoogleCredentialPayload;
    type State = GoogleSyncCheckpoint;

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
        Some("Connect to Google Drive, Docs, Gmail, Google Chat, and more".to_string())
    }

    fn source_types(&self) -> Vec<SourceType> {
        vec![
            SourceType::GoogleDrive,
            SourceType::Gmail,
            SourceType::GoogleChat,
        ]
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
                            "description": "Drive file ID, or a Gmail attachment composite ID in the form urlencoded_rfc822_msgid:att:urlencoded_filename:size"
                        }
                    },
                    "required": ["file_id"]
                }),
                source_types: vec![SourceType::GoogleDrive, SourceType::Gmail],
                admin_only: false,
                hidden: false,
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
                source_types: vec![SourceType::GoogleDrive, SourceType::GoogleChat],
                admin_only: true,
                hidden: false,
            },
            ActionDefinition {
                name: "google_workspace_schema".to_string(),
                description: "Inspect the JSON schema for a Google Workspace CLI method"
                    .to_string(),
                mode: omni_connector_sdk::ActionMode::Read,
                input_schema: json!({
                    "type": "object",
                    "properties": {
                        "schema": {
                            "type": "string",
                            "description": "Google Workspace CLI schema name, e.g. drive.files.list or gmail.users.messages.get"
                        },
                        "resolve_refs": {
                            "type": "boolean",
                            "default": true,
                            "description": "Whether to resolve schema references"
                        }
                    },
                    "required": ["schema"]
                }),
                source_types: vec![SourceType::GoogleDrive, SourceType::Gmail],
                admin_only: false,
                hidden: false,
            },
            ActionDefinition {
                name: "google_workspace_call".to_string(),
                description: "Call a Google Workspace API through the installed gws CLI"
                    .to_string(),
                mode: omni_connector_sdk::ActionMode::Write,
                input_schema: json!({
                    "type": "object",
                    "properties": {
                        "service": {
                            "type": "string",
                            "description": "Google Workspace service, currently drive or gmail"
                        },
                        "resource": {
                            "type": "string",
                            "description": "Resource path, dot-separated for nested resources, e.g. files or users.messages"
                        },
                        "sub_resource": {
                            "type": "string",
                            "description": "Optional additional nested resource path"
                        },
                        "method": {
                            "type": "string",
                            "description": "Method name, e.g. list, get, send"
                        },
                        "params": {
                            "type": "object",
                            "description": "URL/query parameters passed to gws --params"
                        },
                        "body": {
                            "type": "object",
                            "description": "Request body passed to gws --json"
                        },
                        "api_version": {
                            "type": "string",
                            "description": "Optional API version override"
                        },
                        "page_all": {
                            "type": "boolean",
                            "default": false,
                            "description": "Whether gws should auto-paginate"
                        },
                        "page_limit": {
                            "type": "integer",
                            "description": "Maximum pages to fetch when page_all is true"
                        }
                    },
                    "required": ["service", "resource", "method"]
                }),
                source_types: vec![SourceType::GoogleDrive, SourceType::Gmail],
                admin_only: false,
                hidden: false,
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
                operator: "to".to_string(),
                attribute_key: "to".to_string(),
                value_type: "person".to_string(),
            },
            SearchOperator {
                operator: "label".to_string(),
                attribute_key: "labels".to_string(),
                value_type: "text".to_string(),
            },
            SearchOperator {
                operator: "space".to_string(),
                attribute_key: "space".to_string(),
                value_type: "text".to_string(),
            },
            SearchOperator {
                operator: "thread".to_string(),
                attribute_key: "threads".to_string(),
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
                // drive.file scopes the grant to files the app creates/opens,
                // which is the safe default for Workspace write tools.
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

    async fn validate_sync_request(
        &self,
        source: &Source,
        credentials: Option<&ServiceCredential>,
        _sync_type: SyncType,
    ) -> std::result::Result<(), SyncRequestValidationError> {
        let Some(creds) = credentials else {
            return Err(SyncRequestValidationError::BadRequest(
                "Google sync requires credentials".to_string(),
            ));
        };
        if creds.provider != ServiceProvider::Google {
            return Err(SyncRequestValidationError::BadRequest(format!(
                "Expected Google credentials, found {:?}",
                creds.provider
            )));
        }

        match creds.auth_type {
            omni_connector_sdk::AuthType::OAuth => {
                let oauth_credentials: GoogleOAuthCredentials =
                    serde_json::from_value(creds.credentials.clone()).map_err(|e| {
                        SyncRequestValidationError::BadRequest(format!(
                            "Invalid Google OAuth credentials: {}",
                            e
                        ))
                    })?;
                if oauth_credentials.refresh_token.is_empty()
                    || oauth_credentials
                        .user_email
                        .as_deref()
                        .or(creds.principal_email.as_deref())
                        .is_none_or(|email| email.is_empty())
                {
                    return Err(SyncRequestValidationError::BadRequest(
                        "OAuth Google credentials must include refresh_token and user_email/principal_email".to_string(),
                    ));
                }
            }
            _ => {
                create_service_auth(creds, source.source_type).map_err(|e| {
                    SyncRequestValidationError::BadRequest(format!(
                        "Invalid Google service-account credentials: {}",
                        e
                    ))
                })?;
                get_domain_from_credentials(creds).map_err(|e| {
                    SyncRequestValidationError::BadRequest(format!(
                        "Invalid Google service-account config: {}",
                        e
                    ))
                })?;
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
                .into_response());
            }
        };
        match action {
            "fetch_file" => self.execute_fetch_file(params, &creds).await,
            "search_users" => self.execute_search_users(params, &creds).await,
            "google_workspace_schema" => self.execute_gws_schema(params, &creds).await,
            "google_workspace_call" => self.execute_gws_call(params, &creds).await,
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
    use std::sync::Arc;

    use omni_connector_sdk::{AuthType, Connector, SdkClient, ServiceCredential, ServiceProvider};
    use serde_json::json;

    use crate::admin::AdminClient;
    use crate::sync::SyncManager;

    use super::{
        build_attachment_doc_id, build_gws_call_args, build_gws_schema_args,
        file_name_with_extension, gws_action_response, gws_required_action_scopes,
        missing_gws_call_scopes, parse_attachment_doc_id, GoogleConnector, GwsCallRequest,
        GwsSchemaRequest, GMAIL_MODIFY_SCOPE, GMAIL_READ_SCOPE, GMAIL_SEND_SCOPE,
        GOOGLE_DRIVE_READ_SCOPE, GOOGLE_DRIVE_WRITE_SCOPE,
    };

    fn test_connector() -> GoogleConnector {
        let sdk_client = SdkClient::new("http://127.0.0.1:1");
        let admin_client = Arc::new(AdminClient::new());
        let sync_manager = Arc::new(SyncManager::new(
            Arc::clone(&admin_client),
            sdk_client,
            None,
        ));
        GoogleConnector::new(sync_manager, admin_client)
    }

    fn test_service_credential(
        auth_type: AuthType,
        config: serde_json::Value,
    ) -> ServiceCredential {
        let now = time::OffsetDateTime::now_utc();
        ServiceCredential {
            id: "credential".to_string(),
            source_id: "source".to_string(),
            user_id: Some("user".to_string()),
            provider: ServiceProvider::Google,
            auth_type,
            principal_email: Some("admin@example.com".to_string()),
            credentials: json!({"service_account_key": "{}"}),
            config,
            expires_at: None,
            last_validated_at: None,
            created_at: now,
            updated_at: now,
        }
    }

    #[test]
    fn google_connector_does_not_use_mcp_server() {
        let connector = test_connector();
        assert!(connector.mcp_server().is_none());
    }

    #[test]
    fn manifest_includes_google_workspace_bridge_actions() {
        let connector = test_connector();
        let actions = connector.actions();
        assert!(actions.iter().any(|a| a.name == "google_workspace_schema"));
        assert!(actions.iter().any(|a| a.name == "google_workspace_call"));
    }

    #[test]
    fn gws_service_credential_preserves_oauth_auth_type() {
        let connector = test_connector();
        let credential = test_service_credential(AuthType::OAuth, json!({}));
        let creds = connector.gws_service_credential(&credential);

        assert_eq!(creds.auth_type, AuthType::OAuth);
        assert_eq!(creds.id, "credential");
        assert_eq!(creds.user_id.as_deref(), Some("user"));
    }

    #[test]
    fn gws_service_credential_adds_combined_scopes_for_service_accounts() {
        let connector = test_connector();
        let credential = test_service_credential(AuthType::Jwt, json!({}));
        let creds = connector.gws_service_credential(&credential);

        assert_eq!(creds.auth_type, AuthType::Jwt);
        let scopes = creds.config["scopes"].as_array().expect("scopes");
        assert!(scopes
            .iter()
            .any(|s| s == "https://www.googleapis.com/auth/drive.readonly"));
        assert!(scopes
            .iter()
            .any(|s| s == "https://www.googleapis.com/auth/gmail.readonly"));
    }

    #[test]
    fn file_name_with_extension_appends_missing_extension() {
        assert_eq!(
            file_name_with_extension("Dummy Document", ".docx"),
            "Dummy Document.docx"
        );
    }

    #[test]
    fn file_name_with_extension_does_not_duplicate_extension() {
        assert_eq!(
            file_name_with_extension("Dummy Document.DOCX", ".docx"),
            "Dummy Document.DOCX"
        );
    }

    #[test]
    fn build_gws_schema_args_resolves_refs_by_default() {
        let args = build_gws_schema_args(&GwsSchemaRequest {
            schema: "drive.files.list".to_string(),
            resolve_refs: true,
        })
        .unwrap();

        assert_eq!(args, ["schema", "drive.files.list", "--resolve-refs"]);
    }

    #[test]
    fn build_gws_call_args_uses_positional_path_and_optional_flags() {
        let args = build_gws_call_args(&GwsCallRequest {
            service: "gmail".to_string(),
            resource: "users.messages".to_string(),
            sub_resource: None,
            method: "list".to_string(),
            params: Some(json!({"userId": "me"})),
            body: None,
            api_version: Some("v1".to_string()),
            page_all: true,
            page_limit: Some(2),
        })
        .unwrap();

        assert_eq!(
            args,
            [
                "gmail",
                "users",
                "messages",
                "list",
                "--params",
                "{\"userId\":\"me\"}",
                "--api-version",
                "v1",
                "--page-all",
                "--page-limit",
                "2"
            ]
        );
    }

    #[test]
    fn build_gws_call_args_rejects_non_object_params() {
        let err = build_gws_call_args(&GwsCallRequest {
            service: "drive".to_string(),
            resource: "files".to_string(),
            sub_resource: None,
            method: "list".to_string(),
            params: Some(json!("not-an-object")),
            body: None,
            api_version: None,
            page_all: false,
            page_limit: None,
        })
        .unwrap_err();

        assert!(err.to_string().contains("params must be a JSON object"));
    }

    #[test]
    fn gws_required_action_scopes_rejects_unsupported_services() {
        let err = gws_required_action_scopes("calendar").unwrap_err();

        assert!(err
            .to_string()
            .contains("Unsupported Google Workspace service"));
    }

    #[test]
    fn missing_gws_call_scopes_detects_read_only_drive_oauth() {
        let request = GwsCallRequest {
            service: "drive".to_string(),
            resource: "files".to_string(),
            sub_resource: None,
            method: "create".to_string(),
            params: None,
            body: Some(json!({"name": "Budget"})),
            api_version: None,
            page_all: false,
            page_limit: None,
        };
        let credential = test_service_credential(
            AuthType::OAuth,
            json!({"granted_scopes": [GOOGLE_DRIVE_READ_SCOPE]}),
        );

        let missing = missing_gws_call_scopes(&request, &credential).unwrap();

        assert_eq!(missing, [GOOGLE_DRIVE_WRITE_SCOPE]);
    }

    #[test]
    fn missing_gws_call_scopes_accepts_full_gmail_oauth() {
        let request = GwsCallRequest {
            service: "gmail".to_string(),
            resource: "users.messages".to_string(),
            sub_resource: None,
            method: "send".to_string(),
            params: None,
            body: Some(json!({"raw": "abc"})),
            api_version: None,
            page_all: false,
            page_limit: None,
        };
        let credential = test_service_credential(
            AuthType::OAuth,
            json!({"granted_scopes": [GMAIL_READ_SCOPE, GMAIL_SEND_SCOPE, GMAIL_MODIFY_SCOPE]}),
        );

        let missing = missing_gws_call_scopes(&request, &credential).unwrap();

        assert!(missing.is_empty());
    }

    #[test]
    fn missing_gws_call_scopes_skips_service_account_credentials() {
        let request = GwsCallRequest {
            service: "drive".to_string(),
            resource: "files".to_string(),
            sub_resource: None,
            method: "create".to_string(),
            params: None,
            body: Some(json!({"name": "Budget"})),
            api_version: None,
            page_all: false,
            page_limit: None,
        };
        let credential = test_service_credential(AuthType::Jwt, json!({}));

        let missing = missing_gws_call_scopes(&request, &credential).unwrap();

        assert!(missing.is_empty());
    }

    #[test]
    fn gws_action_response_maps_nonzero_exit_to_failure() {
        let response = gws_action_response(false, Some(3), "", "bad args");

        assert_eq!(response.status, "error");
        let error = response.error.expect("error");
        assert!(error.contains("\"exit_code\":3"));
        assert!(error.contains("\"stderr\":\"bad args\""));
    }

    #[test]
    fn gws_action_response_parses_json_stdout() {
        let response = gws_action_response(true, Some(0), "{\"ok\":true}", "");

        assert_eq!(response.status, "success");
        assert_eq!(response.result, Some(json!({"ok": true})));
    }

    #[test]
    fn round_trips_simple_msgid() {
        let id = build_attachment_doc_id("CABc123@mail.example.test", "report.pdf", 12345);
        let parsed = parse_attachment_doc_id(&id).unwrap();
        assert_eq!(parsed.rfc822_msgid, "CABc123@mail.example.test");
        assert_eq!(parsed.filename, "report.pdf");
        assert_eq!(parsed.size, 12345);
    }

    #[test]
    fn round_trips_msgid_and_filename_with_special_chars() {
        // Real-world rfc822 Message-IDs contain @, +, =, .;
        // filenames may contain colons, slashes, unicode, parens.
        let cases = [
            (
                "MA0P287MB3036D91CF4E25D0F29D4941BF3262@mailbox.example.test",
                "weird:name.pdf",
            ),
            (
                "0108019cd78ca34a-33533383-c422+42e2-9016-0632c1a2f408-000000@mailer.example.test",
                "path/with slashes.docx",
            ),
            (
                "<unique-id+tag=value@example.com>"
                    .trim_start_matches('<')
                    .trim_end_matches('>'),
                "résumé final.pdf",
            ),
            ("abc.def.ghi@example.com", "name with spaces (1).pdf"),
        ];
        for (msgid, filename) in cases {
            let id = build_attachment_doc_id(msgid, filename, 42);
            let parsed = parse_attachment_doc_id(&id).unwrap();
            assert_eq!(parsed.rfc822_msgid, msgid, "msgid round-trip failed");
            assert_eq!(parsed.filename, filename, "filename round-trip failed");
            assert_eq!(parsed.size, 42);
        }
    }

    #[test]
    fn rejects_missing_att_marker() {
        assert!(parse_attachment_doc_id("CABc123@mail.example.test:report.pdf:1234").is_err());
    }

    #[test]
    fn rejects_too_few_segments() {
        // Missing filename:size after :att:
        assert!(parse_attachment_doc_id("CABc123%40mail.example.test:att:report.pdf").is_err());
    }

    #[test]
    fn rejects_non_numeric_size() {
        assert!(
            parse_attachment_doc_id("CABc123%40mail.example.test:att:report.pdf:notanumber")
                .is_err()
        );
    }

    #[test]
    fn rejects_empty_segments() {
        // Empty rfc822_msgid
        assert!(parse_attachment_doc_id(":att:report.pdf:1234").is_err());
        // Empty filename
        assert!(parse_attachment_doc_id("CABc123%40mail.example.test:att::1234").is_err());
        // Empty size
        assert!(parse_attachment_doc_id("CABc123%40mail.example.test:att:report.pdf:").is_err());
    }
}

use std::collections::HashMap;
use std::path::PathBuf;
use std::time::Duration;

use anyhow::{Context, Result};
use rmcp::model::{
    CallToolRequestParams, GetPromptRequestParams, PaginatedRequestParams, PromptMessageContent,
    RawContent, ReadResourceRequestParams, ResourceContents,
};
use rmcp::service::RunningService;
use rmcp::transport::streamable_http_client::StreamableHttpClientTransportConfig;
use rmcp::transport::{StreamableHttpClientTransport, TokioChildProcess};
use rmcp::ServiceExt;
use serde::{Deserialize, Serialize};
use serde_json::Value as JsonValue;
use shared::models::{
    ActionDefinition, ActionMode, McpPromptArgument, McpPromptDefinition, McpResourceDefinition,
    ServiceCredential,
};
use tokio::process::Command;
use tokio::sync::RwLock;
use tracing::{debug, info, warn};

use crate::models::ActionResponse;

/// Wire-format credentials forwarded by the connector-manager to `/resource`,
/// `/prompt`, and (via the SDK's bootstrap path) to the connector's
/// `prepare_mcp_*` methods. Mirrors the JSON shape produced by
/// `services/connector-manager/src/handlers.rs:read_resource`/`get_prompt`:
/// `{credentials, config, principal_email}`.
///
/// Connectors typically deserialize `credentials` further into their own
/// typed credentials struct.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct McpCredentials {
    /// The provider-specific credentials blob (e.g. `{token: "..."}`).
    #[serde(default)]
    pub credentials: JsonValue,
    /// The provider-specific service config blob.
    #[serde(default)]
    pub config: JsonValue,
    /// Optional acting-user email (for delegated/principal-aware connectors).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub principal_email: Option<String>,
}

impl McpCredentials {
    /// Convert a typed `ServiceCredential` into the wrapper shape that
    /// `prepare_mcp_*` expects, matching what the connector-manager would
    /// send to `/resource` and `/prompt`.
    pub fn from_service_credential(creds: &ServiceCredential) -> Self {
        Self {
            credentials: creds.credentials.clone(),
            config: creds.config.clone(),
            principal_email: creds.principal_email.clone(),
        }
    }
}

/// Configuration for an MCP server reached via stdio (subprocess).
#[derive(Debug, Clone)]
pub struct StdioMcpServer {
    pub command: String,
    pub args: Vec<String>,
    pub env: HashMap<String, String>,
    pub cwd: Option<PathBuf>,
}

impl StdioMcpServer {
    pub fn new(command: impl Into<String>) -> Self {
        Self {
            command: command.into(),
            args: Vec::new(),
            env: HashMap::new(),
            cwd: None,
        }
    }

    pub fn with_args<I, S>(mut self, args: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        self.args = args.into_iter().map(Into::into).collect();
        self
    }
}

/// Configuration for a remote MCP server reached via Streamable HTTP.
#[derive(Debug, Clone)]
pub struct HttpMcpServer {
    pub url: String,
    pub headers: HashMap<String, String>,
    pub timeout: Duration,
}

impl HttpMcpServer {
    pub fn new(url: impl Into<String>) -> Self {
        Self {
            url: url.into(),
            headers: HashMap::new(),
            timeout: Duration::from_secs(30),
        }
    }
}

#[derive(Debug, Clone)]
pub enum McpServer {
    Stdio(StdioMcpServer),
    Http(HttpMcpServer),
}

/// Bridges an external MCP server into Omni's connector protocol.
///
/// Each operation opens a fresh client/transport pair and tears it down
/// afterwards. Tool/resource/prompt definitions are cached after the first
/// successful discovery so manifest builds don't require live auth.
pub struct McpAdapter {
    server: McpServer,
    cached_actions: RwLock<Option<Vec<ActionDefinition>>>,
    cached_resources: RwLock<Option<Vec<McpResourceDefinition>>>,
    cached_prompts: RwLock<Option<Vec<McpPromptDefinition>>>,
}

type RmcpClient = RunningService<rmcp::RoleClient, ()>;

impl McpAdapter {
    pub fn new(server: McpServer) -> Self {
        Self {
            server,
            cached_actions: RwLock::new(None),
            cached_resources: RwLock::new(None),
            cached_prompts: RwLock::new(None),
        }
    }

    async fn connect(
        &self,
        env: Option<HashMap<String, String>>,
        headers: Option<HashMap<String, String>>,
    ) -> Result<RmcpClient> {
        match &self.server {
            McpServer::Stdio(stdio) => {
                let mut cmd = Command::new(&stdio.command);
                cmd.args(&stdio.args);
                for (k, v) in &stdio.env {
                    cmd.env(k, v);
                }
                if let Some(extra) = env {
                    for (k, v) in extra {
                        cmd.env(k, v);
                    }
                }
                if let Some(cwd) = &stdio.cwd {
                    cmd.current_dir(cwd);
                }
                debug!(
                    "Spawning MCP subprocess: {} {}",
                    stdio.command,
                    stdio.args.join(" ")
                );
                let transport =
                    TokioChildProcess::new(cmd).context("failed to spawn MCP child process")?;
                ().serve(transport)
                    .await
                    .context("MCP stdio handshake failed")
            }
            McpServer::Http(http) => {
                let mut header_map: HashMap<http::HeaderName, http::HeaderValue> = HashMap::new();
                for (k, v) in &http.headers {
                    header_map.insert(
                        http::HeaderName::from_bytes(k.as_bytes())
                            .with_context(|| format!("invalid header name '{}'", k))?,
                        http::HeaderValue::from_str(v)
                            .with_context(|| format!("invalid header value for '{}'", k))?,
                    );
                }
                if let Some(extra) = headers {
                    for (k, v) in extra {
                        header_map.insert(
                            http::HeaderName::from_bytes(k.as_bytes())
                                .with_context(|| format!("invalid header name '{}'", k))?,
                            http::HeaderValue::from_str(&v)
                                .with_context(|| format!("invalid header value for '{}'", k))?,
                        );
                    }
                }
                let config = StreamableHttpClientTransportConfig::with_uri(http.url.clone())
                    .custom_headers(header_map);
                debug!("Opening MCP HTTP session: {}", http.url);
                let transport = StreamableHttpClientTransport::from_config(config);
                ().serve(transport)
                    .await
                    .context("MCP streamable-http handshake failed")
            }
        }
    }

    pub async fn discover(
        &self,
        env: Option<HashMap<String, String>>,
        headers: Option<HashMap<String, String>>,
    ) -> Result<()> {
        let client = self.connect(env, headers).await?;
        let actions = fetch_actions(&client).await?;
        let resources = fetch_resources(&client).await?;
        let prompts = fetch_prompts(&client).await?;
        let _ = client.cancel().await;
        info!(
            "MCP discovery complete: {} tools, {} resources, {} prompts",
            actions.len(),
            resources.len(),
            prompts.len()
        );
        *self.cached_actions.write().await = Some(actions);
        *self.cached_resources.write().await = Some(resources);
        *self.cached_prompts.write().await = Some(prompts);
        Ok(())
    }

    pub async fn get_action_definitions(
        &self,
        env: Option<HashMap<String, String>>,
        headers: Option<HashMap<String, String>>,
    ) -> Result<Vec<ActionDefinition>> {
        if env.is_none() && headers.is_none() {
            return Ok(self.cached_actions.read().await.clone().unwrap_or_default());
        }
        match self.connect(env, headers).await {
            Ok(client) => {
                let actions = fetch_actions(&client).await?;
                let _ = client.cancel().await;
                *self.cached_actions.write().await = Some(actions.clone());
                Ok(actions)
            }
            Err(err) => {
                if let Some(cached) = self.cached_actions.read().await.clone() {
                    warn!(
                        "Live MCP fetch failed, returning {} cached actions: {}",
                        cached.len(),
                        err
                    );
                    Ok(cached)
                } else {
                    Err(err)
                }
            }
        }
    }

    pub async fn get_resource_definitions(
        &self,
        env: Option<HashMap<String, String>>,
        headers: Option<HashMap<String, String>>,
    ) -> Result<Vec<McpResourceDefinition>> {
        if env.is_none() && headers.is_none() {
            return Ok(self
                .cached_resources
                .read()
                .await
                .clone()
                .unwrap_or_default());
        }
        match self.connect(env, headers).await {
            Ok(client) => {
                let resources = fetch_resources(&client).await?;
                let _ = client.cancel().await;
                *self.cached_resources.write().await = Some(resources.clone());
                Ok(resources)
            }
            Err(err) => {
                if let Some(cached) = self.cached_resources.read().await.clone() {
                    Ok(cached)
                } else {
                    Err(err)
                }
            }
        }
    }

    pub async fn get_prompt_definitions(
        &self,
        env: Option<HashMap<String, String>>,
        headers: Option<HashMap<String, String>>,
    ) -> Result<Vec<McpPromptDefinition>> {
        if env.is_none() && headers.is_none() {
            return Ok(self.cached_prompts.read().await.clone().unwrap_or_default());
        }
        match self.connect(env, headers).await {
            Ok(client) => {
                let prompts = fetch_prompts(&client).await?;
                let _ = client.cancel().await;
                *self.cached_prompts.write().await = Some(prompts.clone());
                Ok(prompts)
            }
            Err(err) => {
                if let Some(cached) = self.cached_prompts.read().await.clone() {
                    Ok(cached)
                } else {
                    Err(err)
                }
            }
        }
    }

    pub async fn execute_tool(
        &self,
        name: &str,
        arguments: JsonValue,
        env: Option<HashMap<String, String>>,
        headers: Option<HashMap<String, String>>,
    ) -> ActionResponse {
        let client = match self.connect(env, headers).await {
            Ok(c) => c,
            Err(e) => return ActionResponse::failure(e.to_string()),
        };
        let args_obj = match arguments {
            JsonValue::Object(map) => Some(map),
            JsonValue::Null => None,
            other => {
                let _ = client.cancel().await;
                return ActionResponse::failure(format!(
                    "tool arguments must be an object, got {}",
                    type_of(&other)
                ));
            }
        };
        let mut params = CallToolRequestParams::new(name.to_string());
        if let Some(args) = args_obj {
            params = params.with_arguments(args);
        }
        let result = client.call_tool(params).await;
        let _ = client.cancel().await;
        match result {
            Ok(call_result) => {
                let mut text_parts = Vec::new();
                for block in &call_result.content {
                    match &block.raw {
                        RawContent::Text(t) => text_parts.push(t.text.clone()),
                        RawContent::Image(img) => {
                            text_parts.push(format!("[binary: {}]", img.mime_type));
                        }
                        _ => {}
                    }
                }
                let content = text_parts.join("\n");
                if call_result.is_error.unwrap_or(false) {
                    ActionResponse::failure(content)
                } else {
                    ActionResponse::success(serde_json::json!({ "content": content }))
                }
            }
            Err(e) => ActionResponse::failure(e.to_string()),
        }
    }

    pub async fn read_resource(
        &self,
        uri: &str,
        env: Option<HashMap<String, String>>,
        headers: Option<HashMap<String, String>>,
    ) -> Result<JsonValue> {
        let client = self.connect(env, headers).await?;
        let result = client
            .read_resource(ReadResourceRequestParams::new(uri.to_string()))
            .await;
        let _ = client.cancel().await;
        let result = result?;
        let mut items = Vec::new();
        for c in result.contents {
            let mut entry = serde_json::Map::new();
            match c {
                ResourceContents::TextResourceContents {
                    uri,
                    mime_type,
                    text,
                    ..
                } => {
                    entry.insert("uri".into(), JsonValue::String(uri));
                    entry.insert("text".into(), JsonValue::String(text));
                    if let Some(mt) = mime_type {
                        entry.insert("mime_type".into(), JsonValue::String(mt));
                    }
                }
                ResourceContents::BlobResourceContents {
                    uri,
                    mime_type,
                    blob,
                    ..
                } => {
                    entry.insert("uri".into(), JsonValue::String(uri));
                    entry.insert("blob".into(), JsonValue::String(blob));
                    if let Some(mt) = mime_type {
                        entry.insert("mime_type".into(), JsonValue::String(mt));
                    }
                }
            }
            items.push(JsonValue::Object(entry));
        }
        Ok(serde_json::json!({ "contents": items }))
    }

    pub async fn get_prompt(
        &self,
        name: &str,
        arguments: Option<JsonValue>,
        env: Option<HashMap<String, String>>,
        headers: Option<HashMap<String, String>>,
    ) -> Result<JsonValue> {
        let client = self.connect(env, headers).await?;
        let mut params = GetPromptRequestParams::new(name.to_string());
        if let Some(JsonValue::Object(map)) = arguments {
            params.arguments = Some(map);
        }
        let result = client.get_prompt(params).await;
        let _ = client.cancel().await;
        let result = result?;
        let mut messages = Vec::new();
        for msg in result.messages {
            let role = format!("{:?}", msg.role).to_lowercase();
            let content = match msg.content {
                PromptMessageContent::Text { text } => {
                    serde_json::json!({ "type": "text", "text": text })
                }
                PromptMessageContent::Image { image } => {
                    serde_json::json!({
                        "type": "image",
                        "mime_type": image.mime_type,
                    })
                }
                _ => serde_json::json!({ "type": "unknown" }),
            };
            messages.push(serde_json::json!({ "role": role, "content": content }));
        }
        Ok(serde_json::json!({
            "description": result.description,
            "messages": messages,
        }))
    }
}

fn type_of(v: &JsonValue) -> &'static str {
    match v {
        JsonValue::Null => "null",
        JsonValue::Bool(_) => "bool",
        JsonValue::Number(_) => "number",
        JsonValue::String(_) => "string",
        JsonValue::Array(_) => "array",
        JsonValue::Object(_) => "object",
    }
}

async fn fetch_actions(client: &RmcpClient) -> Result<Vec<ActionDefinition>> {
    let result = client
        .list_tools(Some(PaginatedRequestParams::default()))
        .await?;
    let mut actions = Vec::with_capacity(result.tools.len());
    for tool in result.tools {
        let read_only = tool
            .annotations
            .as_ref()
            .and_then(|a| a.read_only_hint)
            .unwrap_or(false);
        let input_schema: JsonValue = serde_json::to_value(tool.input_schema.as_ref())
            .unwrap_or_else(|_| serde_json::json!({ "type": "object", "properties": {} }));
        actions.push(ActionDefinition {
            name: tool.name.to_string(),
            description: tool.description.map(|c| c.to_string()).unwrap_or_default(),
            input_schema,
            mode: if read_only {
                ActionMode::Read
            } else {
                ActionMode::Write
            },
        });
    }
    Ok(actions)
}

async fn fetch_resources(client: &RmcpClient) -> Result<Vec<McpResourceDefinition>> {
    let mut definitions = Vec::new();

    let templates = client
        .list_resource_templates(Some(PaginatedRequestParams::default()))
        .await?;
    for tmpl in templates.resource_templates {
        definitions.push(McpResourceDefinition {
            uri_template: tmpl.uri_template.clone(),
            name: tmpl.raw.name.clone(),
            description: tmpl.raw.description.clone(),
            mime_type: tmpl.raw.mime_type.clone(),
        });
    }

    let resources = client
        .list_resources(Some(PaginatedRequestParams::default()))
        .await?;
    for res in resources.resources {
        definitions.push(McpResourceDefinition {
            uri_template: res.raw.uri.clone(),
            name: res.raw.name.clone(),
            description: res.raw.description.clone(),
            mime_type: res.raw.mime_type.clone(),
        });
    }

    Ok(definitions)
}

async fn fetch_prompts(client: &RmcpClient) -> Result<Vec<McpPromptDefinition>> {
    let result = client
        .list_prompts(Some(PaginatedRequestParams::default()))
        .await?;
    let mut definitions = Vec::with_capacity(result.prompts.len());
    for prompt in result.prompts {
        let arguments = prompt
            .arguments
            .unwrap_or_default()
            .into_iter()
            .map(|arg| McpPromptArgument {
                name: arg.name,
                description: arg.description,
                required: arg.required.unwrap_or(false),
            })
            .collect();
        definitions.push(McpPromptDefinition {
            name: prompt.name,
            description: prompt.description,
            arguments,
        });
    }
    Ok(definitions)
}

use crate::models::ConnectorManifest;
use crate::remote_mcp::oauth::{parse_oauth_config, usable_oauth_credential, OAuthError};
use futures::StreamExt;
use redis::AsyncCommands;
use reqwest::{Client, Url};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value as JsonValue};
use shared::db::repositories::ServiceCredentialsRepo;
use shared::models::{
    ActionDefinition, ActionMode, AuthType, IntegrationType, McpPromptArgument,
    McpPromptDefinition, McpResourceDefinition, ServiceCredential, ServiceProvider, Source,
};
use shared::{traits::Repository, DatabasePool};
use std::collections::HashMap;
use std::net::{IpAddr, SocketAddr};
use std::sync::{Arc, OnceLock};
use std::time::Duration;
use thiserror::Error;
use tokio::sync::Mutex;
use tracing::warn;

pub const REMOTE_MCP_CONNECTOR_ID_PREFIX: &str = "remote_mcp:";
pub const REMOTE_MCP_MANIFEST_TTL_SECONDS: u64 = 300;
const REMOTE_MCP_CONNECT_TIMEOUT_SECONDS: u64 = 5;
const REMOTE_MCP_READ_TIMEOUT_SECONDS: u64 = 20;
const REMOTE_MCP_OVERALL_TIMEOUT_SECONDS: u64 = 30;
const MAX_MCP_RESPONSE_BYTES: usize = 2 * 1024 * 1024;
const MAX_MCP_CATALOG_ITEMS: usize = 200;
const MAX_MCP_SCHEMA_BYTES: usize = 64 * 1024;
const MAX_MCP_STRING_BYTES: usize = 8 * 1024;

static REMOTE_MCP_OAUTH_REFRESH_LOCKS: OnceLock<Mutex<HashMap<String, Arc<Mutex<()>>>>> =
    OnceLock::new();

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RemoteMcpConfig {
    pub endpoint_url: String,
    #[serde(default)]
    pub auth_type: Option<AuthType>,
    #[serde(default = "default_write_tools_enabled")]
    pub write_tools_enabled: bool,
}

fn default_write_tools_enabled() -> bool {
    true
}

#[derive(Debug, Error)]
pub enum GatewayError {
    #[error("source not found: {0}")]
    SourceNotFound(String),
    #[error("source {0} is not a remote MCP source")]
    NotRemoteMcp(String),
    #[error("invalid remote MCP config for source {source_id}: {message}")]
    InvalidConfig { source_id: String, message: String },
    #[error("unsupported remote MCP auth type for source {source_id}: {auth_type:?}")]
    UnsupportedAuthType {
        source_id: String,
        auth_type: AuthType,
    },
    #[error("remote MCP endpoint resolves to a disallowed address: {0}")]
    DisallowedAddress(String),
    #[error("database error: {0}")]
    Database(#[from] shared::db::error::DatabaseError),
    #[error("redis error: {0}")]
    Redis(#[from] redis::RedisError),
    #[error("http error: {0}")]
    Http(#[from] reqwest::Error),
    #[error("serialization error: {0}")]
    Serde(#[from] serde_json::Error),
    #[error("remote MCP protocol error: {0}")]
    Protocol(String),
    #[error("remote MCP source {source_id} requires per-user OAuth credentials")]
    NeedsUserAuth {
        source_id: String,
        source_type: String,
        provider: ServiceProvider,
    },
    #[error("missing runtime credentials for remote MCP source {0}")]
    MissingCredentials(String),
    #[error("remote MCP source is inactive or deleted: {0}")]
    SourceInactive(String),
}

#[derive(Clone)]
pub struct RemoteMcpGateway {
    db_pool: DatabasePool,
    redis_client: redis::Client,
    http_client: Client,
}

impl RemoteMcpGateway {
    pub fn new(db_pool: DatabasePool, redis_client: redis::Client) -> Result<Self, GatewayError> {
        let http_client = remote_mcp_client_builder()
            .redirect(reqwest::redirect::Policy::none())
            .build()?;
        Ok(Self {
            db_pool,
            redis_client,
            http_client,
        })
    }

    pub(crate) fn redis_client_for_registry(&self) -> redis::Client {
        self.redis_client.clone()
    }

    pub async fn discover_and_register(
        &self,
        source_id: &str,
    ) -> Result<ConnectorManifest, GatewayError> {
        let source = self.load_source(source_id).await?;
        let config = parse_remote_mcp_config(&source)?;
        let manifest = self.discover_manifest(&source, &config).await?;
        self.publish_manifest(&manifest).await?;
        Ok(manifest)
    }

    pub async fn refresh_catalog(
        &self,
        source_id: &str,
    ) -> Result<ConnectorManifest, GatewayError> {
        self.discover_and_register(source_id).await
    }

    pub async fn execute_action(
        &self,
        source: &Source,
        action: &str,
        params: JsonValue,
        user_id: Option<&str>,
    ) -> Result<JsonValue, GatewayError> {
        ensure_active_remote_mcp_source(source)?;
        let config = parse_remote_mcp_config(source)?;
        let headers = self.runtime_auth_headers(source, &config, user_id).await?;
        let session_id = self
            .initialize_session(&config.endpoint_url, &headers)
            .await?;
        let response = self
            .json_rpc(
                &config.endpoint_url,
                session_id.as_deref(),
                &headers,
                json!({
                    "jsonrpc": "2.0",
                    "id": 10,
                    "method": "tools/call",
                    "params": {
                        "name": action,
                        "arguments": params,
                    },
                }),
            )
            .await?;
        Ok(response
            .body
            .get("result")
            .cloned()
            .unwrap_or(response.body))
    }

    pub async fn read_resource(
        &self,
        source: &Source,
        uri: &str,
        user_id: Option<&str>,
    ) -> Result<JsonValue, GatewayError> {
        ensure_active_remote_mcp_source(source)?;
        let config = parse_remote_mcp_config(source)?;
        let headers = self.runtime_auth_headers(source, &config, user_id).await?;
        let session_id = self
            .initialize_session(&config.endpoint_url, &headers)
            .await?;
        let response = self
            .json_rpc(
                &config.endpoint_url,
                session_id.as_deref(),
                &headers,
                json!({
                    "jsonrpc": "2.0",
                    "id": 11,
                    "method": "resources/read",
                    "params": { "uri": uri },
                }),
            )
            .await?;
        Ok(response
            .body
            .get("result")
            .cloned()
            .unwrap_or(response.body))
    }

    pub async fn get_prompt(
        &self,
        source: &Source,
        name: &str,
        arguments: Option<JsonValue>,
        user_id: Option<&str>,
    ) -> Result<JsonValue, GatewayError> {
        ensure_active_remote_mcp_source(source)?;
        let config = parse_remote_mcp_config(source)?;
        let headers = self.runtime_auth_headers(source, &config, user_id).await?;
        let session_id = self
            .initialize_session(&config.endpoint_url, &headers)
            .await?;
        let response = self
            .json_rpc(
                &config.endpoint_url,
                session_id.as_deref(),
                &headers,
                json!({
                    "jsonrpc": "2.0",
                    "id": 12,
                    "method": "prompts/get",
                    "params": {
                        "name": name,
                        "arguments": arguments.unwrap_or_else(|| json!({})),
                    },
                }),
            )
            .await?;
        Ok(response
            .body
            .get("result")
            .cloned()
            .unwrap_or(response.body))
    }

    pub async fn remove_manifest_for_source_type(
        &self,
        source_type: &str,
    ) -> Result<(), GatewayError> {
        let key = manifest_key(source_type);
        let mut conn = self.redis_client.get_multiplexed_async_connection().await?;
        let _: () = conn.del(key).await?;
        Ok(())
    }

    async fn load_source(&self, source_id: &str) -> Result<Source, GatewayError> {
        let repo = shared::db::repositories::SourceRepository::new(self.db_pool.pool());
        repo.find_by_id(source_id.to_string())
            .await?
            .ok_or_else(|| GatewayError::SourceNotFound(source_id.to_string()))
    }

    async fn discover_manifest(
        &self,
        source: &Source,
        config: &RemoteMcpConfig,
    ) -> Result<ConnectorManifest, GatewayError> {
        validate_endpoint_for_gateway(&config.endpoint_url).await?;
        let mut session_id = None;
        let headers = self.discovery_auth_headers(source, config).await?;

        let initialize = self
            .json_rpc(
                &config.endpoint_url,
                session_id.as_deref(),
                &headers,
                json!({
                    "jsonrpc": "2.0",
                    "id": 1,
                    "method": "initialize",
                    "params": {
                        "protocolVersion": "2024-11-05",
                        "capabilities": {},
                        "clientInfo": { "name": "omni-connector-manager", "version": env!("CARGO_PKG_VERSION") }
                    }
                }),
            )
            .await?;
        session_id = initialize.session_id;
        let server_info = initialize
            .body
            .pointer("/result/serverInfo")
            .cloned()
            .unwrap_or(json!({}));
        let version = server_info
            .get("version")
            .and_then(|v| v.as_str())
            .unwrap_or("unknown")
            .to_string();

        let _ = self
            .json_rpc(
                &config.endpoint_url,
                session_id.as_deref(),
                &headers,
                json!({"jsonrpc":"2.0","method":"notifications/initialized","params":{}}),
            )
            .await;

        let actions = self
            .discover_actions(
                &config.endpoint_url,
                session_id.as_deref(),
                &headers,
                config,
            )
            .await
            .unwrap_or_else(|e| {
                warn!(source_id = %source.id, error = %e, "remote MCP tools/list failed");
                Vec::new()
            });
        let resources = self
            .discover_resources(&config.endpoint_url, session_id.as_deref(), &headers)
            .await
            .unwrap_or_else(|e| {
                warn!(source_id = %source.id, error = %e, "remote MCP resources discovery failed");
                Vec::new()
            });
        let prompts = self
            .discover_prompts(&config.endpoint_url, session_id.as_deref(), &headers)
            .await
            .unwrap_or_else(|e| {
                warn!(source_id = %source.id, error = %e, "remote MCP prompts/list failed");
                Vec::new()
            });

        let oauth = if config.auth_type == Some(AuthType::OAuth) {
            self.discover_oauth_metadata(&config.endpoint_url, &source.source_type)
                .await
                .unwrap_or_else(|e| {
                    warn!(source_id = %source.id, error = %e, "remote MCP OAuth metadata discovery failed");
                    None
                })
        } else {
            None
        };

        Ok(build_manifest(
            source, config, version, actions, resources, prompts, oauth,
        ))
    }

    async fn publish_manifest(&self, manifest: &ConnectorManifest) -> Result<(), GatewayError> {
        let manifest_json = serde_json::to_string(manifest)?;
        let mut conn = self.redis_client.get_multiplexed_async_connection().await?;
        let _: () = conn
            .set_ex(
                manifest_key(&manifest.source_types[0]),
                manifest_json,
                REMOTE_MCP_MANIFEST_TTL_SECONDS,
            )
            .await?;
        Ok(())
    }

    async fn initialize_session(
        &self,
        endpoint_url: &str,
        headers: &[(String, String)],
    ) -> Result<Option<String>, GatewayError> {
        let initialize = self
            .json_rpc(
                endpoint_url,
                None,
                headers,
                json!({
                    "jsonrpc": "2.0",
                    "id": 1,
                    "method": "initialize",
                    "params": {
                        "protocolVersion": "2024-11-05",
                        "capabilities": {},
                        "clientInfo": { "name": "omni-connector-manager", "version": env!("CARGO_PKG_VERSION") }
                    }
                }),
            )
            .await?;
        let session_id = initialize.session_id;
        let _ = self
            .json_rpc(
                endpoint_url,
                session_id.as_deref(),
                headers,
                json!({"jsonrpc":"2.0","method":"notifications/initialized","params":{}}),
            )
            .await;
        Ok(session_id)
    }

    async fn discovery_auth_headers(
        &self,
        source: &Source,
        config: &RemoteMcpConfig,
    ) -> Result<Vec<(String, String)>, GatewayError> {
        match config.auth_type {
            None => Ok(Vec::new()),
            Some(AuthType::BearerToken) => {
                let repo =
                    ServiceCredentialsRepo::new(self.db_pool.pool().clone()).map_err(|e| {
                        GatewayError::Protocol(format!(
                            "failed to initialize credential repository: {e}"
                        ))
                    })?;
                let credential = repo.find_org_credential(&source.id).await.map_err(|e| {
                    GatewayError::Protocol(format!("failed to load bearer credential: {e}"))
                })?;
                let token = credential
                    .and_then(|c| {
                        c.credentials
                            .get("token")
                            .and_then(|v| v.as_str())
                            .map(str::to_owned)
                    })
                    .ok_or_else(|| {
                        GatewayError::Protocol("missing bearer token credential".to_string())
                    })?;
                Ok(vec![(
                    "authorization".to_string(),
                    format!("Bearer {token}"),
                )])
            }
            Some(AuthType::OAuth) => {
                let repo =
                    ServiceCredentialsRepo::new(self.db_pool.pool().clone()).map_err(|e| {
                        GatewayError::Protocol(format!(
                            "failed to initialize credential repository: {e}"
                        ))
                    })?;
                let credential = repo.find_org_credential(&source.id).await.map_err(|e| {
                    GatewayError::Protocol(format!(
                        "failed to load OAuth bootstrap credential: {e}"
                    ))
                })?;
                let credential = match credential {
                    Some(credential) => {
                        let oauth_value = self
                            .discover_oauth_metadata(&config.endpoint_url, &source.source_type)
                            .await?
                            .ok_or_else(|| {
                                GatewayError::Protocol(
                                    "missing OAuth metadata for bootstrap credential refresh"
                                        .to_string(),
                                )
                            })?;
                        let oauth = parse_oauth_config(&oauth_value).map_err(|e| {
                            GatewayError::Protocol(format!("invalid OAuth metadata: {e}"))
                        })?;
                        Some(
                            self.usable_oauth_credential_serialized(source, credential, &oauth)
                                .await?,
                        )
                    }
                    None => None,
                };
                let token = credential
                    .and_then(|c| {
                        c.credentials
                            .get("access_token")
                            .and_then(|v| v.as_str())
                            .map(str::to_owned)
                    })
                    .ok_or_else(|| {
                        GatewayError::Protocol("missing OAuth bootstrap access_token".to_string())
                    })?;
                Ok(vec![(
                    "authorization".to_string(),
                    format!("Bearer {token}"),
                )])
            }
            Some(auth_type) => Err(GatewayError::UnsupportedAuthType {
                source_id: source.id.clone(),
                auth_type,
            }),
        }
    }

    async fn runtime_auth_headers(
        &self,
        source: &Source,
        config: &RemoteMcpConfig,
        user_id: Option<&str>,
    ) -> Result<Vec<(String, String)>, GatewayError> {
        let repo = match config.auth_type {
            None => return Ok(Vec::new()),
            Some(AuthType::BearerToken | AuthType::OAuth) => {
                ServiceCredentialsRepo::new(self.db_pool.pool().clone()).map_err(|e| {
                    GatewayError::Protocol(format!(
                        "failed to initialize credential repository: {e}"
                    ))
                })?
            }
            Some(auth_type) => {
                return Err(GatewayError::UnsupportedAuthType {
                    source_id: source.id.clone(),
                    auth_type,
                });
            }
        };

        let org_credential = if config.auth_type == Some(AuthType::BearerToken) {
            repo.find_org_credential(&source.id).await.map_err(|e| {
                GatewayError::Protocol(format!("failed to load org credential: {e}"))
            })?
        } else {
            None
        };
        let mut user_credential = match (config.auth_type, user_id) {
            (Some(AuthType::OAuth), Some(uid)) => repo
                .find_user_credential(&source.id, uid)
                .await
                .map_err(|e| {
                    GatewayError::Protocol(format!("failed to load user credential: {e}"))
                })?,
            _ => None,
        };
        if config.auth_type == Some(AuthType::OAuth) {
            if let Some(credential) = user_credential.take() {
                let oauth_value = self
                    .discover_oauth_metadata(&config.endpoint_url, &source.source_type)
                    .await?
                    .ok_or_else(|| {
                        GatewayError::Protocol(
                            "missing OAuth metadata for credential refresh".to_string(),
                        )
                    })?;
                let oauth = parse_oauth_config(&oauth_value)
                    .map_err(|e| GatewayError::Protocol(format!("invalid OAuth metadata: {e}")))?;
                user_credential = Some(
                    self.usable_oauth_credential_serialized(source, credential, &oauth)
                        .await?,
                );
            }
        }
        runtime_auth_headers_from_credentials(
            config,
            &source.id,
            &source.source_type,
            user_id,
            org_credential.as_ref(),
            user_credential.as_ref(),
        )
    }

    async fn usable_oauth_credential_serialized(
        &self,
        source: &Source,
        credential: ServiceCredential,
        oauth: &crate::remote_mcp::oauth::RemoteMcpOAuthConfig,
    ) -> Result<ServiceCredential, GatewayError> {
        let key = format!(
            "{}:{}",
            source.id,
            credential.user_id.as_deref().unwrap_or("__org__")
        );
        let locks = REMOTE_MCP_OAUTH_REFRESH_LOCKS.get_or_init(|| Mutex::new(HashMap::new()));
        let lock = {
            let mut guard = locks.lock().await;
            guard
                .entry(key)
                .or_insert_with(|| Arc::new(Mutex::new(())))
                .clone()
        };
        let _guard = lock.lock().await;

        let repo = ServiceCredentialsRepo::new(self.db_pool.pool().clone()).map_err(|e| {
            GatewayError::Protocol(format!("failed to create credential repo: {e}"))
        })?;
        let credential = repo
            .find_by_id(&credential.id)
            .await
            .map_err(|e| GatewayError::Protocol(format!("failed to reload credential: {e}")))?
            .ok_or_else(|| GatewayError::MissingCredentials(source.id.clone()))?;

        usable_oauth_credential(&self.db_pool, &self.http_client, source, credential, oauth)
            .await
            .map_err(|e| oauth_error_to_gateway_error(source, e))
    }

    async fn discover_oauth_metadata(
        &self,
        endpoint_url: &str,
        source_type: &str,
    ) -> Result<Option<JsonValue>, GatewayError> {
        let endpoint = Url::parse(endpoint_url)
            .map_err(|e| GatewayError::Protocol(format!("invalid endpoint URL: {e}")))?;
        let mut candidates = Vec::new();
        if endpoint.path() != "/" {
            candidates.push(format!(
                "{}/.well-known/oauth-protected-resource{}",
                endpoint.origin().ascii_serialization(),
                endpoint.path()
            ));
        }
        candidates.push(format!(
            "{}/.well-known/oauth-protected-resource",
            endpoint.origin().ascii_serialization()
        ));

        for candidate in candidates {
            let response = pinned_http_client_for_url(&candidate)
                .await?
                .get(&candidate)
                .header("accept", "application/json")
                .send()
                .await
                .map_err(|e| GatewayError::Protocol(format!("OAuth metadata fetch failed: {e}")))?;
            if !response.status().is_success() {
                continue;
            }
            let prm_text = read_limited_response_text(response).await?;
            let prm: JsonValue = serde_json::from_str(&prm_text).map_err(|e| {
                GatewayError::Protocol(format!("OAuth metadata JSON parse failed: {e}"))
            })?;
            let auth_server = prm
                .get("authorization_servers")
                .and_then(|v| v.as_array())
                .and_then(|servers| servers.iter().find_map(|v| v.as_str()))
                .ok_or_else(|| {
                    GatewayError::Protocol(
                        "OAuth protected resource metadata has no authorization server".to_string(),
                    )
                })?;
            let auth_url = Url::parse(auth_server).map_err(|e| {
                GatewayError::Protocol(format!("invalid authorization server URL: {e}"))
            })?;
            let auth_metadata_url = format!(
                "{}/.well-known/oauth-authorization-server{}",
                auth_url.origin().ascii_serialization(),
                if auth_url.path() == "/" {
                    ""
                } else {
                    auth_url.path()
                }
            );
            let as_response = pinned_http_client_for_url(&auth_metadata_url)
                .await?
                .get(&auth_metadata_url)
                .header("accept", "application/json")
                .send()
                .await
                .map_err(|e| {
                    GatewayError::Protocol(format!("authorization metadata fetch failed: {e}"))
                })?;
            if !as_response.status().is_success() {
                continue;
            }
            let as_text = read_limited_response_text(as_response).await?;
            let as_metadata: JsonValue = serde_json::from_str(&as_text).map_err(|e| {
                GatewayError::Protocol(format!("authorization metadata JSON parse failed: {e}"))
            })?;
            let auth_endpoint = as_metadata
                .get("authorization_endpoint")
                .and_then(|v| v.as_str())
                .ok_or_else(|| {
                    GatewayError::Protocol("missing authorization_endpoint".to_string())
                })?;
            validate_endpoint_for_gateway(auth_endpoint).await?;
            let token_endpoint = as_metadata
                .get("token_endpoint")
                .and_then(|v| v.as_str())
                .ok_or_else(|| GatewayError::Protocol("missing token_endpoint".to_string()))?;
            validate_endpoint_for_gateway(token_endpoint).await?;
            let userinfo_endpoint = match as_metadata
                .get("userinfo_endpoint")
                .and_then(|v| v.as_str())
            {
                Some(value) => {
                    validate_endpoint_for_gateway(value).await?;
                    value.to_string()
                }
                None => endpoint.origin().ascii_serialization(),
            };
            let registration_endpoint = match as_metadata
                .get("registration_endpoint")
                .and_then(|v| v.as_str())
            {
                Some(value) => {
                    validate_endpoint_for_gateway(value).await?;
                    Some(value.to_string())
                }
                None => None,
            };
            let resource = match prm.get("resource").and_then(|v| v.as_str()) {
                Some(value) => {
                    validate_endpoint_for_gateway(value).await?;
                    value.to_string()
                }
                None => endpoint_url.to_string(),
            };
            let scopes = as_metadata
                .get("scopes_supported")
                .and_then(|v| v.as_array())
                .map(|values| values.iter().filter_map(|v| v.as_str()).collect::<Vec<_>>())
                .unwrap_or_default();
            let token_auth_method = as_metadata
                .get("token_endpoint_auth_methods_supported")
                .and_then(|v| v.as_array())
                .and_then(|values| {
                    values
                        .iter()
                        .find_map(|v| (v.as_str() == Some("none")).then_some("none"))
                })
                .unwrap_or("client_secret_post");
            return Ok(Some(json!({
                "provider": format!("{REMOTE_MCP_CONNECTOR_ID_PREFIX}{source_type}"),
                "credential_provider": "remote_mcp",
                "auth_endpoint": auth_endpoint,
                "token_endpoint": token_endpoint,
                "userinfo_endpoint": userinfo_endpoint,
                "userinfo_email_field": "email",
                "identity_scopes": [],
                "scopes": { source_type: { "read": scopes, "write": scopes } },
                "extra_auth_params": {},
                "scope_separator": " ",
                "registration_endpoint": registration_endpoint,
                "token_endpoint_auth_method": token_auth_method,
                "resource": resource,
                "protected_resource_metadata_url": candidate,
                "authorization_server_metadata_url": auth_metadata_url,
            })));
        }
        Ok(None)
    }

    async fn json_rpc(
        &self,
        endpoint_url: &str,
        session_id: Option<&str>,
        headers: &[(String, String)],
        body: JsonValue,
    ) -> Result<JsonRpcResponse, GatewayError> {
        let http_client = pinned_http_client_for_url(endpoint_url).await?;
        let mut request = http_client
            .post(endpoint_url)
            .header("accept", "application/json, text/event-stream")
            .header("content-type", "application/json");
        if let Some(session_id) = session_id {
            request = request.header("mcp-session-id", session_id);
        }
        for (name, value) in headers {
            request = request.header(name, value);
        }
        let response = request.json(&body).send().await?;
        let next_session_id = response
            .headers()
            .get("mcp-session-id")
            .and_then(|v| v.to_str().ok())
            .map(str::to_owned)
            .or_else(|| session_id.map(str::to_owned));
        if !response.status().is_success() {
            return Err(GatewayError::Protocol(format!(
                "MCP HTTP request failed with status {}",
                response.status()
            )));
        }
        let content_type = response
            .headers()
            .get("content-type")
            .and_then(|v| v.to_str().ok())
            .unwrap_or_default()
            .to_string();
        let text = read_limited_response_text(response).await?;
        let body = parse_mcp_response_body(&text, &content_type)?;
        if let Some(error) = body.get("error") {
            return Err(GatewayError::Protocol(format!(
                "MCP JSON-RPC error: {error}"
            )));
        }
        Ok(JsonRpcResponse {
            body,
            session_id: next_session_id,
        })
    }

    async fn discover_actions(
        &self,
        endpoint_url: &str,
        session_id: Option<&str>,
        headers: &[(String, String)],
        config: &RemoteMcpConfig,
    ) -> Result<Vec<ActionDefinition>, GatewayError> {
        let response = self
            .json_rpc(
                endpoint_url,
                session_id,
                headers,
                json!({"jsonrpc":"2.0","id":2,"method":"tools/list","params":{}}),
            )
            .await?;
        let tools = response
            .body
            .pointer("/result/tools")
            .and_then(|v| v.as_array());
        Ok(tools
            .into_iter()
            .flatten()
            .take(MAX_MCP_CATALOG_ITEMS)
            .filter_map(|tool| action_from_tool(tool, config.write_tools_enabled))
            .collect())
    }

    async fn discover_resources(
        &self,
        endpoint_url: &str,
        session_id: Option<&str>,
        headers: &[(String, String)],
    ) -> Result<Vec<McpResourceDefinition>, GatewayError> {
        let mut resources = Vec::new();
        let listed = self
            .json_rpc(
                endpoint_url,
                session_id,
                headers,
                json!({"jsonrpc":"2.0","id":3,"method":"resources/list","params":{}}),
            )
            .await?;
        if let Some(items) = listed
            .body
            .pointer("/result/resources")
            .and_then(|v| v.as_array())
        {
            resources.extend(
                items
                    .iter()
                    .take(MAX_MCP_CATALOG_ITEMS.saturating_sub(resources.len()))
                    .filter_map(resource_from_value),
            );
        }
        if let Ok(templates) = self
            .json_rpc(
                endpoint_url,
                session_id,
                headers,
                json!({"jsonrpc":"2.0","id":4,"method":"resources/templates/list","params":{}}),
            )
            .await
        {
            if let Some(items) = templates
                .body
                .pointer("/result/resourceTemplates")
                .and_then(|v| v.as_array())
            {
                resources.extend(
                    items
                        .iter()
                        .take(MAX_MCP_CATALOG_ITEMS.saturating_sub(resources.len()))
                        .filter_map(resource_from_value),
                );
            }
        }
        Ok(resources)
    }

    async fn discover_prompts(
        &self,
        endpoint_url: &str,
        session_id: Option<&str>,
        headers: &[(String, String)],
    ) -> Result<Vec<McpPromptDefinition>, GatewayError> {
        let response = self
            .json_rpc(
                endpoint_url,
                session_id,
                headers,
                json!({"jsonrpc":"2.0","id":5,"method":"prompts/list","params":{}}),
            )
            .await?;
        let prompts = response
            .body
            .pointer("/result/prompts")
            .and_then(|v| v.as_array());
        Ok(prompts
            .into_iter()
            .flatten()
            .take(MAX_MCP_CATALOG_ITEMS)
            .filter_map(prompt_from_value)
            .collect())
    }
}

struct JsonRpcResponse {
    body: JsonValue,
    session_id: Option<String>,
}

pub fn parse_remote_mcp_config(source: &Source) -> Result<RemoteMcpConfig, GatewayError> {
    if source.integration_type != IntegrationType::RemoteMcp {
        return Err(GatewayError::NotRemoteMcp(source.id.clone()));
    }
    let config: RemoteMcpConfig =
        serde_json::from_value(source.config.clone()).map_err(|e| GatewayError::InvalidConfig {
            source_id: source.id.clone(),
            message: e.to_string(),
        })?;
    Url::parse(&config.endpoint_url)
        .map_err(|e| GatewayError::InvalidConfig {
            source_id: source.id.clone(),
            message: e.to_string(),
        })
        .and_then(|url| {
            if url.scheme() != "https" && url.scheme() != "http" {
                Err(GatewayError::InvalidConfig {
                    source_id: source.id.clone(),
                    message: "endpoint_url must use http or https".to_string(),
                })
            } else if url.username() != "" || url.password().is_some() || url.fragment().is_some() {
                Err(GatewayError::InvalidConfig {
                    source_id: source.id.clone(),
                    message: "endpoint_url must not include credentials or fragments".to_string(),
                })
            } else {
                Ok(())
            }
        })?;
    if let Some(auth_type) = config.auth_type {
        if !matches!(auth_type, AuthType::BearerToken | AuthType::OAuth) {
            return Err(GatewayError::UnsupportedAuthType {
                source_id: source.id.clone(),
                auth_type,
            });
        }
    }
    Ok(config)
}

pub fn manifest_key(source_type: &str) -> String {
    format!("connector:manifest:{REMOTE_MCP_CONNECTOR_ID_PREFIX}{source_type}")
}

fn authorization_header(token: &str) -> Vec<(String, String)> {
    vec![("authorization".to_string(), format!("Bearer {token}"))]
}

pub fn runtime_auth_headers_from_credentials(
    config: &RemoteMcpConfig,
    source_id: &str,
    source_type: &str,
    user_id: Option<&str>,
    org_credential: Option<&ServiceCredential>,
    user_credential: Option<&ServiceCredential>,
) -> Result<Vec<(String, String)>, GatewayError> {
    match config.auth_type {
        None => Ok(Vec::new()),
        Some(AuthType::BearerToken) => {
            let token = org_credential
                .and_then(|credential| credential.credentials.get("token"))
                .and_then(|value| value.as_str())
                .ok_or_else(|| GatewayError::MissingCredentials(source_id.to_string()))?;
            Ok(authorization_header(token))
        }
        Some(AuthType::OAuth) => {
            let Some(_uid) = user_id else {
                return Err(GatewayError::MissingCredentials(source_id.to_string()));
            };
            let token = user_credential
                .and_then(|credential| credential.credentials.get("access_token"))
                .and_then(|value| value.as_str())
                .ok_or_else(|| GatewayError::NeedsUserAuth {
                    source_id: source_id.to_string(),
                    source_type: source_type.to_string(),
                    provider: ServiceProvider::RemoteMcp,
                })?;
            Ok(authorization_header(token))
        }
        Some(auth_type) => Err(GatewayError::UnsupportedAuthType {
            source_id: source_id.to_string(),
            auth_type,
        }),
    }
}

pub fn build_manifest(
    source: &Source,
    config: &RemoteMcpConfig,
    version: String,
    actions: Vec<ActionDefinition>,
    resources: Vec<McpResourceDefinition>,
    prompts: Vec<McpPromptDefinition>,
    oauth: Option<JsonValue>,
) -> ConnectorManifest {
    ConnectorManifest {
        name: source.source_type.clone(),
        display_name: source.name.clone(),
        version,
        sync_modes: Vec::new(),
        connector_id: format!("{REMOTE_MCP_CONNECTOR_ID_PREFIX}{}", source.source_type),
        connector_url: String::new(),
        integration_type: IntegrationType::RemoteMcp,
        source_types: vec![source.source_type.clone()],
        description: None,
        actions,
        search_operators: Vec::new(),
        read_only: !config.write_tools_enabled,
        extra_schema: None,
        attributes_schema: None,
        mcp_enabled: true,
        mcp_catalog_loaded: true,
        resources,
        prompts,
        skills: Vec::new(),
        oauth: oauth.or_else(|| {
            config.auth_type.filter(|a| *a == AuthType::OAuth).map(|_| {
                json!({
                    "provider": format!("{REMOTE_MCP_CONNECTOR_ID_PREFIX}{}", source.source_type),
                    "credential_provider": "remote_mcp",
                })
            })
        }),
    }
}

fn action_from_tool(tool: &JsonValue, write_tools_enabled: bool) -> Option<ActionDefinition> {
    if serde_json::to_vec(tool).ok()?.len() > MAX_MCP_SCHEMA_BYTES {
        return None;
    }
    let name = bounded_string(tool.get("name")?.as_str()?)?;
    let read_only = tool
        .pointer("/annotations/readOnlyHint")
        .or_else(|| tool.pointer("/annotations/read_only_hint"))
        .and_then(|v| v.as_bool())
        .unwrap_or(false);
    let mode = if read_only {
        ActionMode::Read
    } else {
        ActionMode::Write
    };
    if mode == ActionMode::Write && !write_tools_enabled {
        return None;
    }
    Some(ActionDefinition {
        name,
        description: bounded_string(
            tool.get("description")
                .and_then(|v| v.as_str())
                .unwrap_or_default(),
        )
        .unwrap_or_default(),
        input_schema: bounded_schema(
            tool.get("inputSchema")
                .or_else(|| tool.get("input_schema"))
                .cloned()
                .unwrap_or_else(|| json!({"type":"object","properties":{}})),
        )?,
        mode,
        required_scopes: None,
        source_types: Vec::new(),
        admin_only: false,
        hidden: false,
    })
}

fn resource_from_value(value: &JsonValue) -> Option<McpResourceDefinition> {
    if serde_json::to_vec(value).ok()?.len() > MAX_MCP_SCHEMA_BYTES {
        return None;
    }
    let uri_template = bounded_string(
        value
            .get("uriTemplate")
            .or_else(|| value.get("uri_template"))
            .or_else(|| value.get("uri"))?
            .as_str()?,
    )?;
    Some(McpResourceDefinition {
        uri_template,
        name: bounded_string(value.get("name")?.as_str()?)?,
        description: value
            .get("description")
            .and_then(|v| v.as_str())
            .and_then(bounded_string),
        mime_type: value
            .get("mimeType")
            .or_else(|| value.get("mime_type"))
            .and_then(|v| v.as_str())
            .and_then(bounded_string),
    })
}

fn prompt_from_value(value: &JsonValue) -> Option<McpPromptDefinition> {
    if serde_json::to_vec(value).ok()?.len() > MAX_MCP_SCHEMA_BYTES {
        return None;
    }
    let arguments = value
        .get("arguments")
        .and_then(|v| v.as_array())
        .map(|args| {
            args.iter()
                .take(MAX_MCP_CATALOG_ITEMS)
                .filter_map(|arg| {
                    Some(McpPromptArgument {
                        name: bounded_string(arg.get("name")?.as_str()?)?,
                        description: arg
                            .get("description")
                            .and_then(|v| v.as_str())
                            .and_then(bounded_string),
                        required: arg
                            .get("required")
                            .and_then(|v| v.as_bool())
                            .unwrap_or(false),
                    })
                })
                .collect()
        })
        .unwrap_or_default();
    Some(McpPromptDefinition {
        name: bounded_string(value.get("name")?.as_str()?)?,
        description: value
            .get("description")
            .and_then(|v| v.as_str())
            .and_then(bounded_string),
        arguments,
    })
}

fn bounded_string(value: &str) -> Option<String> {
    (value.len() <= MAX_MCP_STRING_BYTES).then(|| value.to_string())
}

fn bounded_schema(value: JsonValue) -> Option<JsonValue> {
    (serde_json::to_vec(&value).ok()?.len() <= MAX_MCP_SCHEMA_BYTES).then_some(value)
}

pub(crate) async fn read_limited_response_text(
    response: reqwest::Response,
) -> Result<String, GatewayError> {
    if let Some(length) = response.content_length() {
        if length > MAX_MCP_RESPONSE_BYTES as u64 {
            return Err(GatewayError::Protocol(format!(
                "MCP response exceeded {} bytes",
                MAX_MCP_RESPONSE_BYTES
            )));
        }
    }
    let mut body = Vec::new();
    let mut stream = response.bytes_stream();
    while let Some(chunk) = stream.next().await {
        let chunk = chunk?;
        if body.len().saturating_add(chunk.len()) > MAX_MCP_RESPONSE_BYTES {
            return Err(GatewayError::Protocol(format!(
                "MCP response exceeded {} bytes",
                MAX_MCP_RESPONSE_BYTES
            )));
        }
        body.extend_from_slice(&chunk);
    }
    String::from_utf8(body)
        .map_err(|e| GatewayError::Protocol(format!("MCP response was not UTF-8: {e}")))
}

fn oauth_error_to_gateway_error(source: &Source, error: OAuthError) -> GatewayError {
    match error {
        OAuthError::ReconnectRequired => GatewayError::NeedsUserAuth {
            source_id: source.id.clone(),
            source_type: source.source_type.clone(),
            provider: ServiceProvider::RemoteMcp,
        },
        other => GatewayError::Protocol(format!("OAuth refresh failed: {other}")),
    }
}

fn parse_mcp_response_body(text: &str, content_type: &str) -> Result<JsonValue, GatewayError> {
    if content_type.contains("text/event-stream") {
        let event_data = text
            .lines()
            .filter_map(|line| line.strip_prefix("data:"))
            .map(str::trim)
            .find(|line| !line.is_empty() && *line != "[DONE]")
            .ok_or_else(|| GatewayError::Protocol("empty MCP event stream".to_string()))?;
        Ok(serde_json::from_str(event_data)?)
    } else if text.trim().is_empty() {
        Ok(json!({}))
    } else {
        Ok(serde_json::from_str(text)?)
    }
}

pub(crate) async fn validate_endpoint_for_gateway(endpoint_url: &str) -> Result<(), GatewayError> {
    let _ = validated_remote_mcp_addrs(endpoint_url).await?;
    Ok(())
}

async fn validated_remote_mcp_addrs(
    endpoint_url: &str,
) -> Result<(Url, String, Vec<SocketAddr>), GatewayError> {
    let url = Url::parse(endpoint_url)
        .map_err(|e| GatewayError::Protocol(format!("invalid endpoint URL: {e}")))?;
    if url.scheme() != "https" && url.scheme() != "http" {
        return Err(GatewayError::Protocol(
            "endpoint URL must use http or https".to_string(),
        ));
    }
    if !url.username().is_empty() || url.password().is_some() {
        return Err(GatewayError::Protocol(
            "endpoint URL must not include credentials".to_string(),
        ));
    }
    if url.fragment().is_some() {
        return Err(GatewayError::Protocol(
            "endpoint URL must not include a fragment".to_string(),
        ));
    }
    let host = url
        .host_str()
        .ok_or_else(|| GatewayError::Protocol("endpoint URL has no host".to_string()))?
        .to_string();
    let addrs: Vec<SocketAddr> =
        tokio::net::lookup_host((host.as_str(), url.port_or_known_default().unwrap_or(443)))
            .await
            .map_err(|e| GatewayError::Protocol(format!("endpoint DNS lookup failed: {e}")))?
            .collect();
    if addrs.is_empty() {
        return Err(GatewayError::Protocol(
            "endpoint host did not resolve".to_string(),
        ));
    }
    for addr in &addrs {
        let ip = addr.ip();
        if is_disallowed_ip(ip) {
            return Err(GatewayError::DisallowedAddress(ip.to_string()));
        }
    }
    Ok((url, host, addrs))
}

fn remote_mcp_client_builder() -> reqwest::ClientBuilder {
    Client::builder()
        .connect_timeout(Duration::from_secs(REMOTE_MCP_CONNECT_TIMEOUT_SECONDS))
        .read_timeout(Duration::from_secs(REMOTE_MCP_READ_TIMEOUT_SECONDS))
        .timeout(Duration::from_secs(REMOTE_MCP_OVERALL_TIMEOUT_SECONDS))
}

pub(crate) async fn pinned_http_client_for_url(endpoint_url: &str) -> Result<Client, GatewayError> {
    let (_url, host, addrs) = validated_remote_mcp_addrs(endpoint_url).await?;
    remote_mcp_client_builder()
        .redirect(reqwest::redirect::Policy::none())
        .resolve_to_addrs(&host, &addrs)
        .build()
        .map_err(GatewayError::Http)
}

fn ensure_active_remote_mcp_source(source: &Source) -> Result<(), GatewayError> {
    if source.integration_type != IntegrationType::RemoteMcp {
        return Err(GatewayError::NotRemoteMcp(source.id.clone()));
    }
    if !source.is_active || source.is_deleted {
        return Err(GatewayError::SourceInactive(source.id.clone()));
    }
    Ok(())
}

fn is_disallowed_ip(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(ip) => is_disallowed_ipv4(ip),
        IpAddr::V6(ip) => {
            if let Some(mapped) = ip.to_ipv4_mapped() {
                return is_disallowed_ipv4(mapped);
            }
            ip.is_loopback()
                || ip.is_multicast()
                || ip.is_unspecified()
                || ((ip.segments()[0] & 0xfe00) == 0xfc00)
                || ((ip.segments()[0] & 0xffc0) == 0xfe80)
                || ip.segments()[0] == 0x2002
                || (ip.segments()[0] == 0x2001 && ip.segments()[1] == 0x0db8)
        }
    }
}

fn is_disallowed_ipv4(ip: std::net::Ipv4Addr) -> bool {
    ip.is_private()
        || ip.is_loopback()
        || ip.is_link_local()
        || ip.is_multicast()
        || ip.is_broadcast()
        || ip.is_documentation()
        || ip.is_unspecified()
        || ip.octets()[0] == 0
        || ip.octets()[0] >= 224
        || (ip.octets()[0] == 100 && (64..=127).contains(&ip.octets()[1]))
        || (ip.octets()[0] == 192 && ip.octets()[1] == 0 && ip.octets()[2] == 0)
        || (ip.octets()[0] == 192 && ip.octets()[1] == 88 && ip.octets()[2] == 99)
        || (ip.octets()[0] == 198 && (18..=19).contains(&ip.octets()[1]))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use shared::models::{SourceScope, UserFilterMode};
    use time::OffsetDateTime;

    fn credential(
        user_id: Option<&str>,
        auth_type: AuthType,
        credentials: JsonValue,
    ) -> ServiceCredential {
        ServiceCredential {
            id: format!("cred_{}", user_id.unwrap_or("org")),
            source_id: "src_1".to_string(),
            user_id: user_id.map(str::to_owned),
            provider: ServiceProvider::RemoteMcp,
            auth_type,
            principal_email: None,
            credentials,
            config: json!({}),
            expires_at: None,
            last_validated_at: None,
            created_at: OffsetDateTime::now_utc(),
            updated_at: OffsetDateTime::now_utc(),
        }
    }

    fn source(config: JsonValue) -> Source {
        Source {
            id: "src_1".to_string(),
            name: "Acme MCP".to_string(),
            source_type: "acme".to_string(),
            integration_type: IntegrationType::RemoteMcp,
            config,
            is_active: true,
            is_deleted: false,
            scope: SourceScope::Org,
            user_filter_mode: UserFilterMode::All,
            user_whitelist: None,
            user_blacklist: None,
            connector_state: None,
            checkpoint: None,
            sync_interval_seconds: None,
            created_at: OffsetDateTime::now_utc(),
            updated_at: OffsetDateTime::now_utc(),
            created_by: "admin".to_string(),
        }
    }

    #[test]
    fn oversized_catalog_entries_are_ignored() {
        let oversized = "x".repeat(MAX_MCP_SCHEMA_BYTES + 1);
        assert!(action_from_tool(
            &json!({"name":"huge","inputSchema":{"description": oversized}}),
            true
        )
        .is_none());
        assert!(resource_from_value(
            &json!({"name":"huge","uri":"file://huge","description": oversized})
        )
        .is_none());
        assert!(prompt_from_value(&json!({"name":"huge","description": oversized})).is_none());
    }

    #[test]
    fn oauth_reconnect_maps_to_needs_user_auth() {
        let source = source(json!({"endpoint_url":"https://mcp.example.com/mcp"}));
        let err = oauth_error_to_gateway_error(&source, OAuthError::ReconnectRequired);
        assert!(matches!(
            err,
            GatewayError::NeedsUserAuth { source_id, source_type, provider }
                if source_id == "src_1" && source_type == "acme" && provider == ServiceProvider::RemoteMcp
        ));
    }

    #[test]
    fn runtime_dispatch_rejects_inactive_or_deleted_remote_mcp_sources() {
        let mut inactive = source(json!({"endpoint_url":"https://mcp.example.com/mcp"}));
        inactive.is_active = false;
        assert!(matches!(
            ensure_active_remote_mcp_source(&inactive),
            Err(GatewayError::SourceInactive(id)) if id == "src_1"
        ));

        let mut deleted = source(json!({"endpoint_url":"https://mcp.example.com/mcp"}));
        deleted.is_deleted = true;
        assert!(matches!(
            ensure_active_remote_mcp_source(&deleted),
            Err(GatewayError::SourceInactive(id)) if id == "src_1"
        ));
    }

    #[test]
    fn parses_remote_mcp_config_and_rejects_unsupported_auth() {
        let valid = parse_remote_mcp_config(&source(json!({
            "endpoint_url": "https://mcp.example.com/mcp",
            "auth_type": "bearer_token",
            "write_tools_enabled": false
        })))
        .unwrap();
        assert_eq!(valid.auth_type, Some(AuthType::BearerToken));
        assert!(!valid.write_tools_enabled);

        let err = parse_remote_mcp_config(&source(json!({
            "endpoint_url": "https://mcp.example.com/mcp",
            "auth_type": "api_key"
        })))
        .unwrap_err();
        assert!(matches!(err, GatewayError::UnsupportedAuthType { .. }));
    }

    #[test]
    fn manifest_uses_normal_source_type_identity_and_no_sync_modes() {
        let cfg = RemoteMcpConfig {
            endpoint_url: "https://mcp.example.com/mcp".to_string(),
            auth_type: None,
            write_tools_enabled: true,
        };
        let manifest = build_manifest(
            &source(json!({})),
            &cfg,
            "1.2.3".to_string(),
            vec![],
            vec![],
            vec![],
            None,
        );
        assert_eq!(manifest.connector_id, "remote_mcp:acme");
        assert_eq!(manifest.integration_type, IntegrationType::RemoteMcp);
        assert_eq!(manifest.source_types, vec!["acme".to_string()]);
        assert!(manifest.sync_modes.is_empty());
        assert!(manifest.connector_url.is_empty());
    }

    #[test]
    fn manifest_preserves_normalized_remote_mcp_oauth_config() {
        let cfg = RemoteMcpConfig {
            endpoint_url: "https://mcp.example.com/mcp".to_string(),
            auth_type: Some(AuthType::OAuth),
            write_tools_enabled: true,
        };
        let oauth = json!({
            "provider": "remote_mcp:acme",
            "credential_provider": "remote_mcp",
            "auth_endpoint": "https://auth.example.com/authorize",
            "token_endpoint": "https://auth.example.com/token",
            "resource": "https://mcp.example.com/mcp"
        });
        let manifest = build_manifest(
            &source(json!({})),
            &cfg,
            "1.2.3".to_string(),
            vec![],
            vec![],
            vec![],
            Some(oauth),
        );
        assert_eq!(
            manifest
                .oauth
                .as_ref()
                .and_then(|v| v.get("provider"))
                .and_then(|v| v.as_str()),
            Some("remote_mcp:acme")
        );
        assert_eq!(
            manifest
                .oauth
                .as_ref()
                .and_then(|v| v.get("credential_provider"))
                .and_then(|v| v.as_str()),
            Some("remote_mcp")
        );
    }

    #[test]
    fn write_tools_can_be_omitted_from_manifest_actions() {
        let read = json!({"name":"read","description":"r","annotations":{"readOnlyHint":true}});
        let write = json!({"name":"write","description":"w","annotations":{"readOnlyHint":false}});
        assert!(action_from_tool(&read, false).is_some());
        assert!(action_from_tool(&write, false).is_none());
    }

    #[test]
    fn runtime_auth_allows_public_without_credentials() {
        let cfg = RemoteMcpConfig {
            endpoint_url: "https://mcp.example.com/mcp".to_string(),
            auth_type: None,
            write_tools_enabled: true,
        };
        let headers = runtime_auth_headers_from_credentials(
            &cfg,
            "src_1",
            "acme",
            Some("user_1"),
            None,
            None,
        )
        .unwrap();
        assert!(headers.is_empty());
    }

    #[test]
    fn runtime_auth_uses_org_bearer_token_for_shared_bearer_sources() {
        let cfg = RemoteMcpConfig {
            endpoint_url: "https://mcp.example.com/mcp".to_string(),
            auth_type: Some(AuthType::BearerToken),
            write_tools_enabled: true,
        };
        let org = credential(None, AuthType::BearerToken, json!({"token":"org-token"}));
        let headers = runtime_auth_headers_from_credentials(
            &cfg,
            "src_1",
            "acme",
            Some("user_1"),
            Some(&org),
            None,
        )
        .unwrap();
        assert_eq!(
            headers,
            vec![("authorization".to_string(), "Bearer org-token".to_string())]
        );
    }

    #[test]
    fn runtime_oauth_requires_exact_user_credential_without_org_fallback() {
        let cfg = RemoteMcpConfig {
            endpoint_url: "https://mcp.example.com/mcp".to_string(),
            auth_type: Some(AuthType::OAuth),
            write_tools_enabled: true,
        };
        let org = credential(
            None,
            AuthType::OAuth,
            json!({"access_token":"bootstrap-token"}),
        );
        let err = runtime_auth_headers_from_credentials(
            &cfg,
            "src_1",
            "acme",
            Some("user_1"),
            Some(&org),
            None,
        )
        .unwrap_err();
        assert!(matches!(err, GatewayError::NeedsUserAuth { .. }));

        let user = credential(
            Some("user_1"),
            AuthType::OAuth,
            json!({"access_token":"user-token"}),
        );
        let headers = runtime_auth_headers_from_credentials(
            &cfg,
            "src_1",
            "acme",
            Some("user_1"),
            Some(&org),
            Some(&user),
        )
        .unwrap();
        assert_eq!(
            headers,
            vec![("authorization".to_string(), "Bearer user-token".to_string())]
        );
    }

    #[test]
    fn parses_sse_json_rpc_response() {
        let parsed = parse_mcp_response_body(
            "event: message\ndata: {\"jsonrpc\":\"2.0\",\"result\":{}}\n\n",
            "text/event-stream",
        )
        .unwrap();
        assert!(parsed.get("result").is_some());
    }

    #[test]
    fn gateway_ssrf_filter_blocks_reserved_ipv6_and_mapped_ipv4_ranges() {
        for ip in [
            "::1",
            "fc00::1",
            "fe80::1",
            "2001:db8::1",
            "2002:c000:0201::1",
            "::ffff:127.0.0.1",
            "::ffff:169.254.169.254",
        ] {
            assert!(
                is_disallowed_ip(ip.parse().unwrap()),
                "expected {ip} to be blocked"
            );
        }
        assert!(!is_disallowed_ip("2001:4860:4860::8888".parse().unwrap()));
    }

    #[test]
    fn gateway_ssrf_filter_blocks_additional_reserved_ipv4_ranges() {
        for ip in [
            "0.0.0.0",
            "100.64.0.1",
            "192.0.0.8",
            "192.88.99.1",
            "198.18.0.1",
            "203.0.113.10",
        ] {
            assert!(
                is_disallowed_ip(ip.parse().unwrap()),
                "expected {ip} to be blocked"
            );
        }
        assert!(!is_disallowed_ip("8.8.8.8".parse().unwrap()));
    }

    #[test]
    fn resource_and_prompt_catalog_entries_accept_mcp_wire_naming() {
        let resource = resource_from_value(&json!({
            "uriTemplate": "repo://files/{path}",
            "name": "Repository File",
            "description": "Read a file",
            "mimeType": "text/plain"
        }))
        .unwrap();
        assert_eq!(resource.uri_template, "repo://files/{path}");
        assert_eq!(resource.mime_type.as_deref(), Some("text/plain"));

        let prompt = prompt_from_value(&json!({
            "name": "review_change",
            "description": "Review a change",
            "arguments": [
                {"name": "diff", "description": "Patch", "required": true},
                {"name": "tone"}
            ]
        }))
        .unwrap();
        assert_eq!(prompt.arguments.len(), 2);
        assert!(prompt.arguments[0].required);
        assert!(!prompt.arguments[1].required);
    }
}

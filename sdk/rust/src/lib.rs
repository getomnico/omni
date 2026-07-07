pub mod client;
pub mod connector;
pub mod context;
pub mod mcp_adapter;
pub mod models;
pub mod server;

pub use client::{build_connector_url, SdkClient, SdkError, SdkResult};
pub use connector::{Connector, SyncRequestValidationError};
pub use context::SyncContext;
pub use mcp_adapter::{HttpMcpServer, McpAdapter, McpServer, StdioMcpServer};
pub use models::{
    ActionActor, ActionContext, ActionRequest, ActionResponse, CancelRequest, CancelResponse,
    McpCredentials, OAuthManifestConfig, OAuthScopeSet, OAuthTokenEndpointAuthMethod,
    PromptRequest, ResourceRequest, SkillRequest, SkillResponse, SyncRequest, SyncResponse,
    SyncStatusResponse,
};
pub use server::{create_router, serve, serve_with_config, serve_with_extra_routes, ServerConfig};

pub use shared::models::DocumentAttributes;
pub use shared::models::{
    ActionDefinition, ActionMode, AuthType, ConnectorEvent, ConnectorManifest,
    ConnectorSkillDefinition, DocumentMetadata, DocumentPermissions, McpPromptDefinition,
    McpResourceDefinition, SearchOperator, ServiceCredential, ServiceProvider, Source, SourceType,
    SyncRun, SyncStatus, SyncType,
};
pub use shared::rate_limiter::{RateLimiter, RetryableError};
pub use shared::telemetry;

pub mod content_extractor {
    pub use shared::content_extractor::extract_content;
}

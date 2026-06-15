pub mod client;
pub mod connector;
pub mod context;
pub mod mcp_adapter;
pub mod models;
pub mod server;

pub use client::{SdkClient, SdkError, SdkResult, build_connector_url};
pub use connector::{Connector, SyncRequestValidationError};
pub use context::SyncContext;
pub use mcp_adapter::{HttpMcpServer, McpAdapter, McpCredentials, McpServer, StdioMcpServer};
pub use models::{
    ActionRequest, ActionResponse, CancelRequest, CancelResponse, OAuthManifestConfig,
    OAuthScopeSet, PromptRequest, ResourceRequest, SyncRequest, SyncResponse, SyncStatusResponse,
};
pub use server::{ServerConfig, create_router, serve, serve_with_config, serve_with_extra_routes};

pub use shared::models::{
    ActionDefinition, ActionMode, AuthType, ConnectorEvent, ConnectorManifest, DocumentMetadata,
    DocumentPermissions, McpPromptDefinition, McpResourceDefinition, SearchOperator,
    ServiceCredential, ServiceProvider, Source, SourceType, SyncRun, SyncStatus, SyncType,
};
pub use shared::models::{ConfluenceSourceConfig, DocumentAttributes, JiraSourceConfig};
pub use shared::rate_limiter::{RateLimiter, RetryableError};
pub use shared::telemetry;

pub mod content_extractor {
    pub use shared::content_extractor::extract_content;
}

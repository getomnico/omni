pub mod connector;
pub mod context;
pub mod models;
pub mod server;

pub use connector::Connector;
pub use context::SyncContext;
pub use models::{
    ActionRequest, ActionResponse, CancelRequest, CancelResponse, SyncRequest, SyncResponse,
};
pub use server::{create_router, serve, serve_with_config, ServerConfig};

pub use shared::models::{
    ActionDefinition, ConnectorEvent, ConnectorManifest, DocumentMetadata, DocumentPermissions,
    McpPromptDefinition, McpResourceDefinition, SearchOperator, ServiceCredentials, Source,
    SourceType,
};
pub use shared::{SdkClient, SdkError, SdkResult};

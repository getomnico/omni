pub mod connector;
pub mod context;
pub mod models;
pub mod server;

pub use connector::Connector;
pub use context::SyncContext;
pub use models::{
    ActionRequest, ActionResponse, CancelRequest, CancelResponse, SyncRequest, SyncResponse,
    SyncStatusResponse,
};
pub use server::{create_router, serve, serve_with_config, ServerConfig};

pub use shared::models::{
    ActionDefinition, ConnectorEvent, ConnectorManifest, DocumentMetadata, DocumentPermissions,
    McpPromptDefinition, McpResourceDefinition, SearchOperator, ServiceCredentials, Source,
    SourceType, SyncRun, SyncStatus, SyncType,
};
pub use shared::telemetry;
pub use shared::{build_connector_url, SdkClient, SdkError, SdkResult};

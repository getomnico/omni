pub mod auth;
pub mod client;
pub mod config;
pub mod confluence;
pub mod connector;
pub mod jira;
pub mod models;
pub mod routes;
pub mod sync;
pub mod user_resolver;

pub use auth::{AtlassianCredentials, AuthManager};
pub use client::{AtlassianApi, AtlassianClient};
pub use config::AtlassianConnectorConfig;
pub use confluence::ConfluenceProcessor;
pub use connector::{AtlassianConnector, handle_search_spaces};
pub use jira::JiraProcessor;
pub use sync::SyncManager;

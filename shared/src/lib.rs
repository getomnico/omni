pub mod clients;
pub mod config;
pub mod constants;
pub mod db;
pub mod models;
pub mod queue;
pub mod service_auth;
pub mod traits;
pub mod utils;

pub mod test_utils;

pub mod test_environment;

pub use clients::ai::AIClient;
pub use config::*;
pub use db::repositories::{
    DocumentRepository, EmbeddingRepository, SourceRepository, UserRepository,
};
pub use db::{DatabaseError, DatabasePool};
pub use models::*;
pub use queue::{EventQueue, QueueStats};
pub use service_auth::{create_service_auth, ServiceAuth, ServiceCredentialsRepo};
pub use traits::Repository;

pub fn init() {
    println!("Shared library initialized");
}

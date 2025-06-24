pub mod clients;
pub mod config;
pub mod constants;
pub mod db;
pub mod embedding_queue;
pub mod models;
pub mod queue;
pub mod rate_limiter;
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
pub use embedding_queue::{EmbeddingQueue, EmbeddingQueueItem};
pub use models::*;
pub use queue::{EventQueue, QueueStats};
pub use rate_limiter::RateLimiter;
pub use service_auth::{create_service_auth, ServiceAuth, ServiceCredentialsRepo};
pub use traits::Repository;

pub fn init() {
    println!("Shared library initialized");
}

pub mod clients;
pub mod config;
pub mod constants;
pub mod content_chunker;
pub mod content_extractor;
pub mod content_storage;
pub mod db;
pub mod embedding_queue;
pub mod encryption;
pub mod models;
pub mod queue;
pub mod rate_limiter;
pub mod sdk_client;
pub mod service_auth;
pub mod storage;
pub mod telemetry;
pub mod traits;
pub mod utils;

pub mod test_utils;

pub mod test_environment;

pub use clients::ai::AIClient;
pub use config::*;
pub use content_chunker::ContentChunker;
pub use content_storage::{ContentStorage, ContentStorageError};
pub use db::repositories::{
    ActionMode, ConnectorConfigRepository, CredentialResolutionError, DocumentRepository,
    EmbeddingRepository, GroupRepository, PersonRepository, PersonSearchResult, PersonUpsert,
    ServiceCredentialsRepo, SourceRepository, TitleEntry, UserRepository,
};
pub use db::{DatabaseError, DatabasePool};
pub use embedding_queue::{EmbeddingQueue, EmbeddingQueueItem};
pub use encryption::{EncryptedData, EncryptionService};
pub use models::*;
pub use queue::{EventQueue, QueueStats, QueueSummary};
pub use rate_limiter::{RateLimiter, RetryableError};
pub use sdk_client::{
    build_connector_url, start_registration_loop, SdkClient, SdkError, SdkResult,
};
pub use service_auth::{create_service_auth, ServiceAuth};
pub use storage::{
    factory::{StorageBackend, StorageFactory},
    ContentMetadata as StorageContentMetadata, ObjectStorage, StorageError,
};
pub use traits::Repository;

pub fn init() {
    println!("Shared library initialized");
}

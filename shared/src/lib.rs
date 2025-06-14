pub mod clients;
pub mod config;
pub mod constants;
pub mod db;
pub mod models;
pub mod traits;

pub mod test_utils;

pub use clients::ai::AIClient;
pub use config::*;
pub use constants::*;
pub use db::repositories::{
    DocumentRepository, EmbeddingRepository, SourceRepository, UserRepository,
};
pub use db::{DatabaseError, DatabasePool};
pub use models::*;
pub use traits::Repository;

pub fn init() {
    println!("Shared library initialized");
}

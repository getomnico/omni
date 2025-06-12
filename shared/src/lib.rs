pub mod db;
pub mod models;
pub mod traits;

pub use db::repositories::{
    DocumentRepository, EmbeddingRepository, SourceRepository, UserRepository,
};
pub use db::{DatabaseError, DatabasePool};
pub use models::*;
pub use traits::Repository;

pub fn init() {
    println!("Shared library initialized");
}

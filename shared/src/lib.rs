pub mod models;
pub mod db;
pub mod traits;

pub use models::*;
pub use db::{DatabasePool, DatabaseError};
pub use db::repositories::{
    UserRepository,
    SourceRepository,
    DocumentRepository,
    EmbeddingRepository,
};
pub use traits::Repository;

pub fn init() {
    println!("Shared library initialized");
}
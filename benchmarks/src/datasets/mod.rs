pub mod beir;
pub mod custom;
pub mod msmarco;
pub mod natural_questions;

pub use beir::*;
#[allow(unused_imports)]
pub use custom::*;
pub use msmarco::*;
#[allow(unused_imports)]
pub use natural_questions::*;

use anyhow::Result;
use async_trait::async_trait;
use futures::Stream;
use serde::{Deserialize, Serialize};
use std::pin::Pin;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Dataset {
    pub name: String,
    pub queries: Vec<Query>,
    pub documents: Vec<Document>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Query {
    pub id: String,
    pub text: String,
    pub relevant_docs: Vec<RelevantDoc>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Document {
    pub id: String,
    pub title: String,
    pub content: String,
    pub metadata: std::collections::HashMap<String, String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RelevantDoc {
    pub doc_id: String,
    pub relevance_score: f64,
}

#[async_trait]
pub trait DatasetLoader: Send + Sync {
    async fn download(&self) -> Result<()>;
    async fn load_dataset(&self) -> Result<Dataset>;
    fn get_name(&self) -> String;
    fn get_cache_dir(&self) -> String;

    // Streaming methods for memory-efficient processing
    fn stream_documents(&self) -> Pin<Box<dyn Stream<Item = Result<Document>> + Send>>;
    fn stream_queries(&self) -> Pin<Box<dyn Stream<Item = Result<Query>> + Send>>;
}

use crate::datasets::{Dataset, DatasetLoader, Document, Query, RelevantDoc};
use anyhow::Result;
use async_trait::async_trait;
use futures::stream::{self};
use futures::Stream;
use std::collections::HashMap;
use std::fs::File;
use std::io::{BufRead, BufReader};
use std::path::PathBuf;
use std::pin::Pin;
use tracing::info;
#[cfg(test)]
use {futures::StreamExt, std::path::Path};

/// Dataset loader for prepared Natural Questions benchmark data.
///
/// Expects the following files in the data directory:
/// - corpus.jsonl: Documents with {id, title, text}
/// - queries.jsonl: Queries with {id, text, relevant_doc_id?}
/// - metadata.json: Statistics about the dataset
pub struct NaturalQuestionsDataset {
    data_dir: PathBuf,
    max_documents: Option<usize>,
    max_queries: Option<usize>,
}

impl NaturalQuestionsDataset {
    pub fn new(data_dir: String) -> Self {
        Self {
            data_dir: PathBuf::from(data_dir),
            max_documents: None,
            max_queries: None,
        }
    }

    pub fn with_max_documents(mut self, max: usize) -> Self {
        self.max_documents = Some(max);
        self
    }

    pub fn with_max_queries(mut self, max: usize) -> Self {
        self.max_queries = Some(max);
        self
    }

    fn corpus_path(&self) -> PathBuf {
        self.data_dir.join("corpus.jsonl")
    }

    fn queries_path(&self) -> PathBuf {
        self.data_dir.join("queries.jsonl")
    }

    fn metadata_path(&self) -> PathBuf {
        self.data_dir.join("metadata.json")
    }

    fn load_corpus(&self) -> Result<Vec<Document>> {
        let path = self.corpus_path();
        if !path.exists() {
            return Err(anyhow::anyhow!(
                "Corpus file not found: {}. Run prepare_nq_data.py first.",
                path.display()
            ));
        }

        let file = File::open(&path)?;
        let reader = BufReader::new(file);
        let mut documents = Vec::new();
        let mut count = 0;

        for line in reader.lines() {
            let line = line?;
            if line.trim().is_empty() {
                continue;
            }

            let doc_data: serde_json::Value = serde_json::from_str(&line)?;

            let id = doc_data["id"]
                .as_str()
                .ok_or_else(|| anyhow::anyhow!("Missing id field"))?;
            let title = doc_data["title"].as_str().unwrap_or("");
            let text = doc_data["text"]
                .as_str()
                .ok_or_else(|| anyhow::anyhow!("Missing text field"))?;

            documents.push(Document {
                id: id.to_string(),
                title: title.to_string(),
                content: text.to_string(),
                metadata: HashMap::new(),
            });

            count += 1;
            if let Some(max) = self.max_documents {
                if count >= max {
                    break;
                }
            }
        }

        info!("Loaded {} documents from NQ corpus", documents.len());
        Ok(documents)
    }

    fn load_queries_with_rels(&self) -> Result<Vec<Query>> {
        let path = self.queries_path();
        if !path.exists() {
            return Err(anyhow::anyhow!(
                "Queries file not found: {}. Run prepare_nq_data.py first.",
                path.display()
            ));
        }

        let file = File::open(&path)?;
        let reader = BufReader::new(file);
        let mut queries = Vec::new();
        let mut count = 0;

        for line in reader.lines() {
            let line = line?;
            if line.trim().is_empty() {
                continue;
            }

            let query_data: serde_json::Value = serde_json::from_str(&line)?;

            let id = query_data["id"]
                .as_str()
                .ok_or_else(|| anyhow::anyhow!("Missing id field"))?;
            let text = query_data["text"]
                .as_str()
                .ok_or_else(|| anyhow::anyhow!("Missing text field"))?;

            // Get relevant doc if available (for relevance evaluation)
            let relevant_docs = if let Some(rel_doc_id) = query_data["relevant_doc_id"].as_str() {
                vec![RelevantDoc {
                    doc_id: rel_doc_id.to_string(),
                    relevance_score: 1.0,
                }]
            } else {
                vec![]
            };

            queries.push(Query {
                id: id.to_string(),
                text: text.to_string(),
                relevant_docs,
            });

            count += 1;
            if let Some(max) = self.max_queries {
                if count >= max {
                    break;
                }
            }
        }

        info!("Loaded {} queries from NQ dataset", queries.len());
        Ok(queries)
    }

    fn stream_corpus_impl(&self) -> Pin<Box<dyn Stream<Item = Result<Document>> + Send>> {
        let path = self.corpus_path();
        let max_documents = self.max_documents;

        if !path.exists() {
            return Box::pin(stream::once(async move {
                Err(anyhow::anyhow!(
                    "Corpus file not found: {}. Run prepare_nq_data.py first.",
                    path.display()
                ))
            }));
        }

        Box::pin(stream::try_unfold(
            (path, max_documents, None::<BufReader<File>>, 0usize),
            move |(path, max_docs, mut reader_opt, count)| async move {
                // Check max documents limit
                if let Some(max) = max_docs {
                    if count >= max {
                        return Ok(None);
                    }
                }

                // Initialize reader if needed
                if reader_opt.is_none() {
                    let file = File::open(&path).map_err(|e| {
                        anyhow::anyhow!("Failed to open corpus file {}: {}", path.display(), e)
                    })?;
                    reader_opt = Some(BufReader::new(file));
                }

                let reader = reader_opt.as_mut().unwrap();
                let mut line = String::new();

                loop {
                    line.clear();
                    match reader.read_line(&mut line) {
                        Ok(0) => return Ok(None), // EOF
                        Ok(_) => {
                            if line.trim().is_empty() {
                                continue;
                            }

                            let doc_data: serde_json::Value = match serde_json::from_str(&line) {
                                Ok(data) => data,
                                Err(e) => {
                                    tracing::warn!("Failed to parse JSON line: {}", e);
                                    continue;
                                }
                            };

                            let id = match doc_data["id"].as_str() {
                                Some(id) => id,
                                None => continue,
                            };
                            let title = doc_data["title"].as_str().unwrap_or("");
                            let text = match doc_data["text"].as_str() {
                                Some(text) => text,
                                None => continue,
                            };

                            let document = Document {
                                id: id.to_string(),
                                title: title.to_string(),
                                content: text.to_string(),
                                metadata: HashMap::new(),
                            };

                            return Ok(Some((document, (path, max_docs, reader_opt, count + 1))));
                        }
                        Err(e) => return Err(anyhow::anyhow!("Failed to read line: {}", e)),
                    }
                }
            },
        ))
    }

    fn stream_queries_impl(&self) -> Pin<Box<dyn Stream<Item = Result<Query>> + Send>> {
        let path = self.queries_path();
        let max_queries = self.max_queries;

        if !path.exists() {
            return Box::pin(stream::once(async move {
                Err(anyhow::anyhow!(
                    "Queries file not found: {}. Run prepare_nq_data.py first.",
                    path.display()
                ))
            }));
        }

        Box::pin(stream::try_unfold(
            (path, max_queries, None::<BufReader<File>>, 0usize),
            move |(path, max_queries, mut reader_opt, count)| async move {
                // Check max queries limit
                if let Some(max) = max_queries {
                    if count >= max {
                        return Ok(None);
                    }
                }

                // Initialize reader if needed
                if reader_opt.is_none() {
                    let file = File::open(&path).map_err(|e| {
                        anyhow::anyhow!("Failed to open queries file {}: {}", path.display(), e)
                    })?;
                    reader_opt = Some(BufReader::new(file));
                }

                let reader = reader_opt.as_mut().unwrap();
                let mut line = String::new();

                loop {
                    line.clear();
                    match reader.read_line(&mut line) {
                        Ok(0) => return Ok(None), // EOF
                        Ok(_) => {
                            if line.trim().is_empty() {
                                continue;
                            }

                            let query_data: serde_json::Value = match serde_json::from_str(&line) {
                                Ok(data) => data,
                                Err(e) => {
                                    tracing::warn!("Failed to parse query JSON line: {}", e);
                                    continue;
                                }
                            };

                            let id = match query_data["id"].as_str() {
                                Some(id) => id,
                                None => continue,
                            };
                            let text = match query_data["text"].as_str() {
                                Some(text) => text,
                                None => continue,
                            };

                            // Get relevant doc if available
                            let relevant_docs =
                                if let Some(rel_doc_id) = query_data["relevant_doc_id"].as_str() {
                                    vec![RelevantDoc {
                                        doc_id: rel_doc_id.to_string(),
                                        relevance_score: 1.0,
                                    }]
                                } else {
                                    vec![]
                                };

                            let query = Query {
                                id: id.to_string(),
                                text: text.to_string(),
                                relevant_docs,
                            };

                            return Ok(Some((query, (path, max_queries, reader_opt, count + 1))));
                        }
                        Err(e) => return Err(anyhow::anyhow!("Failed to read line: {}", e)),
                    }
                }
            },
        ))
    }
}

#[async_trait]
impl DatasetLoader for NaturalQuestionsDataset {
    async fn download(&self) -> Result<()> {
        // Data is prepared by prepare_nq_data.py script
        if !self.data_dir.exists() {
            return Err(anyhow::anyhow!(
                "NQ benchmark data not found at {}. Run prepare_nq_data.py first.",
                self.data_dir.display()
            ));
        }

        if !self.corpus_path().exists() {
            return Err(anyhow::anyhow!(
                "Corpus file not found. Run prepare_nq_data.py first."
            ));
        }

        if !self.queries_path().exists() {
            return Err(anyhow::anyhow!(
                "Queries file not found. Run prepare_nq_data.py first."
            ));
        }

        info!("NQ benchmark data found at {}", self.data_dir.display());
        Ok(())
    }

    async fn load_dataset(&self) -> Result<Dataset> {
        self.download().await?;

        let documents = self.load_corpus()?;
        let queries = self.load_queries_with_rels()?;

        Ok(Dataset {
            name: self.get_name(),
            queries,
            documents,
        })
    }

    fn get_name(&self) -> String {
        "natural-questions".to_string()
    }

    fn get_cache_dir(&self) -> String {
        self.data_dir.to_string_lossy().to_string()
    }

    fn stream_documents(&self) -> Pin<Box<dyn Stream<Item = Result<Document>> + Send>> {
        self.stream_corpus_impl()
    }

    fn stream_queries(&self) -> Pin<Box<dyn Stream<Item = Result<Query>> + Send>> {
        self.stream_queries_impl()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::TempDir;

    fn create_test_data(dir: &Path) -> Result<()> {
        // Create test corpus
        let corpus_data = r#"{"id": "doc1", "title": "Test Document", "text": "This is test content."}
{"id": "doc2", "title": "Another Doc", "text": "More test content here."}"#;
        fs::write(dir.join("corpus.jsonl"), corpus_data)?;

        // Create test queries
        let queries_data = r#"{"id": "q1", "text": "What is the test?", "relevant_doc_id": "doc1"}
{"id": "q2", "text": "Another question?"}"#;
        fs::write(dir.join("queries.jsonl"), queries_data)?;

        // Create metadata
        let metadata = r#"{"total_documents": 2, "total_queries": 2}"#;
        fs::write(dir.join("metadata.json"), metadata)?;

        Ok(())
    }

    #[tokio::test]
    async fn test_nq_dataset_load() {
        let temp_dir = TempDir::new().unwrap();
        create_test_data(temp_dir.path()).unwrap();

        let dataset = NaturalQuestionsDataset::new(temp_dir.path().to_string_lossy().to_string());

        let loaded = dataset.load_dataset().await.unwrap();
        assert_eq!(loaded.documents.len(), 2);
        assert_eq!(loaded.queries.len(), 2);
        assert_eq!(loaded.documents[0].id, "doc1");
        assert_eq!(loaded.queries[0].text, "What is the test?");
    }

    #[tokio::test]
    async fn test_nq_dataset_stream() {
        let temp_dir = TempDir::new().unwrap();
        create_test_data(temp_dir.path()).unwrap();

        let dataset = NaturalQuestionsDataset::new(temp_dir.path().to_string_lossy().to_string());

        // Test document streaming
        let docs: Vec<_> = dataset.stream_documents().collect().await;
        assert_eq!(docs.len(), 2);
        assert!(docs[0].is_ok());

        // Test query streaming
        let queries: Vec<_> = dataset.stream_queries().collect().await;
        assert_eq!(queries.len(), 2);
        assert!(queries[0].is_ok());
    }

    #[tokio::test]
    async fn test_max_documents_limit() {
        let temp_dir = TempDir::new().unwrap();
        create_test_data(temp_dir.path()).unwrap();

        let dataset = NaturalQuestionsDataset::new(temp_dir.path().to_string_lossy().to_string())
            .with_max_documents(1);

        let docs: Vec<_> = dataset.stream_documents().collect().await;
        assert_eq!(docs.len(), 1);
    }
}

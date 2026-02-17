use crate::datasets::{Dataset, DatasetLoader, Document, Query, RelevantDoc};
use anyhow::Result;
use async_trait::async_trait;
use flate2::read::GzDecoder;
use futures::stream::{self};
use futures::Stream;
use indicatif::{ProgressBar, ProgressStyle};
use std::collections::HashMap;
use std::fs::{self, File};
use std::io::{BufRead, BufReader};
use std::path::Path;
use std::pin::Pin;
use tar::Archive;
use tracing::info;

pub struct MsMarcoDataset {
    cache_dir: String,
    dataset_type: String, // "passage" or "document"
    query_url: String,
    corpus_url: String,
    qrels_url: String,
}

impl MsMarcoDataset {
    pub fn new(cache_dir: String) -> Self {
        Self {
            cache_dir,
            dataset_type: "passage".to_string(),
            query_url: "https://msmarco.blob.core.windows.net/msmarcoranking/queries.tar.gz"
                .to_string(),
            corpus_url: "https://msmarco.blob.core.windows.net/msmarcoranking/collection.tar.gz"
                .to_string(),
            qrels_url: "https://msmarco.blob.core.windows.net/msmarcoranking/qrels.dev.small.tsv"
                .to_string(),
        }
    }

    pub fn with_dataset_type(mut self, dataset_type: String) -> Self {
        self.dataset_type = dataset_type;
        self
    }

    pub fn with_urls(mut self, query_url: String, corpus_url: String, qrels_url: String) -> Self {
        self.query_url = query_url;
        self.corpus_url = corpus_url;
        self.qrels_url = qrels_url;
        self
    }

    async fn download_file(&self, url: &str, output_path: &str) -> Result<()> {
        if Path::new(output_path).exists() {
            info!("File already exists: {}", output_path);
            return Ok(());
        }

        info!("Downloading: {}", url);
        let response = reqwest::get(url).await?;

        if !response.status().is_success() {
            return Err(anyhow::anyhow!(
                "Failed to download file: HTTP {}",
                response.status()
            ));
        }

        let total_size = response.content_length().unwrap_or(0);
        let progress_bar = ProgressBar::new(total_size);
        progress_bar.set_style(
            ProgressStyle::default_bar()
                .template("{spinner:.green} [{elapsed_precise}] [{bar:40.cyan/blue}] {bytes}/{total_bytes} ({eta})")
                .unwrap()
                .progress_chars("#>-"),
        );

        let mut file = std::fs::File::create(output_path)?;
        let mut reader = response.bytes_stream();
        let mut downloaded = 0u64;

        use futures_util::StreamExt;
        use std::io::Write;

        while let Some(item) = reader.next().await {
            let chunk = item?;
            file.write_all(&chunk)?;
            downloaded += chunk.len() as u64;
            progress_bar.set_position(downloaded);
        }

        progress_bar.finish_with_message("Download completed");
        Ok(())
    }

    fn extract_tar_gz(&self, archive_path: &str, extract_dir: &str) -> Result<()> {
        info!("Extracting: {}", archive_path);

        let tar_gz = File::open(archive_path)?;
        let tar = GzDecoder::new(tar_gz);
        let mut archive = Archive::new(tar);

        fs::create_dir_all(extract_dir)?;
        archive.unpack(extract_dir)?;

        info!("Extraction completed: {}", extract_dir);
        Ok(())
    }

    fn load_queries(&self, queries_file: &str) -> Result<HashMap<String, String>> {
        let file = File::open(queries_file)?;
        let reader = BufReader::new(file);
        let mut queries = HashMap::new();

        for line in reader.lines() {
            let line = line?;
            if line.trim().is_empty() {
                continue;
            }

            let parts: Vec<&str> = line.split('\t').collect();
            if parts.len() >= 2 {
                let query_id = parts[0].to_string();
                let query_text = parts[1].to_string();
                queries.insert(query_id, query_text);
            }
        }

        info!("Loaded {} queries", queries.len());
        Ok(queries)
    }

    fn load_corpus(&self, corpus_file: &str) -> Result<Vec<Document>> {
        let file = File::open(corpus_file)?;
        let reader = BufReader::new(file);
        let mut documents = Vec::new();

        for line in reader.lines() {
            let line = line?;
            if line.trim().is_empty() {
                continue;
            }

            let parts: Vec<&str> = line.split('\t').collect();
            if parts.len() >= 2 {
                let doc_id = parts[0].to_string();
                let content = parts[1].to_string();

                // For MS MARCO passages, title is often the first sentence or empty
                let title = if content.len() > 100 {
                    format!("{}...", &content[..100])
                } else {
                    content.clone()
                };

                documents.push(Document {
                    id: doc_id,
                    title,
                    content,
                    metadata: HashMap::new(),
                });
            }
        }

        info!("Loaded {} documents", documents.len());
        Ok(documents)
    }

    fn load_qrels(&self, qrels_file: &str) -> Result<HashMap<String, Vec<(String, f64)>>> {
        let file = File::open(qrels_file)?;
        let reader = BufReader::new(file);
        let mut qrels = HashMap::new();

        for line in reader.lines() {
            let line = line?;
            if line.trim().is_empty() {
                continue;
            }

            let parts: Vec<&str> = line.split('\t').collect();
            if parts.len() >= 4 {
                let query_id = parts[0].to_string();
                let doc_id = parts[2].to_string();
                let relevance: f64 = parts[3].parse().unwrap_or(0.0);

                qrels
                    .entry(query_id)
                    .or_insert_with(Vec::new)
                    .push((doc_id, relevance));
            }
        }

        info!("Loaded qrels for {} queries", qrels.len());
        Ok(qrels)
    }

    fn combine_queries_and_qrels(
        &self,
        queries: HashMap<String, String>,
        qrels: HashMap<String, Vec<(String, f64)>>,
    ) -> Result<Vec<Query>> {
        let mut combined_queries = Vec::new();

        for (query_id, query_text) in queries {
            let relevant_docs: Vec<RelevantDoc> = qrels
                .get(&query_id)
                .map(|rels| {
                    rels.iter()
                        .map(|(doc_id, relevance)| RelevantDoc {
                            doc_id: doc_id.clone(),
                            relevance_score: *relevance,
                        })
                        .collect()
                })
                .unwrap_or_default();

            // Only include queries that have relevant documents
            if !relevant_docs.is_empty() {
                combined_queries.push(Query {
                    id: query_id,
                    text: query_text,
                    relevant_docs,
                });
            }
        }

        info!(
            "Combined {} queries with relevance judgments",
            combined_queries.len()
        );
        Ok(combined_queries)
    }

    fn stream_corpus_file(
        &self,
        corpus_file: &str,
    ) -> Pin<Box<dyn Stream<Item = Result<Document>> + Send>> {
        let corpus_file = corpus_file.to_string();
        Box::pin(stream::try_unfold(
            (corpus_file, None::<BufReader<File>>),
            move |(corpus_file, mut reader_opt)| async move {
                // Initialize reader if needed
                if reader_opt.is_none() {
                    let file = File::open(&corpus_file).map_err(|e| {
                        anyhow::anyhow!("Failed to open corpus file {}: {}", corpus_file, e)
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

                            let parts: Vec<&str> = line.split('\t').collect();
                            if parts.len() >= 2 {
                                let doc_id = parts[0].to_string();
                                let content = parts[1].to_string();

                                // For MS MARCO passages, title is often the first sentence or empty
                                let title = if content.len() > 100 {
                                    format!("{}...", &content[..100])
                                } else {
                                    content.clone()
                                };

                                let document = Document {
                                    id: doc_id,
                                    title,
                                    content,
                                    metadata: HashMap::new(),
                                };

                                return Ok(Some((document, (corpus_file, reader_opt))));
                            }
                        }
                        Err(e) => return Err(anyhow::anyhow!("Failed to read line: {}", e)),
                    }
                }
            },
        ))
    }

    fn stream_corpus_queries(
        &self,
        queries_file: &str,
        qrels_file: &str,
    ) -> Pin<Box<dyn Stream<Item = Result<Query>> + Send>> {
        let queries_file = queries_file.to_string();
        let qrels_file = qrels_file.to_string();

        Box::pin(stream::try_unfold(
            (
                queries_file,
                qrels_file,
                None::<(
                    HashMap<String, String>,
                    HashMap<String, Vec<(String, f64)>>,
                    std::collections::hash_map::IntoIter<String, String>,
                )>,
            ),
            move |(queries_file, qrels_file, mut state_opt)| async move {
                // Initialize state if needed
                if state_opt.is_none() {
                    // Create a new instance to avoid borrowing self
                    let temp_dataset = MsMarcoDataset::new("".to_string());
                    let queries = temp_dataset.load_queries(&queries_file)?;
                    let qrels = temp_dataset.load_qrels(&qrels_file)?;
                    let queries_iter = queries.into_iter();
                    state_opt = Some((HashMap::new(), qrels, queries_iter));
                }

                let (_, qrels, queries_iter) = state_opt.as_mut().unwrap();

                while let Some((query_id, query_text)) = queries_iter.next() {
                    let relevant_docs: Vec<RelevantDoc> = qrels
                        .get(&query_id)
                        .map(|rels| {
                            rels.iter()
                                .map(|(doc_id, relevance)| RelevantDoc {
                                    doc_id: doc_id.clone(),
                                    relevance_score: *relevance,
                                })
                                .collect()
                        })
                        .unwrap_or_default();

                    // Only yield queries that have relevant documents
                    if !relevant_docs.is_empty() {
                        let query = Query {
                            id: query_id,
                            text: query_text,
                            relevant_docs,
                        };
                        return Ok(Some((query, (queries_file, qrels_file, state_opt))));
                    }
                }

                Ok(None) // No more queries
            },
        ))
    }
}

#[async_trait]
impl DatasetLoader for MsMarcoDataset {
    async fn download(&self) -> Result<()> {
        fs::create_dir_all(&self.cache_dir)?;

        // Download queries
        let queries_archive = format!("{}/queries.tar.gz", self.cache_dir);
        self.download_file(&self.query_url, &queries_archive)
            .await?;

        // Extract queries
        let queries_dir = format!("{}/queries", self.cache_dir);
        self.extract_tar_gz(&queries_archive, &queries_dir)?;

        // Download corpus
        let corpus_archive = format!("{}/collection.tar.gz", self.cache_dir);
        self.download_file(&self.corpus_url, &corpus_archive)
            .await?;

        // Extract corpus
        let corpus_dir = format!("{}/collection", self.cache_dir);
        self.extract_tar_gz(&corpus_archive, &corpus_dir)?;

        // Download qrels (relevance judgments)
        let qrels_file = format!("{}/qrels.dev.small.tsv", self.cache_dir);
        self.download_file(&self.qrels_url, &qrels_file).await?;

        // Clean up archives
        let _ = fs::remove_file(&queries_archive);
        let _ = fs::remove_file(&corpus_archive);

        info!("MS MARCO dataset download completed");
        Ok(())
    }

    async fn load_dataset(&self) -> Result<Dataset> {
        let queries_file = format!("{}/queries/queries.dev.small.tsv", self.cache_dir);
        let corpus_file = format!("{}/collection/collection.tsv", self.cache_dir);
        let qrels_file = format!("{}/qrels.dev.small.tsv", self.cache_dir);

        // Check if files exist
        for file in &[&queries_file, &corpus_file, &qrels_file] {
            if !Path::new(file).exists() {
                return Err(anyhow::anyhow!(
                    "MS MARCO dataset file not found: {}. Please download first.",
                    file
                ));
            }
        }

        info!("Loading MS MARCO dataset");

        // Load components
        let queries = self.load_queries(&queries_file)?;
        let documents = self.load_corpus(&corpus_file)?;
        let qrels = self.load_qrels(&qrels_file)?;

        // Combine queries with their relevant documents
        let queries_with_rels = self.combine_queries_and_qrels(queries, qrels)?;

        Ok(Dataset {
            name: format!("MS-MARCO-{}", self.dataset_type),
            queries: queries_with_rels,
            documents,
        })
    }

    fn get_name(&self) -> String {
        format!("MS-MARCO-{}", self.dataset_type)
    }

    fn get_cache_dir(&self) -> String {
        self.cache_dir.clone()
    }

    fn stream_documents(&self) -> Pin<Box<dyn Stream<Item = Result<Document>> + Send>> {
        let corpus_file = format!("{}/collection/collection.tsv", self.cache_dir);

        // Check if file exists
        if !Path::new(&corpus_file).exists() {
            return Box::pin(stream::empty());
        }

        self.stream_corpus_file(&corpus_file)
    }

    fn stream_queries(&self) -> Pin<Box<dyn Stream<Item = Result<Query>> + Send>> {
        let queries_file = format!("{}/queries/queries.dev.small.tsv", self.cache_dir);
        let qrels_file = format!("{}/qrels.dev.small.tsv", self.cache_dir);

        // Check if files exist
        if !Path::new(&queries_file).exists() || !Path::new(&qrels_file).exists() {
            return Box::pin(stream::empty());
        }

        self.stream_corpus_queries(&queries_file, &qrels_file)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[tokio::test]
    async fn test_msmarco_dataset_creation() {
        let temp_dir = TempDir::new().unwrap();
        let cache_dir = temp_dir.path().to_str().unwrap().to_string();

        let dataset =
            MsMarcoDataset::new(cache_dir.clone()).with_dataset_type("passage".to_string());

        assert_eq!(dataset.get_name(), "MS-MARCO-passage");
        assert_eq!(dataset.get_cache_dir(), cache_dir);
    }
}

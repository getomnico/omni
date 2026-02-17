use crate::datasets::{Dataset, DatasetLoader, Document, Query, RelevantDoc};
use anyhow::Result;
use async_trait::async_trait;
use futures::stream::{self};
use futures::Stream;
use indicatif::{ProgressBar, ProgressStyle};
use std::collections::HashMap;
use std::fs::{self, File};
use std::io::{BufRead, BufReader};
use std::path::Path;
use std::pin::Pin;
use tracing::info;

pub struct BeirDataset {
    cache_dir: String,
    dataset_names: Vec<String>,
    download_url_base: String,
    selected_dataset: Option<String>,
}

impl BeirDataset {
    pub fn new(cache_dir: String) -> Self {
        Self {
            cache_dir,
            dataset_names: vec![
                "nfcorpus".to_string(),
                "fiqa".to_string(),
                "trec-covid".to_string(),
                "scifact".to_string(),
                "scidocs".to_string(),
                "nq".to_string(),
                "hotpotqa".to_string(),
                "climate-fever".to_string(),
                "fever".to_string(),
                "dbpedia-entity".to_string(),
                "webis-touche2020".to_string(),
                "quora".to_string(),
            ],
            download_url_base: "https://public.ukp.informatik.tu-darmstadt.de/thakur/BEIR/datasets"
                .to_string(),
            selected_dataset: None,
        }
    }

    pub fn with_datasets(mut self, dataset_names: Vec<String>) -> Self {
        self.dataset_names = dataset_names;
        self
    }

    pub fn with_download_url(mut self, url_base: String) -> Self {
        self.download_url_base = url_base;
        self
    }

    pub fn with_selected_dataset(mut self, dataset_name: String) -> Self {
        self.selected_dataset = Some(dataset_name);
        self
    }

    fn get_dataset_to_use(&self) -> Option<&str> {
        if let Some(ref selected) = self.selected_dataset {
            // Verify the selected dataset is in the available list
            if self.dataset_names.contains(selected) {
                Some(selected)
            } else {
                None
            }
        } else {
            // Return first available dataset if none selected
            self.dataset_names.first().map(|s| s.as_str())
        }
    }

    pub async fn download_all(&self) -> Result<()> {
        fs::create_dir_all(&self.cache_dir)?;

        for dataset_name in &self.dataset_names {
            info!("Downloading BEIR dataset: {}", dataset_name);
            self.download_single_dataset(dataset_name).await?;
        }

        Ok(())
    }

    async fn download_single_dataset(&self, dataset_name: &str) -> Result<()> {
        let dataset_dir = format!("{}/{}", self.cache_dir, dataset_name);

        // Check if already downloaded
        if Path::new(&dataset_dir).exists() {
            info!("Dataset {} already exists, skipping download", dataset_name);
            return Ok(());
        }

        let download_url = format!("{}/{}.zip", self.download_url_base, dataset_name);
        let zip_path = format!("{}/{}.zip", self.cache_dir, dataset_name);

        // Download the dataset
        info!("Downloading from: {}", download_url);
        let response = reqwest::get(&download_url).await?;

        if !response.status().is_success() {
            return Err(anyhow::anyhow!(
                "Failed to download dataset {}: HTTP {}",
                dataset_name,
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

        let mut file = std::fs::File::create(&zip_path)?;
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

        // Extract the dataset
        info!("Extracting dataset: {}", dataset_name);
        std::process::Command::new("unzip")
            .args(["-q", &zip_path, "-d", &self.cache_dir])
            .status()?;

        // Clean up zip file
        fs::remove_file(&zip_path)?;

        info!("Successfully downloaded and extracted: {}", dataset_name);
        Ok(())
    }

    async fn load_single_dataset(&self, dataset_name: &str) -> Result<Dataset> {
        let dataset_dir = format!("{}/{}", self.cache_dir, dataset_name);

        if !Path::new(&dataset_dir).exists() {
            return Err(anyhow::anyhow!(
                "Dataset {} not found. Please download it first.",
                dataset_name
            ));
        }

        info!("Loading BEIR dataset: {}", dataset_name);

        // Load queries
        let queries_path = format!("{}/queries.jsonl", dataset_dir);
        let queries = self.load_queries(&queries_path)?;

        // Load corpus (documents)
        let corpus_path = format!("{}/corpus.jsonl", dataset_dir);
        let documents = self.load_corpus(&corpus_path)?;

        // Load relevance judgments (qrels)
        let qrels_path = format!("{}/qrels/test.tsv", dataset_dir);
        let qrels = self.load_qrels(&qrels_path)?;

        // Combine queries with their relevant documents
        let queries_with_rels = self.combine_queries_and_qrels(queries, qrels)?;

        Ok(Dataset {
            name: dataset_name.to_string(),
            queries: queries_with_rels,
            documents,
        })
    }

    fn load_queries(&self, path: &str) -> Result<HashMap<String, String>> {
        let file = File::open(path)?;
        let reader = BufReader::new(file);
        let mut queries = HashMap::new();

        for line in reader.lines() {
            let line = line?;
            if line.trim().is_empty() {
                continue;
            }

            let query_data: serde_json::Value = serde_json::from_str(&line)?;
            if let (Some(id), Some(text)) =
                (query_data["_id"].as_str(), query_data["text"].as_str())
            {
                queries.insert(id.to_string(), text.to_string());
            }
        }

        info!("Loaded {} queries", queries.len());
        Ok(queries)
    }

    fn load_corpus(&self, path: &str) -> Result<Vec<Document>> {
        let file = File::open(path)?;
        let reader = BufReader::new(file);
        let mut documents = Vec::new();

        for line in reader.lines() {
            let line = line?;
            if line.trim().is_empty() {
                continue;
            }

            let doc_data: serde_json::Value = serde_json::from_str(&line)?;
            if let (Some(id), Some(title), Some(text)) = (
                doc_data["_id"].as_str(),
                doc_data["title"].as_str(),
                doc_data["text"].as_str(),
            ) {
                let mut metadata = HashMap::new();
                if let Some(metadata_obj) = doc_data["metadata"].as_object() {
                    for (k, v) in metadata_obj {
                        if let Some(v_str) = v.as_str() {
                            metadata.insert(k.clone(), v_str.to_string());
                        }
                    }
                }

                documents.push(Document {
                    id: id.to_string(),
                    title: title.to_string(),
                    content: text.to_string(),
                    metadata,
                });
            }
        }

        info!("Loaded {} documents", documents.len());
        Ok(documents)
    }

    fn load_qrels(&self, path: &str) -> Result<HashMap<String, Vec<(String, f64)>>> {
        let file = File::open(path)?;
        let reader = BufReader::new(file);
        let mut qrels = HashMap::new();

        for line in reader.lines() {
            let line = line?;
            if line.trim().is_empty() {
                continue;
            }

            let parts: Vec<&str> = line.split('\t').collect();
            if parts.len() >= 3 {
                let query_id = parts[0].to_string();
                let doc_id = parts[1].to_string();
                let relevance: f64 = parts[2].parse().unwrap_or(0.0);

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

    fn stream_corpus(&self, path: &str) -> Pin<Box<dyn Stream<Item = Result<Document>> + Send>> {
        let path = path.to_string();
        Box::pin(stream::try_unfold(
            (path, None::<BufReader<File>>),
            move |(path, mut reader_opt)| async move {
                // Initialize reader if needed
                if reader_opt.is_none() {
                    let file = File::open(&path).map_err(|e| {
                        anyhow::anyhow!("Failed to open corpus file {}: {}", path, e)
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

                            if let (Some(id), Some(title), Some(text)) = (
                                doc_data["_id"].as_str(),
                                doc_data["title"].as_str(),
                                doc_data["text"].as_str(),
                            ) {
                                let mut metadata = HashMap::new();
                                if let Some(metadata_obj) = doc_data["metadata"].as_object() {
                                    for (k, v) in metadata_obj {
                                        if let Some(v_str) = v.as_str() {
                                            metadata.insert(k.clone(), v_str.to_string());
                                        }
                                    }
                                }

                                let document = Document {
                                    id: id.to_string(),
                                    title: title.to_string(),
                                    content: text.to_string(),
                                    metadata,
                                };

                                return Ok(Some((document, (path, reader_opt))));
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
        queries_path: &str,
        qrels_path: &str,
    ) -> Pin<Box<dyn Stream<Item = Result<Query>> + Send>> {
        let queries_path = queries_path.to_string();
        let qrels_path = qrels_path.to_string();

        // Load qrels outside the closure to avoid lifetime issues
        let qrels = match self.load_qrels(&qrels_path) {
            Ok(qrels) => qrels,
            Err(e) => {
                return Box::pin(stream::once(async move {
                    Err(anyhow::anyhow!("Failed to load qrels: {}", e))
                }))
            }
        };

        Box::pin(stream::try_unfold(
            (queries_path, qrels, None::<BufReader<File>>),
            move |(queries_path, qrels, mut queries_reader_opt)| async move {
                // Initialize queries reader if needed
                if queries_reader_opt.is_none() {
                    let queries_file = File::open(&queries_path)?;
                    let queries_reader = BufReader::new(queries_file);
                    queries_reader_opt = Some(queries_reader);
                }

                let queries_reader = queries_reader_opt.as_mut().unwrap();
                let mut query_line = String::new();

                loop {
                    query_line.clear();
                    match queries_reader.read_line(&mut query_line) {
                        Ok(0) => return Ok(None), // EOF
                        Ok(_) => {
                            if query_line.trim().is_empty() {
                                continue;
                            }

                            let query_data: serde_json::Value =
                                match serde_json::from_str(&query_line) {
                                    Ok(data) => data,
                                    Err(e) => {
                                        tracing::warn!("Failed to parse query JSON line: {}", e);
                                        continue;
                                    }
                                };

                            if let (Some(query_id), Some(query_text)) =
                                (query_data["_id"].as_str(), query_data["text"].as_str())
                            {
                                // Look up relevance judgments for this query
                                let relevant_docs: Vec<RelevantDoc> = qrels
                                    .get(query_id)
                                    .map(|rels| {
                                        rels.iter()
                                            .map(|(doc_id, relevance)| RelevantDoc {
                                                doc_id: doc_id.clone(),
                                                relevance_score: *relevance,
                                            })
                                            .collect()
                                    })
                                    .unwrap_or_default();

                                // Only return queries that have relevant documents
                                if !relevant_docs.is_empty() {
                                    let query = Query {
                                        id: query_id.to_string(),
                                        text: query_text.to_string(),
                                        relevant_docs,
                                    };
                                    return Ok(Some((
                                        query,
                                        (queries_path, qrels, queries_reader_opt),
                                    )));
                                }
                            }
                        }
                        Err(e) => {
                            return Err(anyhow::anyhow!("Failed to read queries file: {}", e))
                        }
                    }
                }
            },
        ))
    }
}

#[async_trait]
impl DatasetLoader for BeirDataset {
    async fn download(&self) -> Result<()> {
        self.download_all().await
    }

    async fn load_dataset(&self) -> Result<Dataset> {
        if let Some(dataset_name) = self.get_dataset_to_use() {
            let dataset_dir = format!("{}/{}", self.cache_dir, dataset_name);
            if Path::new(&dataset_dir).exists() {
                return self.load_single_dataset(dataset_name).await;
            } else {
                return Err(anyhow::anyhow!(
                    "Selected BEIR dataset '{}' not found. Please download first.",
                    dataset_name
                ));
            }
        }

        Err(anyhow::anyhow!(
            "No BEIR dataset configured or available. Please download first."
        ))
    }

    fn get_name(&self) -> String {
        "BEIR".to_string()
    }

    fn get_cache_dir(&self) -> String {
        self.cache_dir.clone()
    }

    fn stream_documents(&self) -> Pin<Box<dyn Stream<Item = Result<Document>> + Send>> {
        if let Some(dataset_name) = self.get_dataset_to_use() {
            let dataset_dir = format!("{}/{}", self.cache_dir, dataset_name);
            if Path::new(&dataset_dir).exists() {
                let corpus_path = format!("{}/corpus.jsonl", dataset_dir);
                return self.stream_corpus(&corpus_path);
            }
        }

        // If no dataset found, return empty stream
        Box::pin(stream::empty())
    }

    fn stream_queries(&self) -> Pin<Box<dyn Stream<Item = Result<Query>> + Send>> {
        if let Some(dataset_name) = self.get_dataset_to_use() {
            let dataset_dir = format!("{}/{}", self.cache_dir, dataset_name);
            if Path::new(&dataset_dir).exists() {
                let queries_path = format!("{}/queries.jsonl", dataset_dir);
                let qrels_path = format!("{}/qrels/test.tsv", dataset_dir);
                return self.stream_corpus_queries(&queries_path, &qrels_path);
            }
        }

        // If no dataset found, return empty stream
        Box::pin(stream::empty())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[tokio::test]
    async fn test_beir_dataset_creation() {
        let temp_dir = TempDir::new().unwrap();
        let cache_dir = temp_dir.path().to_str().unwrap().to_string();

        let dataset =
            BeirDataset::new(cache_dir.clone()).with_datasets(vec!["nfcorpus".to_string()]);

        assert_eq!(dataset.get_name(), "BEIR");
        assert_eq!(dataset.get_cache_dir(), cache_dir);
    }
}

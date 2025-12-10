use anyhow::{Context, Result};
use flate2::read::GzDecoder;
use indicatif::{ProgressBar, ProgressStyle};
use scraper::Html;
use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use std::fs::{self, File};
use std::io::{BufRead, BufReader, BufWriter, Write};
use std::path::{Path, PathBuf};
use tracing::info;

#[derive(Debug, Deserialize)]
struct NqRecord {
    document_url: Option<String>,
    document_html: Option<String>,
    document_title: Option<String>,
    question_text: Option<String>,
}

#[derive(Debug, Serialize)]
struct OutputDocument {
    id: String,
    title: String,
    text: String,
}

#[derive(Debug, Serialize)]
struct OutputQuery {
    id: String,
    text: String,
    relevant_doc_id: Option<String>,
}

#[derive(Debug, Serialize)]
struct Metadata {
    total_documents: usize,
    total_queries: usize,
    total_text_bytes: usize,
    avg_document_length_bytes: usize,
    queries_with_relevant_docs: usize,
}

fn html_to_text(html: &str) -> String {
    let document = Html::parse_document(html);

    let mut text_parts = Vec::new();

    for text_node in document.root_element().text() {
        let trimmed = text_node.trim();
        if !trimmed.is_empty() {
            text_parts.push(trimmed);
        }
    }

    let text = text_parts.join(" ");

    // Clean up whitespace
    let text = text.split_whitespace().collect::<Vec<_>>().join(" ");

    text
}

fn generate_doc_id(url: &str) -> String {
    use std::collections::hash_map::DefaultHasher;
    use std::hash::{Hash, Hasher};

    let mut hasher = DefaultHasher::new();
    url.hash(&mut hasher);
    format!("{:016x}", hasher.finish())
}

fn find_nq_files(input_dir: &Path) -> Vec<PathBuf> {
    let mut files = Vec::new();

    // Look for train files
    let train_dir = input_dir.join("train");
    if train_dir.exists() {
        if let Ok(entries) = fs::read_dir(&train_dir) {
            for entry in entries.flatten() {
                let path = entry.path();
                if path.extension().map_or(false, |e| e == "gz") {
                    files.push(path);
                }
            }
        }
    }

    // Look for dev files
    let dev_dir = input_dir.join("dev");
    if dev_dir.exists() {
        if let Ok(entries) = fs::read_dir(&dev_dir) {
            for entry in entries.flatten() {
                let path = entry.path();
                if path.extension().map_or(false, |e| e == "gz") {
                    files.push(path);
                }
            }
        }
    }

    files.sort();
    files
}

pub fn prepare_nq_data(
    input_dir: &Path,
    output_dir: &Path,
    max_documents: Option<usize>,
    max_queries: Option<usize>,
) -> Result<()> {
    info!("Input directory: {:?}", input_dir);
    info!("Output directory: {:?}", output_dir);
    if let Some(max) = max_documents {
        info!("Max documents: {}", max);
    }

    // Create output directory
    fs::create_dir_all(output_dir)?;

    // Find all NQ files
    let files = find_nq_files(input_dir);
    if files.is_empty() {
        anyhow::bail!("No NQ files found in {:?}", input_dir);
    }

    info!("Found {} NQ files to process", files.len());

    // Open output files for streaming writes - this is the key to avoiding memory bloat
    let corpus_path = output_dir.join("corpus.jsonl");
    let queries_path = output_dir.join("queries.jsonl");
    let mut corpus_file = BufWriter::new(File::create(&corpus_path)?);
    let mut queries_file = BufWriter::new(File::create(&queries_path)?);

    // Track seen URLs and questions (only store small strings, not full documents)
    let mut seen_urls: HashSet<String> = HashSet::new();
    let mut seen_questions: HashSet<String> = HashSet::new();
    // Map URL to doc_id for query->doc linking (much smaller than storing full docs)
    let mut url_to_doc_id: std::collections::HashMap<String, String> =
        std::collections::HashMap::new();

    let mut doc_count = 0usize;
    let mut query_count = 0usize;
    let mut total_text_bytes = 0usize;
    let mut queries_with_relevant = 0usize;
    let max_docs = max_documents.unwrap_or(usize::MAX);
    let max_qs = max_queries.unwrap_or(usize::MAX);

    let progress = ProgressBar::new(files.len() as u64);
    progress.set_style(
        ProgressStyle::default_bar()
            .template("{spinner:.green} [{elapsed_precise}] [{bar:40.cyan/blue}] {pos}/{len} files ({msg})")
            .unwrap()
            .progress_chars("#>-"),
    );

    // Process files sequentially
    for file_path in &files {
        // Check if we've hit document limit
        if doc_count >= max_docs {
            progress.println(format!("Reached max documents ({}), stopping", max_docs));
            break;
        }

        progress.set_message(format!(
            "Processing {:?}",
            file_path.file_name().unwrap_or_default()
        ));

        let file =
            File::open(file_path).with_context(|| format!("Failed to open {:?}", file_path))?;
        let decoder = GzDecoder::new(file);
        let reader = BufReader::with_capacity(256 * 1024, decoder);

        // Stream records one at a time
        for line in reader.lines() {
            let line = match line {
                Ok(l) => l,
                Err(_) => continue,
            };

            let record: NqRecord = match serde_json::from_str(&line) {
                Ok(r) => r,
                Err(_) => continue,
            };

            // Check early termination
            if doc_count >= max_docs {
                break;
            }

            let doc_url = match &record.document_url {
                Some(url) if !url.is_empty() => url.clone(),
                _ => continue,
            };

            let doc_id = generate_doc_id(&doc_url);

            // Process document if we haven't seen it - write directly to disk
            if !seen_urls.contains(&doc_url) && doc_count < max_docs {
                if let Some(html) = &record.document_html {
                    let text = html_to_text(html);

                    // Skip very short documents
                    if text.len() >= 100 {
                        let title = record.document_title.clone().unwrap_or_default();

                        // Write document directly to file (no memory accumulation)
                        let doc = OutputDocument {
                            id: doc_id.clone(),
                            title,
                            text: text.clone(),
                        };
                        serde_json::to_writer(&mut corpus_file, &doc)?;
                        corpus_file.write_all(b"\n")?;

                        total_text_bytes += text.len();
                        seen_urls.insert(doc_url.clone());
                        url_to_doc_id.insert(doc_url.clone(), doc_id.clone());
                        doc_count += 1;

                        if doc_count % 100 == 0 {
                            progress.set_message(format!(
                                "{} docs, {} queries",
                                doc_count, query_count
                            ));
                            // Flush periodically to avoid buffering too much
                            corpus_file.flush()?;
                        }
                    }
                }
            }

            // Process query - write directly to disk
            if let Some(question) = &record.question_text {
                if !question.is_empty()
                    && query_count < max_qs
                    && !seen_questions.contains(question)
                {
                    seen_questions.insert(question.clone());

                    let relevant_doc_id = url_to_doc_id.get(&doc_url).cloned();
                    if relevant_doc_id.is_some() {
                        queries_with_relevant += 1;
                    }

                    let query = OutputQuery {
                        id: format!("q{:08}", query_count),
                        text: question.clone(),
                        relevant_doc_id,
                    };
                    serde_json::to_writer(&mut queries_file, &query)?;
                    queries_file.write_all(b"\n")?;

                    query_count += 1;

                    if query_count % 1000 == 0 {
                        queries_file.flush()?;
                    }
                }
            }
        }

        progress.inc(1);
    }

    progress.finish_with_message("Processing complete");

    // Final flush
    corpus_file.flush()?;
    queries_file.flush()?;

    info!("Extracted {} unique documents", doc_count);
    info!("Extracted {} unique queries", query_count);
    info!("  Written: {:?}", corpus_path);
    info!("  Written: {:?}", queries_path);

    // Write metadata
    let metadata = Metadata {
        total_documents: doc_count,
        total_queries: query_count,
        total_text_bytes,
        avg_document_length_bytes: if doc_count == 0 {
            0
        } else {
            total_text_bytes / doc_count
        },
        queries_with_relevant_docs: queries_with_relevant,
    };

    let metadata_path = output_dir.join("metadata.json");
    let metadata_file = File::create(&metadata_path)?;
    serde_json::to_writer_pretty(metadata_file, &metadata)?;
    info!("  Written: {:?}", metadata_path);

    // Print summary
    info!("");
    info!("=== Summary ===");
    info!("  total_documents: {}", metadata.total_documents);
    info!("  total_queries: {}", metadata.total_queries);
    info!("  total_text_bytes: {}", metadata.total_text_bytes);
    info!(
        "  avg_document_length_bytes: {}",
        metadata.avg_document_length_bytes
    );
    info!(
        "  queries_with_relevant_docs: {}",
        metadata.queries_with_relevant_docs
    );

    Ok(())
}

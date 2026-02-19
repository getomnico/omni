use crate::models::{
    RecentSearchesResponse, SearchMode, SearchRequest, SearchResponse, SearchResult,
};
use anyhow::Result;
use redis::{AsyncCommands, Client as RedisClient};
use shared::db::repositories::{DocumentRepository, EmbeddingRepository};
use shared::models::{ChunkResult, Document};
use shared::utils::safe_str_slice;
use shared::{
    AIClient, DatabasePool, ObjectStorage, Repository, SearcherConfig, StorageFactory,
    UserRepository,
};
use std::cmp::Ordering;
use std::collections::hash_map::DefaultHasher;
use std::collections::HashMap;
use std::hash::{Hash, Hasher};
use std::sync::Arc;
use std::time::{Duration, Instant};
use tracing::{debug, error, info};

pub struct SearchEngine {
    db_pool: DatabasePool,
    redis_client: RedisClient,
    ai_client: AIClient,
    content_storage: Arc<dyn ObjectStorage>,
    config: SearcherConfig,
}

impl SearchEngine {
    const CONTENT_SIZE_THRESHOLD: usize = 50_000; // 50KB threshold

    pub async fn new(
        db_pool: DatabasePool,
        redis_client: RedisClient,
        ai_client: AIClient,
        config: SearcherConfig,
    ) -> Result<Self> {
        let content_storage = StorageFactory::from_env(db_pool.pool().clone()).await?;
        Ok(Self {
            db_pool,
            redis_client,
            ai_client,
            content_storage,
            config,
        })
    }

    fn prepare_document_for_response(&self, mut doc: Document) -> Document {
        doc.content_id = None;

        // Append metadata hash to URL for frontend icon resolution
        if let Some(url_str) = &doc.url {
            if doc.content_type.is_some() {
                let mut metadata_parts = Vec::new();
                // Note: source_type would go here if we had it
                // For now, we only add content_type
                if let Some(ref ct) = doc.content_type {
                    metadata_parts.push(ct.clone());
                }
                if !metadata_parts.is_empty() {
                    // Parse URL and update/replace hash fragment
                    if let Ok(mut parsed_url) = url::Url::parse(url_str) {
                        parsed_url
                            .set_fragment(Some(&format!("meta={}", metadata_parts.join(","))));
                        doc.url = Some(parsed_url.to_string());
                    } else {
                        // Fallback for unparseable URLs (shouldn't happen, but be defensive)
                        doc.url = Some(format!("{}#meta={}", url_str, metadata_parts.join(",")));
                    }
                }
            }
        }

        doc
    }

    pub async fn search(&self, request: SearchRequest) -> Result<SearchResponse> {
        let start_time = Instant::now();

        info!(
            "Searching for query: '{}', mode: {:?}",
            request.query,
            request.search_mode()
        );

        // In case the request contains only user_id, populate user_email for permission filtering
        let user_repo = UserRepository::new(self.db_pool.pool());
        let request = match (&request.user_id, &request.user_email) {
            (Some(user_id), None) => {
                info!("Search request has user_id but no email, fetching email from DB for user ID: {}", user_id);
                let res = user_repo.find_by_id(user_id.clone()).await;
                info!("Fetched user: {:?}", res);
                if let Ok(Some(user)) = res {
                    info!(
                        "Fetched user email: {} for user ID: {}",
                        user.email, user_id
                    );
                    let mut new_request = request.clone();
                    new_request.user_email = Some(user.email);
                    new_request
                } else {
                    info!("Failed to fetch user email for user ID: {}", user_id);
                    request
                }
            }
            _ => request,
        };

        // Handle document_id filter for read_document tool
        if let Some(document_id) = &request.document_id {
            info!("Document ID filter detected: {}", document_id);
            return self.read_document_by_id(document_id, &request).await;
        }

        // Generate cache key based on request parameters
        let cache_key = self.generate_cache_key(&request);

        // Try to get from cache first
        if let Ok(mut conn) = self.redis_client.get_multiplexed_async_connection().await {
            if let Ok(cached_response) = conn.get::<_, String>(&cache_key).await {
                if let Ok(response) = serde_json::from_str::<SearchResponse>(&cached_response) {
                    info!("Cache hit for query: '{}'", request.query);
                    return Ok(response);
                }
            }
        }

        let repo = DocumentRepository::new(self.db_pool.pool());
        let limit = request.limit();

        if request.query.trim().is_empty() {
            return Err(anyhow::anyhow!("Search query cannot be empty"));
        }

        let source_ids = repo
            .fetch_active_source_ids(request.source_types.as_deref())
            .await?;

        let search_future = async {
            let start_ts = Instant::now();
            let res = match request.search_mode() {
                SearchMode::Fulltext => self.fulltext_search(&repo, &request, &source_ids).await,
                SearchMode::Semantic => self.semantic_search(&request).await,
                SearchMode::Hybrid => self.hybrid_search(&request).await,
            };

            debug!("Search future completed in: {:?}", start_ts.elapsed());
            res
        };

        let facets_future = async {
            if request.include_facets() {
                let start_ts = Instant::now();
                let content_types = request.content_types.as_deref();
                let attribute_filters = request.attribute_filters.as_ref();
                let facets = repo
                    .get_facet_counts(
                        &request.query,
                        &source_ids,
                        content_types,
                        attribute_filters,
                        request.user_email().map(|e| e.as_str()),
                    )
                    .await
                    .unwrap_or_else(|e| {
                        info!("Failed to get facet counts: {}", e);
                        vec![]
                    });

                debug!("Facets fetched in {:?}", start_ts.elapsed());
                facets
            } else {
                debug!("Facets not requested, returning empty array.");
                vec![]
            }
        };

        let (search_result, facets) = tokio::join!(search_future, facets_future);
        let results = search_result?;
        let total_count = results.len() as i64;
        let has_more = results.len() as i64 >= limit;
        let query_time = start_time.elapsed().as_millis() as u64;

        info!(
            "Search completed in {}ms, found {} results",
            query_time,
            results.len()
        );

        let response = SearchResponse {
            results,
            total_count,
            query_time_ms: query_time,
            has_more,
            query: request.query.clone(),
            facets: if facets.is_empty() {
                None
            } else {
                Some(facets)
            },
        };

        // Cache the response for 5 minutes
        if let Ok(mut conn) = self.redis_client.get_multiplexed_async_connection().await {
            if let Ok(response_json) = serde_json::to_string(&response) {
                let _: Result<(), _> = conn.set_ex(&cache_key, response_json, 300).await;
            }
        }

        Ok(response)
    }

    async fn fulltext_search(
        &self,
        repo: &DocumentRepository,
        request: &SearchRequest,
        source_ids: &[String],
    ) -> Result<Vec<SearchResult>> {
        let start_time = Instant::now();
        let content_types = request.content_types.as_deref();
        let attribute_filters = request.attribute_filters.as_ref();

        debug!("Running fulltext search for {}", &request.query);
        let search_hits = repo
            .search(
                &request.query,
                source_ids,
                content_types,
                attribute_filters,
                request.limit(),
                request.offset(),
                request.user_email().map(|e| e.as_str()),
                request.document_id.as_deref(),
            )
            .await?;

        let mut results = Vec::new();

        for search_hit in search_hits {
            let doc = search_hit.document;
            debug!(
                "[FTS] Document {} [id={}] score={}",
                doc.title, doc.id, search_hit.score,
            );
            let prepared_doc = self.prepare_document_for_response(doc);

            let highlights = search_hit
                .content_snippets
                .unwrap_or_default()
                .into_iter()
                .filter(|s| !s.is_empty())
                .collect::<Vec<String>>();

            results.push(SearchResult {
                document: prepared_doc,
                score: search_hit.score as f32,
                highlights,
                match_type: "fulltext".to_string(),
                content: None,
            });
        }

        info!(
            "Fulltext search completed in {}ms",
            start_time.elapsed().as_millis()
        );
        Ok(results)
    }

    async fn semantic_search(&self, request: &SearchRequest) -> Result<Vec<SearchResult>> {
        let start_time = Instant::now();
        info!("Performing semantic search for query: '{}'", request.query);

        let query_embedding = self.generate_query_embedding(&request.query).await?;

        let embedding_repo = EmbeddingRepository::new(self.db_pool.pool());
        let doc_repo = DocumentRepository::new(self.db_pool.pool());

        let sources = request.source_types.as_deref();
        let content_types = request.content_types.as_deref();

        let chunk_results = embedding_repo
            .find_similar_with_filters(
                query_embedding,
                sources,
                content_types,
                request.limit(),
                request.offset(),
                request.user_email().map(|e| e.as_str()),
                request.document_id.as_deref(),
            )
            .await?;

        // Get unique document IDs and batch fetch documents
        let document_ids: Vec<String> = chunk_results
            .iter()
            .map(|chunk| chunk.document_id.clone())
            .collect::<std::collections::HashSet<_>>()
            .into_iter()
            .collect();

        let documents = doc_repo.find_by_ids(&document_ids).await?;
        let documents_map: HashMap<String, _> = documents
            .into_iter()
            .map(|doc| (doc.id.clone(), doc))
            .collect();

        // Group chunks by document_id to collect all matching chunks per document
        let mut document_chunks: HashMap<String, Vec<&ChunkResult>> = HashMap::new();
        for chunk_result in &chunk_results {
            document_chunks
                .entry(chunk_result.document_id.clone())
                .or_insert_with(Vec::new)
                .push(chunk_result);
        }

        let mut results = Vec::new();
        for (document_id, chunks) in document_chunks {
            if let Some(doc) = documents_map.get(&document_id) {
                // Use the highest scoring chunk as the document score
                let max_score = chunks
                    .iter()
                    .map(|chunk| chunk.similarity_score)
                    .fold(f32::NEG_INFINITY, f32::max);

                // Fetch document content and extract chunk text using offsets
                let mut chunk_highlights: Vec<(f32, String)> = Vec::new();
                if let Some(content_id) = &doc.content_id {
                    if let Ok(content) = self.content_storage.get_text(content_id).await {
                        for chunk in chunks {
                            let chunk_text = self.extract_chunk_from_content(
                                &content,
                                chunk.chunk_start_offset,
                                chunk.chunk_end_offset,
                            );
                            chunk_highlights
                                .push((chunk.similarity_score, chunk_text.trim().to_string()));
                        }
                    }
                }

                // Sort by similarity score (highest first)
                chunk_highlights
                    .sort_by(|a, b| b.0.partial_cmp(&a.0).unwrap_or(std::cmp::Ordering::Equal));

                // Extract just the snippets in sorted order, limited to top 5
                let all_highlights: Vec<String> = chunk_highlights
                    .into_iter()
                    .take(5)
                    .map(|(_, snippet)| snippet)
                    .collect();

                let prepared_doc = self.prepare_document_for_response(doc.clone());
                results.push(SearchResult {
                    document: prepared_doc,
                    score: max_score,
                    highlights: all_highlights,
                    match_type: "semantic".to_string(),
                    content: None, // Using highlights instead of single content snippet
                });
            }
        }

        // Sort results by score in descending order
        results.sort_by(|a, b| {
            b.score
                .partial_cmp(&a.score)
                .unwrap_or(std::cmp::Ordering::Equal)
        });

        info!(
            "Semantic search completed in {}ms",
            start_time.elapsed().as_millis()
        );
        Ok(results)
    }

    fn extract_chunk_from_content(
        &self,
        content: &str,
        start_offset: i32,
        end_offset: i32,
    ) -> String {
        let start = start_offset as usize;
        let end = end_offset as usize;

        if start >= content.len() || end > content.len() || start >= end {
            return String::new();
        }

        safe_str_slice(content, start, end).to_string()
    }

    /// Read a specific document by ID, returning full content for small documents
    /// or relevant chunks for large documents
    async fn read_document_by_id(
        &self,
        document_id: &str,
        request: &SearchRequest,
    ) -> Result<SearchResponse> {
        let start_time = Instant::now();
        info!("Reading document by ID: {}", document_id);

        let doc_repo = DocumentRepository::new(self.db_pool.pool());
        let doc = doc_repo
            .find_by_id(document_id)
            .await?
            .ok_or_else(|| anyhow::anyhow!("Document not found: {}", document_id))?;

        // Get actual content size (extracted text, not original file)
        let results = if let Some(content_id) = &doc.content_id {
            match self.content_storage.get_text(content_id).await {
                Ok(content) => {
                    let content_size = content.len();
                    if content_size < Self::CONTENT_SIZE_THRESHOLD {
                        // Small document: return full content
                        info!(
                            "Document content is small ({}B), returning full content",
                            content_size
                        );
                        vec![SearchResult {
                            document: self.prepare_document_for_response(doc.clone()),
                            score: 1.0,
                            highlights: vec![content],
                            match_type: "full_content".to_string(),
                            content: None,
                        }]
                    } else {
                        // Check if specific line range is requested
                        match (
                            request.document_content_start_line,
                            request.document_content_end_line,
                        ) {
                            (Some(start_line), Some(end_line)) => {
                                debug!("Start ({}) and end ({}) line numbers are specified, returning these specific lines.", start_line, end_line);

                                // Validate line numbers (1-indexed from user perspective)
                                if start_line < 1 || end_line < start_line {
                                    return Err(anyhow::anyhow!(
                                        "Invalid line range: start={}, end={}",
                                        start_line,
                                        end_line
                                    ));
                                }

                                let mut selected_lines = Vec::new();
                                let start_idx = start_line as usize;
                                let end_idx = end_line as usize;

                                for (line_num, line) in content.lines().enumerate() {
                                    let current_line = line_num + 1; // Convert to 1-indexed

                                    if current_line >= start_idx && current_line <= end_idx {
                                        // Prefix each line with its line number
                                        selected_lines.push(format!("{} | {}", current_line, line));
                                    }

                                    // Stop iteration once we've passed the end line
                                    if current_line > end_idx {
                                        break;
                                    }
                                }

                                if selected_lines.is_empty() {
                                    return Err(anyhow::anyhow!(
                                        "Line range {}-{} is out of bounds for document",
                                        start_line,
                                        end_line
                                    ));
                                }

                                let selected_content = selected_lines.join("\n");

                                info!(
                                    "Returning lines {}-{} from document ({} lines, {}B)",
                                    start_line,
                                    start_line + (selected_lines.len() as u32) - 1,
                                    selected_lines.len(),
                                    selected_content.len()
                                );

                                vec![SearchResult {
                                    document: self.prepare_document_for_response(doc.clone()),
                                    score: 1.0,
                                    highlights: vec![selected_content],
                                    match_type: "line_range".to_string(),
                                    content: None,
                                }]
                            }
                            _ => {
                                info!(
                                    "Document content is large ({}B), using chunk-based retrieval",
                                    content_size
                                );
                                self.read_document_chunks(document_id, &doc, request)
                                    .await?
                            }
                        }
                    }
                }
                Err(e) => {
                    info!(
                        "Failed to read document content: {}, falling back to chunk retrieval",
                        e
                    );
                    self.read_document_chunks(document_id, &doc, request)
                        .await?
                }
            }
        } else {
            info!("No content_id available for document");
            vec![]
        };

        let total_count = results.len() as i64;
        let query_time = start_time.elapsed().as_millis() as u64;

        info!(
            "Read document completed in {}ms, returned {} chunks",
            query_time,
            results.len()
        );

        Ok(SearchResponse {
            results,
            total_count,
            query_time_ms: query_time,
            has_more: false,
            query: request.query.clone(),
            facets: None,
        })
    }

    /// Read document chunks, optionally filtered by query for semantic search
    async fn read_document_chunks(
        &self,
        document_id: &str,
        doc: &shared::models::Document,
        request: &SearchRequest,
    ) -> Result<Vec<SearchResult>> {
        let results = if !request.query.trim().is_empty() {
            // Query provided: do hybrid search within document
            info!("Query provided, hybrid search within document");
            self.hybrid_search(request).await?
        } else {
            info!(
                "No query provided, returning first 500 lines from document ID {}",
                document_id
            );
            if let Some(content_id) = &doc.content_id {
                let content = self.content_storage.get_text(content_id).await?;

                // Take first 500 lines and prefix with line numbers
                let prefixed_content: String = content
                    .lines()
                    .take(500)
                    .enumerate()
                    .map(|(idx, line)| format!("{} | {}", idx + 1, line))
                    .collect::<Vec<_>>()
                    .join("\n");

                // Apply character limit after line prefixing
                let truncated: String = prefixed_content
                    .chars()
                    .take(Self::CONTENT_SIZE_THRESHOLD)
                    .collect();

                vec![SearchResult {
                    document: doc.clone(),
                    score: 1.0,
                    highlights: vec![truncated],
                    match_type: "fulltext".to_string(),
                    content: None,
                }]
            } else {
                error!(
                    "Content ID not found for document ID [{}], content ID [{:?}]",
                    document_id, doc.content_id
                );
                vec![]
            }
        };

        Ok(results)
    }

    async fn generate_query_embedding(&self, query: &str) -> Result<Vec<f32>> {
        debug!("Generating query embeddings for query '{}'", query);
        let embeddings = self
            .ai_client
            .generate_embeddings_with_options(
                vec![query.to_string()],
                Some("query".to_string()),
                None,
                Some("none".to_string()),
                Some("high".to_string()), // High priority for search queries
            )
            .await?;
        if let Some(first_embedding) = embeddings.first() {
            if let Some(first_chunk) = first_embedding.chunk_embeddings.first() {
                return Ok(first_chunk.clone());
            }
        }
        Err(anyhow::anyhow!("Failed to generate embedding for query"))
    }

    /// Get semantic search results enhanced with expanded context for RAG
    async fn get_enhanced_semantic_results_for_rag(
        &self,
        request: &SearchRequest,
    ) -> Result<Vec<SearchResult>> {
        let start_time = Instant::now();
        info!(
            "Generating enhanced semantic search results for RAG query: '{}'",
            request.query
        );

        let query_embedding = self.generate_query_embedding(&request.query).await?;
        let embedding_repo = EmbeddingRepository::new(self.db_pool.pool());
        let doc_repo = DocumentRepository::new(self.db_pool.pool());

        let sources = request.source_types.as_deref();
        let content_types = request.content_types.as_deref();

        // Get chunk results with indices
        let chunk_results = embedding_repo
            .find_similar_with_filters(
                query_embedding,
                sources,
                content_types,
                request.limit(),
                request.offset(),
                request.user_email().map(|e| e.as_str()),
                None,
            )
            .await?;

        // Group chunks by document_id
        let mut document_chunks: HashMap<String, Vec<&ChunkResult>> = HashMap::new();
        for chunk_result in &chunk_results {
            document_chunks
                .entry(chunk_result.document_id.clone())
                .or_insert_with(Vec::new)
                .push(chunk_result);
        }

        // Get documents
        let document_ids: Vec<String> = document_chunks.keys().cloned().collect();
        let documents = doc_repo.find_by_ids(&document_ids).await?;
        let documents_map: HashMap<String, _> = documents
            .into_iter()
            .map(|doc| (doc.id.clone(), doc))
            .collect();

        let mut results = Vec::new();

        for (document_id, chunks) in document_chunks {
            if let Some(doc) = documents_map.get(&document_id) {
                let max_score = chunks
                    .iter()
                    .map(|chunk| chunk.similarity_score)
                    .fold(f32::NEG_INFINITY, f32::max);

                // Extract chunk indices for this document
                let chunk_indices: Vec<i32> = chunks.iter().map(|c| c.chunk_index).collect();

                // Fetch expanded context using surrounding chunks
                let expanded_chunks = embedding_repo
                    .find_surrounding_chunks_for_document(
                        &document_id,
                        &chunk_indices,
                        self.config.rag_context_window,
                    )
                    .await?;

                // Combine expanded chunks into continuous text
                let expanded_context = if let Some(content_id) = &doc.content_id {
                    if let Ok(content) = self.content_storage.get_text(content_id).await {
                        let mut chunk_texts = Vec::new();
                        for chunk in &expanded_chunks {
                            let chunk_text = self.extract_chunk_from_content(
                                &content,
                                chunk.chunk_start_offset,
                                chunk.chunk_end_offset,
                            );
                            if !chunk_text.trim().is_empty() {
                                chunk_texts.push(chunk_text.trim().to_string());
                            }
                        }
                        chunk_texts.join(" ")
                    } else {
                        String::new()
                    }
                } else {
                    String::new()
                };

                let prepared_doc = self.prepare_document_for_response(doc.clone());
                results.push(SearchResult {
                    document: prepared_doc,
                    score: max_score,
                    highlights: if expanded_context.trim().is_empty() {
                        vec![]
                    } else {
                        vec![expanded_context]
                    },
                    match_type: "semantic".to_string(),
                    content: None,
                });
            }
        }

        // Sort results by score in descending order
        results.sort_by(|a, b| {
            b.score
                .partial_cmp(&a.score)
                .unwrap_or(std::cmp::Ordering::Equal)
        });

        info!(
            "Enhanced semantic search for RAG completed in {}ms",
            start_time.elapsed().as_millis()
        );
        Ok(results)
    }

    async fn hybrid_search(&self, request: &SearchRequest) -> Result<Vec<SearchResult>> {
        info!("Performing hybrid search for query: '{}'", request.query);
        let start_time = Instant::now();

        let repo = DocumentRepository::new(self.db_pool.pool());
        let source_ids = repo
            .fetch_active_source_ids(request.source_types.as_deref())
            .await?;
        let fts_future = self.fulltext_search(&repo, request, &source_ids);

        // Apply timeout to semantic search
        let semantic_future = tokio::time::timeout(
            Duration::from_millis(self.config.semantic_search_timeout_ms),
            self.semantic_search(request),
        );

        let (fts_results, semantic_results) = tokio::join!(fts_future, semantic_future);
        let fts_results = fts_results?;

        // Handle semantic search timeout gracefully
        let semantic_results = match semantic_results {
            Ok(Ok(results)) => results,
            Ok(Err(e)) => {
                info!("Semantic search failed: {}, falling back to FTS only", e);
                vec![]
            }
            Err(_) => {
                info!(
                    "Semantic search timed out after {}ms, falling back to FTS only",
                    self.config.semantic_search_timeout_ms
                );
                vec![]
            }
        };
        info!("Retrieved {} results from FTS", fts_results.len());
        info!(
            "Retrieved {} results from semantic search",
            semantic_results.len()
        );

        // Reciprocal Rank Fusion: score by rank position, not raw scores
        let k = self.config.rrf_k;
        let mut combined_results: HashMap<String, SearchResult> = HashMap::new();
        let mut rrf_scores: HashMap<String, f32> = HashMap::new();

        for (rank, result) in fts_results.into_iter().enumerate() {
            let doc_id = result.document.id.clone();
            let rrf_contrib = 1.0 / (k + (rank + 1) as f32);
            debug!(
                "FTS result document {} [id={}], rank={}, rrf_contrib={:.6}",
                result.document.title,
                doc_id,
                rank + 1,
                rrf_contrib
            );
            *rrf_scores.entry(doc_id.clone()).or_insert(0.0) += rrf_contrib;
            let prepared_doc = self.prepare_document_for_response(result.document);
            combined_results.insert(
                doc_id,
                SearchResult {
                    document: prepared_doc,
                    score: 0.0,
                    highlights: result.highlights,
                    match_type: "fulltext".to_string(),
                    content: result.content,
                },
            );
        }

        for (rank, result) in semantic_results.into_iter().enumerate() {
            let doc_id = result.document.id.clone();
            let rrf_contrib = 1.0 / (k + (rank + 1) as f32);
            debug!(
                "Semantic result document {} [id={}], rank={}, rrf_contrib={:.6}",
                result.document.title,
                doc_id,
                rank + 1,
                rrf_contrib
            );
            *rrf_scores.entry(doc_id.clone()).or_insert(0.0) += rrf_contrib;
            combined_results
                .entry(doc_id)
                .and_modify(|existing| {
                    existing.match_type = "hybrid".to_string();
                })
                .or_insert_with(|| {
                    let prepared_doc = self.prepare_document_for_response(result.document);
                    SearchResult {
                        document: prepared_doc,
                        score: 0.0,
                        highlights: result.highlights,
                        match_type: "semantic".to_string(),
                        content: result.content,
                    }
                });
        }

        // Apply RRF scores and sort
        let mut final_results: Vec<SearchResult> = combined_results
            .into_iter()
            .map(|(doc_id, mut result)| {
                result.score = rrf_scores[&doc_id];
                result
            })
            .collect();
        final_results.sort_by(|a, b| b.score.partial_cmp(&a.score).unwrap_or(Ordering::Equal));

        if final_results.len() > request.limit() as usize {
            final_results.truncate(request.limit() as usize);
        }

        info!(
            "Hybrid search completed in {}ms",
            start_time.elapsed().as_millis()
        );
        Ok(final_results)
    }

    fn generate_cache_key(&self, request: &SearchRequest) -> String {
        let mut hasher = DefaultHasher::new();
        request.query.hash(&mut hasher);
        request.search_mode().hash(&mut hasher);
        request.limit().hash(&mut hasher);
        request.offset().hash(&mut hasher);

        if let Some(sources) = &request.source_types {
            for source in sources {
                source.hash(&mut hasher);
            }
        }

        if let Some(content_types) = &request.content_types {
            for ct in content_types {
                ct.hash(&mut hasher);
            }
        }

        request.include_facets().hash(&mut hasher);

        if let Some(attribute_filters) = &request.attribute_filters {
            let json = serde_json::to_string(attribute_filters).unwrap_or_default();
            json.hash(&mut hasher);
        }

        if let Some(user_email) = &request.user_email {
            user_email.hash(&mut hasher);
        }

        format!("search:{:x}", hasher.finish())
    }

    /// Store search history for a user in Redis
    pub async fn store_search_history(&self, user_id: &str, query: &str) -> Result<()> {
        let trimmed_query = query.trim();
        if trimmed_query.is_empty() {
            return Ok(());
        }

        let key = format!("search_history:{}", user_id);
        let mut conn = self.redis_client.get_multiplexed_async_connection().await?;

        // Get all existing searches
        let existing_searches: Vec<String> = conn.lrange(&key, 0, -1).await.unwrap_or_default();

        // Remove any existing occurrence of this query
        let mut deduped_searches: Vec<String> = existing_searches
            .into_iter()
            .filter(|s| s != trimmed_query)
            .collect();

        // Add the new query at the beginning
        deduped_searches.insert(0, trimmed_query.to_string());

        // Keep only the latest 5
        deduped_searches.truncate(5);

        // Clear the list and repopulate with deduplicated searches
        let _: () = conn.del(&key).await?;
        if !deduped_searches.is_empty() {
            for search in deduped_searches.iter() {
                let _: () = conn.rpush(&key, search).await?;
            }

            // Set TTL to 30 days for the search history
            let _: () = conn.expire(&key, 30 * 24 * 60 * 60).await?;
        }

        debug!(
            "Stored search query '{}' for user {}",
            trimmed_query, user_id
        );

        Ok(())
    }

    /// Get recent searches for a user from Redis
    pub async fn get_recent_searches(&self, user_id: &str) -> Result<RecentSearchesResponse> {
        let key = format!("search_history:{}", user_id);
        let mut conn = self.redis_client.get_multiplexed_async_connection().await?;

        // Get all searches (up to 5 as we maintain that limit)
        let searches: Vec<String> = conn.lrange(&key, 0, -1).await.unwrap_or_default();

        debug!(
            "Retrieved {} recent searches for user {}",
            searches.len(),
            user_id
        );

        Ok(RecentSearchesResponse { searches })
    }

    /// Generate RAG context from search request using chunk-based approach with expanded context
    pub async fn get_rag_context(&self, request: &SearchRequest) -> Result<Vec<SearchResult>> {
        info!("Generating RAG context for query: '{}'", request.query);

        let repo = DocumentRepository::new(self.db_pool.pool());
        let source_ids = repo
            .fetch_active_source_ids(request.source_types.as_deref())
            .await?;
        let fts_results = self.fulltext_search(&repo, request, &source_ids).await?;

        // Get semantic search results enhanced with expanded context for RAG
        let semantic_results = self.get_enhanced_semantic_results_for_rag(request).await?;

        // Combine semantic and fulltext context
        let mut combined_results = Vec::new();

        // Add enhanced semantic search results
        for semantic_result in semantic_results {
            combined_results.push(semantic_result);
        }

        // Add context around fulltext matches
        for fts_result in fts_results.into_iter().take(5) {
            // For fulltext matches, we already have highlights generated
            combined_results.push(fts_result);
        }

        // Sort by score and take top results
        combined_results.sort_by(|a, b| {
            b.score
                .partial_cmp(&a.score)
                .unwrap_or(std::cmp::Ordering::Equal)
        });
        combined_results.truncate(10);

        info!(
            "Generated RAG context with {} chunks",
            combined_results.len()
        );
        Ok(combined_results)
    }

    /// Generate cache key for AI answers based on query only
    pub fn generate_ai_cache_key(&self, query: &str) -> String {
        use std::collections::hash_map::DefaultHasher;
        use std::hash::{Hash, Hasher};

        let mut hasher = DefaultHasher::new();
        query.trim().to_lowercase().hash(&mut hasher);
        format!("ai_answer:{:x}", hasher.finish())
    }

    /// Build RAG prompt with context chunks and citation instructions
    pub fn build_rag_prompt(&self, query: &str, context: &[SearchResult]) -> String {
        let mut prompt = String::new();

        prompt.push_str("You are Omni - an AI assistant that assists users with their queries. ");
        prompt.push_str(
            "Please provide a response to the user's question/instruction using the information from the provided context. ",
        );
        prompt.push_str(
            "When referencing information, cite it using the format [<Document Title>](<Document URL>). Return your response in markdown format. Only reference documents provided as context below, do not cite anything else. ",
        );

        prompt.push_str("Context Information:\n");
        for (i, result) in context.iter().enumerate() {
            prompt.push_str(&format!(
                "Context {}: \nTitle: \"{}\"\nURL: {}\nMatch Type: {}\n",
                i + 1,
                result.document.title,
                result.document.url.as_deref().unwrap_or("<unknown>"),
                result.match_type,
            ));

            match result.match_type.as_str() {
                "semantic" => {
                    // For semantic chunks, use the highlights if available
                    if !result.highlights.is_empty() {
                        prompt.push_str(&format!("Content: {}\n", result.highlights[0]));
                    }
                }
                "fulltext" => {
                    // For fulltext matches, use the highlights which contain context around matches
                    if !result.highlights.is_empty() {
                        prompt.push_str(&format!("Relevant excerpt: {}\n", result.highlights[0]));
                    }
                }
                _ => {
                    if let Some(_content_id) = &result.document.content_id {
                        if !result.highlights.is_empty() {
                            prompt.push_str(&format!("Content: {}\n", result.highlights[0]));
                        }
                    }
                }
            }
            prompt.push_str("\n");
        }

        prompt.push_str(&format!("Question: {}\n\n", query));

        prompt
    }
}

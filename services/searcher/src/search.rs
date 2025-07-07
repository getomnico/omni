use crate::models::{SearchMode, SearchRequest, SearchResponse, SearchResult, SuggestionsResponse};
use anyhow::Result;
use redis::{AsyncCommands, Client as RedisClient};
use shared::db::repositories::{DocumentRepository, EmbeddingRepository};
use shared::{AIClient, ContentStorage, DatabasePool, SearcherConfig};
use std::time::Instant;
use tracing::{debug, info};

pub struct SearchEngine {
    db_pool: DatabasePool,
    redis_client: RedisClient,
    ai_client: AIClient,
    content_storage: ContentStorage,
    config: SearcherConfig,
}

impl SearchEngine {
    pub fn new(
        db_pool: DatabasePool,
        redis_client: RedisClient,
        ai_client: AIClient,
        config: SearcherConfig,
    ) -> Self {
        let content_storage = ContentStorage::new(db_pool.pool().clone());
        Self {
            db_pool,
            redis_client,
            ai_client,
            content_storage,
            config,
        }
    }

    fn prepare_document_for_response(
        &self,
        mut doc: shared::models::Document,
    ) -> shared::models::Document {
        // Clear content_id from search responses for security and efficiency
        doc.content_id = None;
        doc
    }

    pub async fn search(&self, request: SearchRequest) -> Result<SearchResponse> {
        let start_time = Instant::now();

        info!(
            "Searching for query: '{}', mode: {:?}",
            request.query,
            request.search_mode()
        );

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

        let (results, corrected_query) = match request.search_mode() {
            SearchMode::Fulltext => self.fulltext_search(&repo, &request).await?,
            SearchMode::Semantic => (self.semantic_search(&request).await?, None),
            SearchMode::Hybrid => self.hybrid_search(&request).await?,
        };

        let total_count = results.len() as i64;
        let has_more = results.len() as i64 >= limit;
        let query_time = start_time.elapsed().as_millis() as u64;

        info!(
            "Search completed in {}ms, found {} results",
            query_time,
            results.len()
        );

        // Get facets if requested
        let facets = if request.include_facets() {
            let sources = request.sources.as_deref();
            let content_types = request.content_types.as_deref();

            repo.get_facet_counts_with_filters(&request.query, sources, content_types)
                .await
                .unwrap_or_else(|e| {
                    info!("Failed to get facet counts: {}", e);
                    vec![]
                })
        } else {
            vec![]
        };

        let response = SearchResponse {
            results,
            total_count,
            query_time_ms: query_time,
            has_more,
            query: request.query,
            corrected_query,
            corrections: None, // TODO: implement word-level corrections tracking
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
    ) -> Result<(Vec<SearchResult>, Option<String>)> {
        let sources = request.sources.as_deref();
        let content_types = request.content_types.as_deref();

        let (documents, corrected_query) = if self.config.typo_tolerance_enabled {
            debug!("Searching for {} with typo tolerance", &request.query);
            repo.search_with_typo_tolerance_and_filters(
                &request.query,
                sources,
                content_types,
                request.limit(),
                request.offset(),
                self.config.typo_tolerance_max_distance,
                self.config.typo_tolerance_min_word_length,
                request.user_email().map(|e| e.as_str()),
            )
            .await?
        } else {
            debug!("Searching for {} without typo tolerance", &request.query);
            (
                repo.search_with_filters(
                    &request.query,
                    sources,
                    content_types,
                    request.limit(),
                    request.offset(),
                    request.user_email().map(|e| e.as_str()),
                )
                .await?,
                None,
            )
        };

        let mut results = Vec::new();
        for doc in documents {
            // Fetch content from LOB storage for highlight generation and content display
            let (highlights, content) = if let Some(content_id) = &doc.content_id {
                match self.content_storage.get_text(content_id).await {
                    Ok(content) => {
                        let highlights = self.extract_highlights(&content, &request.query);
                        // Truncate content for display (first 500 chars)
                        let truncated_content = if content.len() > 500 {
                            format!("{}...", content.chars().take(500).collect::<String>())
                        } else {
                            content.clone()
                        };
                        (highlights, Some(truncated_content))
                    }
                    Err(e) => {
                        debug!(
                            "Failed to fetch content for highlights from document {}: {}",
                            doc.id, e
                        );
                        (vec![], None)
                    }
                }
            } else {
                (vec![], None)
            };

            let prepared_doc = self.prepare_document_for_response(doc);
            results.push(SearchResult {
                document: prepared_doc,
                score: 1.0,
                highlights,
                match_type: "fulltext".to_string(),
                content,
            });
        }

        Ok((results, corrected_query))
    }

    async fn semantic_search(&self, request: &SearchRequest) -> Result<Vec<SearchResult>> {
        info!("Performing semantic search for query: '{}'", request.query);

        let query_embedding = self.generate_query_embedding(&request.query).await?;

        let embedding_repo = EmbeddingRepository::new(self.db_pool.pool());

        let sources = request.sources.as_deref();
        let content_types = request.content_types.as_deref();

        let documents_with_scores = embedding_repo
            .find_similar_with_filters(
                query_embedding,
                sources,
                content_types,
                request.limit(),
                request.offset(),
                request.user_email().map(|e| e.as_str()),
            )
            .await?;

        let mut results = Vec::new();
        for (doc, score) in documents_with_scores {
            let prepared_doc = self.prepare_document_for_response(doc);
            results.push(SearchResult {
                document: prepared_doc,
                score,
                highlights: vec![],
                match_type: "semantic".to_string(),
                content: None, // Semantic search doesn't need full content
            });
        }

        Ok(results)
    }

    async fn generate_query_embedding(&self, query: &str) -> Result<Vec<f32>> {
        let embeddings = self
            .ai_client
            .generate_embeddings(vec![query.to_string()])
            .await?;
        if let Some(first_embedding) = embeddings.first() {
            if let Some(first_chunk) = first_embedding.chunk_embeddings.first() {
                return Ok(first_chunk.clone());
            }
        }
        Err(anyhow::anyhow!("Failed to generate embedding for query"))
    }

    async fn hybrid_search(
        &self,
        request: &SearchRequest,
    ) -> Result<(Vec<SearchResult>, Option<String>)> {
        info!("Performing hybrid search for query: '{}'", request.query);

        // Get results from both FTS and semantic search
        let repo = DocumentRepository::new(self.db_pool.pool());
        let (fts_results, corrected_query) = self.fulltext_search(&repo, request).await?;
        info!("Retrieved {} results from FTS", fts_results.len());
        let semantic_results = self.semantic_search(request).await?;
        info!(
            "Retrieved {} results from semantic search",
            semantic_results.len()
        );

        // Combine and deduplicate results
        let mut combined_results = std::collections::HashMap::new();

        // Add FTS results with normalized scores
        for result in fts_results {
            let doc_id = result.document.id.clone();
            let normalized_score = self.normalize_fts_score(result.score);
            let prepared_doc = self.prepare_document_for_response(result.document);
            combined_results.insert(
                doc_id,
                SearchResult {
                    document: prepared_doc,
                    score: normalized_score * self.config.hybrid_search_fts_weight,
                    highlights: result.highlights,
                    match_type: "hybrid".to_string(),
                    content: result.content,
                },
            );
        }

        // Add or update with semantic results
        for result in semantic_results {
            let doc_id = result.document.id.clone();

            match combined_results.get_mut(&doc_id) {
                Some(existing) => {
                    // Combine scores for documents found in both searches
                    existing.score += result.score * self.config.hybrid_search_semantic_weight;
                }
                None => {
                    // Add new semantic-only result
                    let prepared_doc = self.prepare_document_for_response(result.document);
                    combined_results.insert(
                        doc_id,
                        SearchResult {
                            document: prepared_doc,
                            score: result.score * self.config.hybrid_search_semantic_weight,
                            highlights: result.highlights,
                            match_type: "hybrid".to_string(),
                            content: result.content,
                        },
                    );
                }
            }
        }

        // Convert to vector and sort by combined score
        let mut final_results: Vec<SearchResult> = combined_results.into_values().collect();
        final_results.sort_by(|a, b| {
            b.score
                .partial_cmp(&a.score)
                .unwrap_or(std::cmp::Ordering::Equal)
        });

        // Apply limit
        if final_results.len() > request.limit() as usize {
            final_results.truncate(request.limit() as usize);
        }

        Ok((final_results, corrected_query))
    }

    fn normalize_fts_score(&self, score: f32) -> f32 {
        // Simple normalization - in practice this would be more sophisticated
        // based on the actual FTS scoring algorithm
        score.min(1.0).max(0.0)
    }

    fn generate_cache_key(&self, request: &SearchRequest) -> String {
        use std::collections::hash_map::DefaultHasher;
        use std::hash::{Hash, Hasher};

        let mut hasher = DefaultHasher::new();
        request.query.hash(&mut hasher);
        request.search_mode().hash(&mut hasher);
        request.limit().hash(&mut hasher);
        request.offset().hash(&mut hasher);

        if let Some(sources) = &request.sources {
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

        if let Some(user_email) = &request.user_email {
            user_email.hash(&mut hasher);
        }

        format!("search:{:x}", hasher.finish())
    }

    fn extract_highlights(&self, content: &str, query: &str) -> Vec<String> {
        if content.is_empty() || query.is_empty() {
            return vec![];
        }

        let query_lower = query.to_lowercase();
        let content_lower = content.to_lowercase();
        let mut highlights = Vec::new();

        // Find all occurrences of the query terms
        let terms: Vec<&str> = query_lower.split_whitespace().collect();

        for term in terms {
            if term.len() < 3 {
                continue; // Skip very short terms
            }

            // Find all positions where this term appears
            let mut search_start = 0;
            while let Some(pos) = content_lower[search_start..].find(term) {
                let absolute_pos = search_start + pos;

                // Extract context around the match (50 chars before and after)
                let context_start = absolute_pos.saturating_sub(50);
                let context_end = (absolute_pos + term.len() + 50).min(content.len());

                // Find word boundaries, ensuring we respect UTF-8 char boundaries
                let start = if context_start == 0 {
                    0
                } else {
                    // Find a safe position that's on a char boundary
                    let safe_start = content
                        .char_indices()
                        .take_while(|(i, _)| *i <= context_start)
                        .last()
                        .map(|(i, _)| i)
                        .unwrap_or(0);

                    content[..safe_start]
                        .rfind(char::is_whitespace)
                        .map(|i| i + 1)
                        .unwrap_or(safe_start)
                };

                let end = if context_end >= content.len() {
                    content.len()
                } else {
                    // Find a safe position that's on a char boundary
                    let safe_end = content
                        .char_indices()
                        .find(|(i, _)| *i >= context_end)
                        .map(|(i, _)| i)
                        .unwrap_or(content.len());

                    content[safe_end..]
                        .find(char::is_whitespace)
                        .map(|i| safe_end + i)
                        .unwrap_or(safe_end)
                };

                let mut snippet = String::new();
                if start > 0 {
                    snippet.push_str("...");
                }

                // Add the snippet with the term highlighted using markdown bold
                let snippet_text = &content[start..end];
                let highlighted = snippet_text.replace(
                    &content[absolute_pos..absolute_pos + term.len()],
                    &format!("**{}**", &content[absolute_pos..absolute_pos + term.len()]),
                );
                snippet.push_str(&highlighted);

                if end < content.len() {
                    snippet.push_str("...");
                }

                highlights.push(snippet);

                // Only keep first 3 highlights per term
                if highlights.len() >= 3 {
                    break;
                }

                search_start = absolute_pos + term.len();
            }
        }

        // Deduplicate and limit total highlights
        highlights.sort();
        highlights.dedup();
        highlights.truncate(5);

        highlights
    }

    fn extract_context_around_matches(&self, content: &str, query: &str) -> String {
        if content.is_empty() || query.is_empty() {
            return String::new();
        }

        let query_lower = query.to_lowercase();
        let content_lower = content.to_lowercase();
        let terms: Vec<&str> = query_lower.split_whitespace().collect();

        let mut contexts = Vec::new();

        for term in terms {
            if term.len() < 3 {
                continue;
            }

            if let Some(pos) = content_lower.find(term) {
                // Extract larger context around the match (200 chars before and after)
                let context_start = pos.saturating_sub(200);
                let context_end = (pos + term.len() + 200).min(content.len());

                // Find sentence boundaries for cleaner context, ensuring UTF-8 char boundaries
                let start = if context_start == 0 {
                    0
                } else {
                    // Find a safe position that's on a char boundary
                    let safe_start = content
                        .char_indices()
                        .take_while(|(i, _)| *i <= context_start)
                        .last()
                        .map(|(i, _)| i)
                        .unwrap_or(0);

                    content[..safe_start]
                        .rfind('.')
                        .map(|i| i + 1)
                        .unwrap_or(safe_start)
                };

                let end = if context_end >= content.len() {
                    content.len()
                } else {
                    // Find a safe position that's on a char boundary
                    let safe_end = content
                        .char_indices()
                        .find(|(i, _)| *i >= context_end)
                        .map(|(i, _)| i)
                        .unwrap_or(content.len());

                    content[safe_end..]
                        .find('.')
                        .map(|i| safe_end + i + 1)
                        .unwrap_or(safe_end)
                };

                let context_text = content[start..end].trim();
                if !context_text.is_empty() && !contexts.contains(&context_text) {
                    contexts.push(context_text);
                }
            }
        }

        // Join contexts and limit total length
        let combined = contexts.join(" ... ");
        if combined.chars().count() > 1000 {
            let truncated: String = combined.chars().take(1000).collect();
            format!("{}...", truncated)
        } else {
            combined
        }
    }

    pub async fn suggest(&self, query: &str, limit: i64) -> Result<SuggestionsResponse> {
        info!("Getting suggestions for query: '{}'", query);

        if query.trim().is_empty() {
            return Ok(SuggestionsResponse {
                suggestions: vec![],
                query: query.to_string(),
            });
        }

        let suggestions = sqlx::query_scalar::<_, String>(
            r#"
            SELECT DISTINCT title
            FROM documents
            WHERE title ILIKE $1
            ORDER BY title
            LIMIT $2
            "#,
        )
        .bind(format!("%{}%", query))
        .bind(limit)
        .fetch_all(self.db_pool.pool())
        .await?;

        info!("Found {} suggestions", suggestions.len());

        Ok(SuggestionsResponse {
            suggestions,
            query: query.to_string(),
        })
    }

    /// Generate RAG context from search request using chunk-based approach
    pub async fn get_rag_context(&self, request: &SearchRequest) -> Result<Vec<SearchResult>> {
        info!("Generating RAG context for query: '{}'", request.query);

        // Get embedding chunks that match the query semantically
        let query_embedding = self.generate_query_embedding(&request.query).await?;
        let embedding_repo = EmbeddingRepository::new(self.db_pool.pool());

        // Get multiple chunks per document for better context
        let embedding_chunks = embedding_repo
            .find_rag_chunks(
                query_embedding,
                3,   // max 3 chunks per document
                0.7, // similarity threshold
                15,  // max 15 total chunks
                request.user_email().map(|e| e.as_str()),
            )
            .await?;

        // Get full-text search results to extract context around exact matches
        let repo = DocumentRepository::new(self.db_pool.pool());
        let (fts_results, _) = self.fulltext_search(&repo, request).await?;

        // Combine embedding chunks and fulltext context
        let mut combined_results = Vec::new();

        // Add embedding chunks as SearchResults
        for (doc, score, chunk_text) in embedding_chunks {
            let mut doc_with_chunk = self.prepare_document_for_response(doc);
            // Create a new document with the specific chunk for semantic matches
            // Note: We don't set content field as it's not part of the Document model anymore

            combined_results.push(SearchResult {
                document: doc_with_chunk,
                score,
                highlights: vec![],
                match_type: "semantic_chunk".to_string(),
                content: Some(chunk_text),
            });
        }

        // Add context around fulltext matches
        for fts_result in fts_results.into_iter().take(5) {
            // For fulltext matches, we already have highlights generated
            // Use the prepared document and existing highlights
            combined_results.push(SearchResult {
                document: fts_result.document,
                score: fts_result.score,
                highlights: fts_result.highlights,
                match_type: "fulltext_context".to_string(),
                content: fts_result.content,
            });
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

    /// Build RAG prompt with context chunks and citation instructions
    pub fn build_rag_prompt(&self, query: &str, context: &[SearchResult]) -> String {
        let mut prompt = String::new();

        prompt.push_str("You are a helpful AI assistant that answers questions based on the provided context from various documents. ");
        prompt.push_str(
            "Please provide a comprehensive answer using the information from the context. ",
        );
        prompt.push_str(
            "When referencing information, cite it using the format [Source: Document Title]. ",
        );
        prompt.push_str(
            "If the context doesn't contain enough information to answer the question, say so.\n\n",
        );

        prompt.push_str("Context Information:\n");
        for (i, result) in context.iter().enumerate() {
            prompt.push_str(&format!(
                "Context {}: From \"{}\" ({})\n",
                i + 1,
                result.document.title,
                result.match_type
            ));

            match result.match_type.as_str() {
                "semantic_chunk" => {
                    // For semantic chunks, use the highlights if available
                    if !result.highlights.is_empty() {
                        prompt.push_str(&format!("Content: {}\n", result.highlights[0]));
                    }
                }
                "fulltext_context" => {
                    // For fulltext matches, use the highlights which contain context around matches
                    if !result.highlights.is_empty() {
                        prompt.push_str(&format!("Relevant excerpt: {}\n", result.highlights[0]));
                    }
                }
                _ => {
                    // Fallback: try to get content from LOB storage for other match types
                    if let Some(_content_id) = &result.document.content_id {
                        // Note: In a real implementation, we would need to fetch from LOB storage
                        // For now, we'll use the highlights if available
                        if !result.highlights.is_empty() {
                            prompt.push_str(&format!("Content: {}\n", result.highlights[0]));
                        }
                    }
                }
            }
            prompt.push_str("\n");
        }

        prompt.push_str(&format!("Question: {}\n\n", query));
        prompt.push_str("Answer:");

        prompt
    }
}

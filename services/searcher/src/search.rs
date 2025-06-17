use crate::models::{SearchMode, SearchRequest, SearchResponse, SearchResult, SuggestionsResponse};
use anyhow::Result;
use redis::{AsyncCommands, Client as RedisClient};
use shared::db::repositories::{DocumentRepository, EmbeddingRepository};
use shared::{AIClient, DatabasePool, SearcherConfig};
use std::time::Instant;
use tracing::info;

pub struct SearchEngine {
    db_pool: DatabasePool,
    redis_client: RedisClient,
    ai_client: AIClient,
    config: SearcherConfig,
}

impl SearchEngine {
    pub fn new(
        db_pool: DatabasePool,
        redis_client: RedisClient,
        ai_client: AIClient,
        config: SearcherConfig,
    ) -> Self {
        Self {
            db_pool,
            redis_client,
            ai_client,
            config,
        }
    }

    fn truncate_document_content(
        &self,
        mut doc: shared::models::Document,
    ) -> shared::models::Document {
        const MAX_CONTENT_LENGTH: usize = 500;
        if let Some(content) = &doc.content {
            if content.len() > MAX_CONTENT_LENGTH {
                doc.content = Some(format!("{}...", &content[..MAX_CONTENT_LENGTH]));
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
            facets,
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
            repo.search_with_typo_tolerance_and_filters(
                &request.query,
                sources,
                content_types,
                request.limit(),
                request.offset(),
                self.config.typo_tolerance_max_distance,
                self.config.typo_tolerance_min_word_length,
            )
            .await?
        } else {
            (
                repo.search_with_filters(
                    &request.query,
                    sources,
                    content_types,
                    request.limit(),
                    request.offset(),
                )
                .await?,
                None,
            )
        };

        let results = documents
            .into_iter()
            .map(|doc| {
                let highlights = if let Some(content) = &doc.content {
                    self.extract_highlights(content, &request.query)
                } else {
                    vec![]
                };
                SearchResult {
                    document: self.truncate_document_content(doc),
                    score: 1.0,
                    highlights,
                    match_type: "fulltext".to_string(),
                }
            })
            .collect();

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
            )
            .await?;

        let results: Vec<SearchResult> = documents_with_scores
            .into_iter()
            .map(|(doc, score)| SearchResult {
                document: self.truncate_document_content(doc),
                score,
                highlights: vec![],
                match_type: "semantic".to_string(),
            })
            .collect();

        Ok(results)
    }

    async fn generate_query_embedding(&self, query: &str) -> Result<Vec<f32>> {
        let embeddings = self
            .ai_client
            .generate_embeddings(&[query.to_string()])
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
        let semantic_results = self.semantic_search(request).await?;

        // Combine and deduplicate results
        let mut combined_results = std::collections::HashMap::new();

        // Add FTS results with normalized scores
        for result in fts_results {
            let doc_id = result.document.id.clone();
            let normalized_score = self.normalize_fts_score(result.score);
            combined_results.insert(
                doc_id,
                SearchResult {
                    document: self.truncate_document_content(result.document),
                    score: normalized_score * self.config.hybrid_search_fts_weight,
                    highlights: result.highlights,
                    match_type: "hybrid".to_string(),
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
                    combined_results.insert(
                        doc_id,
                        SearchResult {
                            document: self.truncate_document_content(result.document),
                            score: result.score * self.config.hybrid_search_semantic_weight,
                            highlights: result.highlights,
                            match_type: "hybrid".to_string(),
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

                // Find word boundaries
                let start = content[..context_start]
                    .rfind(char::is_whitespace)
                    .map(|i| i + 1)
                    .unwrap_or(context_start);

                let end = content[context_end..]
                    .find(char::is_whitespace)
                    .map(|i| context_end + i)
                    .unwrap_or(context_end);

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

    /// Generate RAG context from search request by running hybrid search
    pub async fn get_rag_context(&self, request: &SearchRequest) -> Result<Vec<SearchResult>> {
        info!("Generating RAG context for query: '{}'", request.query);

        // Always use hybrid search for RAG to get best context
        let (results, _) = self.hybrid_search(request).await?;

        // Take top 5 results for RAG context
        let context_results: Vec<SearchResult> = results.into_iter().take(5).collect();

        info!(
            "Generated RAG context with {} documents",
            context_results.len()
        );
        Ok(context_results)
    }

    /// Build RAG prompt with context documents and citation instructions
    pub fn build_rag_prompt(&self, query: &str, context: &[SearchResult]) -> String {
        let mut prompt = String::new();

        prompt.push_str("You are a helpful AI assistant that answers questions based on the provided context documents. ");
        prompt.push_str(
            "Please provide a comprehensive answer using the information from the documents. ",
        );
        prompt.push_str("When referencing information from a document, cite it using the format [Source: Document Title]. ");
        prompt.push_str(
            "If the context doesn't contain enough information to answer the question, say so.\n\n",
        );

        prompt.push_str("Context Documents:\n");
        for (i, result) in context.iter().enumerate() {
            prompt.push_str(&format!("Document {}: {}\n", i + 1, result.document.title));
            if let Some(content) = &result.document.content {
                // Truncate content for context (keep more than search results)
                let truncated_content = if content.len() > 2000 {
                    format!("{}...", &content[..2000])
                } else {
                    content.clone()
                };
                prompt.push_str(&format!("Content: {}\n", truncated_content));
            }
            prompt.push_str("\n");
        }

        prompt.push_str(&format!("Question: {}\n\n", query));
        prompt.push_str("Answer:");

        prompt
    }
}

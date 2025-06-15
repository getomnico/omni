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

        let (results, corrected_query) = if request.query.trim().is_empty() {
            let documents = repo.find_all(limit, request.offset()).await?;
            let results = documents
                .into_iter()
                .map(|doc| SearchResult {
                    document: self.truncate_document_content(doc),
                    score: 1.0,
                    highlights: vec![],
                    match_type: "listing".to_string(),
                })
                .collect();
            (results, None)
        } else {
            match request.search_mode() {
                SearchMode::Fulltext => self.fulltext_search(&repo, &request).await?,
                SearchMode::Semantic => (self.semantic_search(&request).await?, None),
                SearchMode::Hybrid => self.hybrid_search(&request).await?,
            }
        };

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
            query: request.query,
            corrected_query,
            corrections: None, // TODO: implement word-level corrections tracking
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
        let (mut documents, corrected_query) = if self.config.typo_tolerance_enabled {
            repo.search_with_typo_tolerance(
                &request.query,
                request.limit(),
                self.config.typo_tolerance_max_distance,
                self.config.typo_tolerance_min_word_length,
            )
            .await?
        } else {
            (repo.search(&request.query, request.limit()).await?, None)
        };

        if let Some(sources) = &request.sources {
            if !sources.is_empty() {
                documents.retain(|doc| sources.contains(&doc.source_id));
            }
        }

        if let Some(content_types) = &request.content_types {
            if !content_types.is_empty() {
                documents.retain(|doc| {
                    doc.content_type
                        .as_ref()
                        .map(|ct| content_types.contains(ct))
                        .unwrap_or(false)
                });
            }
        }

        if request.offset() > 0 {
            let offset = request.offset() as usize;
            if offset < documents.len() {
                documents = documents[offset..].to_vec();
            } else {
                documents.clear();
            }
        }

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
        let documents_with_scores = embedding_repo
            .find_similar(query_embedding, request.limit())
            .await?;

        let mut results: Vec<SearchResult> = documents_with_scores
            .into_iter()
            .map(|(doc, score)| SearchResult {
                document: self.truncate_document_content(doc),
                score,
                highlights: vec![],
                match_type: "semantic".to_string(),
            })
            .collect();

        // Apply filters
        if let Some(sources) = &request.sources {
            if !sources.is_empty() {
                results.retain(|result| sources.contains(&result.document.source_id));
            }
        }

        if let Some(content_types) = &request.content_types {
            if !content_types.is_empty() {
                results.retain(|result| {
                    result
                        .document
                        .content_type
                        .as_ref()
                        .map(|ct| content_types.contains(ct))
                        .unwrap_or(false)
                });
            }
        }

        // Apply offset
        if request.offset() > 0 {
            let offset = request.offset() as usize;
            if offset < results.len() {
                results = results[offset..].to_vec();
            } else {
                results.clear();
            }
        }

        Ok(results)
    }

    async fn generate_query_embedding(&self, query: &str) -> Result<Vec<f32>> {
        self.ai_client.generate_embedding(query).await
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
                    score: normalized_score * 0.6, // Weight FTS at 60%
                    highlights: result.highlights,
                    match_type: "hybrid".to_string(),
                },
            );
        }

        // Add or update with semantic results
        for result in semantic_results {
            let doc_id = result.document.id.clone();
            let semantic_weight = 0.4; // Weight semantic at 40%

            match combined_results.get_mut(&doc_id) {
                Some(existing) => {
                    // Combine scores for documents found in both searches
                    existing.score += result.score * semantic_weight;
                }
                None => {
                    // Add new semantic-only result
                    combined_results.insert(
                        doc_id,
                        SearchResult {
                            document: self.truncate_document_content(result.document),
                            score: result.score * semantic_weight,
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
}

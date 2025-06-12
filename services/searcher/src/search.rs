use crate::models::{SearchMode, SearchRequest, SearchResponse, SearchResult, SuggestionsResponse};
use anyhow::Result;
use shared::db::repositories::{DocumentRepository, EmbeddingRepository};
use sqlx::PgPool;
use std::time::Instant;
use tracing::{info, warn};

pub struct SearchEngine {
    db_pool: PgPool,
}

impl SearchEngine {
    pub fn new(db_pool: PgPool) -> Self {
        Self { db_pool }
    }

    pub async fn search(&self, request: SearchRequest) -> Result<SearchResponse> {
        let start_time = Instant::now();

        info!(
            "Searching for query: '{}', mode: {:?}",
            request.query,
            request.search_mode()
        );

        let repo = DocumentRepository::new(&self.db_pool);
        let limit = request.limit();

        let results = if request.query.trim().is_empty() {
            let documents = repo.find_all(limit, request.offset()).await?;
            documents
                .into_iter()
                .map(|doc| SearchResult {
                    document: doc,
                    score: 1.0,
                    highlights: vec![],
                    match_type: "listing".to_string(),
                })
                .collect()
        } else {
            match request.search_mode() {
                SearchMode::Fulltext => self.fulltext_search(&repo, &request).await?,
                SearchMode::Semantic => self.semantic_search(&request).await?,
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

        Ok(SearchResponse {
            results,
            total_count,
            query_time_ms: query_time,
            has_more,
            query: request.query,
        })
    }

    async fn fulltext_search(
        &self,
        repo: &DocumentRepository,
        request: &SearchRequest,
    ) -> Result<Vec<SearchResult>> {
        let mut documents = repo.search(&request.query, request.limit()).await?;

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
            .map(|doc| SearchResult {
                document: doc,
                score: 1.0,
                highlights: vec![],
                match_type: "fulltext".to_string(),
            })
            .collect();

        Ok(results)
    }

    async fn semantic_search(&self, request: &SearchRequest) -> Result<Vec<SearchResult>> {
        info!("Performing semantic search for query: '{}'", request.query);

        let query_embedding = self.generate_query_embedding(&request.query).await?;

        let embedding_repo = EmbeddingRepository::new(&self.db_pool);
        let documents_with_scores = embedding_repo
            .find_similar(query_embedding, request.limit())
            .await?;

        let mut results: Vec<SearchResult> = documents_with_scores
            .into_iter()
            .map(|(doc, score)| SearchResult {
                document: doc,
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
        // TODO: Implement actual embedding generation via AI service
        // For now, return a placeholder embedding
        warn!("Using placeholder embedding for query: '{}'", query);

        // Return a 1024-dimensional zero vector as placeholder
        // This matches the intfloat/e5-large-v2 model dimensions mentioned in CLAUDE.md
        Ok(vec![0.0; 1024])
    }

    async fn hybrid_search(&self, request: &SearchRequest) -> Result<Vec<SearchResult>> {
        info!("Performing hybrid search for query: '{}'", request.query);

        // Get results from both FTS and semantic search
        let repo = DocumentRepository::new(&self.db_pool);
        let fts_results = self.fulltext_search(&repo, request).await?;
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
                    document: result.document,
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
                            document: result.document,
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

        Ok(final_results)
    }

    fn normalize_fts_score(&self, score: f32) -> f32 {
        // Simple normalization - in practice this would be more sophisticated
        // based on the actual FTS scoring algorithm
        score.min(1.0).max(0.0)
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
        .fetch_all(&self.db_pool)
        .await?;

        info!("Found {} suggestions", suggestions.len());

        Ok(SuggestionsResponse {
            suggestions,
            query: query.to_string(),
        })
    }
}

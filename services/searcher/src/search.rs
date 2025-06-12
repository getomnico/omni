use crate::models::{SearchRequest, SearchResponse, SearchResult, SuggestionsResponse};
use anyhow::Result;
use shared::db::repositories::DocumentRepository;
use shared::models::Document;
use sqlx::PgPool;
use std::time::Instant;
use tracing::info;

pub struct SearchEngine {
    db_pool: PgPool,
}

impl SearchEngine {
    pub fn new(db_pool: PgPool) -> Self {
        Self { db_pool }
    }

    pub async fn search(&self, request: SearchRequest) -> Result<SearchResponse> {
        let start_time = Instant::now();
        
        info!("Searching for query: '{}'", request.query);

        let repo = DocumentRepository::new(&self.db_pool);
        let limit = request.limit();
        
        let documents = if request.query.trim().is_empty() {
            repo.find_all(limit, request.offset()).await?
        } else {
            self.search_with_filters(&repo, &request).await?
        };

        let total_count = documents.len() as i64;
        let has_more = documents.len() as i64 >= limit;

        let results: Vec<SearchResult> = documents
            .into_iter()
            .map(|doc| SearchResult {
                document: doc,
                score: 1.0,
                match_type: "fulltext".to_string(),
            })
            .collect();

        let query_time = start_time.elapsed().as_millis() as u64;
        
        info!("Search completed in {}ms, found {} results", query_time, results.len());

        Ok(SearchResponse {
            results,
            total_count,
            query_time_ms: query_time,
            has_more,
            query: request.query,
        })
    }

    async fn search_with_filters(
        &self,
        repo: &DocumentRepository,
        request: &SearchRequest,
    ) -> Result<Vec<Document>> {
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

        Ok(documents)
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
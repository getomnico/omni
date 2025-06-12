use serde::{Deserialize, Serialize};
use shared::models::Document;

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum SearchMode {
    Fulltext,
    Semantic,
    Hybrid,
}

#[derive(Debug, Deserialize, Serialize)]
pub struct SearchRequest {
    pub query: String,
    pub sources: Option<Vec<String>>,
    pub content_types: Option<Vec<String>>,
    pub limit: Option<i64>,
    pub offset: Option<i64>,
    pub mode: Option<SearchMode>,
}

impl SearchRequest {
    pub fn limit(&self) -> i64 {
        self.limit.unwrap_or(20).min(100)
    }

    pub fn offset(&self) -> i64 {
        self.offset.unwrap_or(0).max(0)
    }

    pub fn search_mode(&self) -> &SearchMode {
        self.mode.as_ref().unwrap_or(&SearchMode::Fulltext)
    }
}

#[derive(Debug, Serialize)]
pub struct SearchResponse {
    pub results: Vec<SearchResult>,
    pub total_count: i64,
    pub query_time_ms: u64,
    pub has_more: bool,
    pub query: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct SearchResult {
    pub document: Document,
    pub score: f32,
    pub highlights: Vec<String>,
    pub match_type: String,
}

#[derive(Debug, Deserialize)]
pub struct SuggestionsQuery {
    pub q: String,
    pub limit: Option<i64>,
}

impl SuggestionsQuery {
    pub fn limit(&self) -> i64 {
        self.limit.unwrap_or(5).min(20)
    }
}

#[derive(Debug, Serialize)]
pub struct SuggestionsResponse {
    pub suggestions: Vec<String>,
    pub query: String,
}

use serde::{Deserialize, Serialize};
use shared::{
    models::{Document, Facet},
    SourceType,
};

#[derive(Debug, Clone, Deserialize, Serialize, Hash, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum SearchMode {
    Fulltext,
    Semantic,
    Hybrid,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct SearchRequest {
    pub query: String,
    pub source_types: Option<Vec<SourceType>>,
    pub content_types: Option<Vec<String>>,
    pub limit: Option<i64>,
    pub offset: Option<i64>,
    pub mode: Option<SearchMode>,
    pub include_facets: Option<bool>,
    pub user_email: Option<String>,
    pub user_id: Option<String>,
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

    pub fn include_facets(&self) -> bool {
        self.include_facets.unwrap_or(true)
    }

    pub fn user_email(&self) -> Option<&String> {
        self.user_email.as_ref()
    }
}

#[derive(Debug, Serialize, Deserialize)]
pub struct SearchResponse {
    pub results: Vec<SearchResult>,
    pub total_count: i64,
    pub query_time_ms: u64,
    pub has_more: bool,
    pub query: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub corrected_query: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub corrections: Option<Vec<WordCorrection>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub facets: Option<Vec<Facet>>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct WordCorrection {
    pub original: String,
    pub corrected: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SearchResult {
    pub document: Document,
    pub score: f32,
    pub highlights: Vec<String>,
    pub match_type: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub content: Option<String>,
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

#[derive(Debug, Deserialize)]
pub struct RecentSearchesRequest {
    pub user_id: String,
}

#[derive(Debug, Serialize)]
pub struct RecentSearchesResponse {
    pub searches: Vec<String>,
}

use serde::{Deserialize, Serialize};
use shared::{
    models::{AttributeFilter, Document, Facet},
    SourceType,
};
use std::collections::HashMap;

#[derive(Debug, Clone, Deserialize, Serialize, Hash, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum SearchMode {
    Fulltext,
    Semantic,
    Hybrid,
}

#[derive(Debug, Clone, Default, Deserialize, Serialize)]
pub struct SearchRequest {
    pub query: String,
    pub source_types: Option<Vec<SourceType>>,
    pub content_types: Option<Vec<String>>,
    /// Attribute filters for filtering by document attributes.
    /// Keys are attribute names, values are AttributeFilter specifications.
    /// Examples:
    /// - `{"status": "Done"}` - exact match
    /// - `{"labels": ["bug", "urgent"]}` - match any of these values
    /// - `{"date": {"gte": "2024-01-01", "lte": "2024-12-31"}}` - date range
    pub attribute_filters: Option<HashMap<String, AttributeFilter>>,
    pub limit: Option<i64>,
    pub offset: Option<i64>,
    pub mode: Option<SearchMode>,
    pub include_facets: Option<bool>,
    pub user_email: Option<String>,
    pub user_id: Option<String>,
    pub is_generated_query: Option<bool>,
    pub original_user_query: Option<String>,
    pub document_id: Option<String>,
    // This SearchRequest doubles as a ReadDocumentRequest (because we want to support searching
    // through a single doc, to handle large documents).
    // So these next two fields allow us to read a specific set of lines from the document.
    // Both inclusive.
    pub document_content_start_line: Option<u32>,
    pub document_content_end_line: Option<u32>,
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
    pub facets: Option<Vec<Facet>>,
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

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SuggestedQuestion {
    pub question: String,
    pub document_id: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SuggestedQuestionsRequest {
    pub user_id: String,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct SuggestedQuestionsResponse {
    pub questions: Vec<SuggestedQuestion>,
}

#[derive(Debug, Deserialize)]
pub struct TypeaheadQuery {
    pub q: String,
    pub limit: Option<usize>,
}

impl TypeaheadQuery {
    pub fn limit(&self) -> usize {
        self.limit.unwrap_or(5).min(20)
    }
}

#[derive(Debug, Serialize)]
pub struct TypeaheadResponse {
    pub results: Vec<TypeaheadResult>,
    pub query: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct TypeaheadResult {
    pub document_id: String,
    pub title: String,
    pub url: Option<String>,
    pub source_id: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_search_request_defaults() {
        let request = SearchRequest {
            query: "test".to_string(),
            ..Default::default()
        };

        assert_eq!(request.limit(), 20);
        assert_eq!(request.offset(), 0);
        assert!(matches!(request.search_mode(), SearchMode::Fulltext));
    }

    #[test]
    fn test_search_request_limits() {
        let request = SearchRequest {
            query: "test".to_string(),
            limit: Some(200),
            offset: Some(-10),
            ..Default::default()
        };

        assert_eq!(request.limit(), 100); // Should be capped at 100
        assert_eq!(request.offset(), 0); // Negative offset should become 0
    }

    #[test]
    fn test_search_modes() {
        let modes = vec![
            SearchMode::Fulltext,
            SearchMode::Semantic,
            SearchMode::Hybrid,
        ];

        for mode in modes {
            let request = SearchRequest {
                query: "test".to_string(),
                mode: Some(mode.clone()),
                ..Default::default()
            };

            match (request.search_mode(), &mode) {
                (SearchMode::Fulltext, SearchMode::Fulltext) => (),
                (SearchMode::Semantic, SearchMode::Semantic) => (),
                (SearchMode::Hybrid, SearchMode::Hybrid) => (),
                _ => panic!("Search mode mismatch"),
            }
        }
    }

    #[test]
    fn test_suggestions_query_defaults() {
        let query = SuggestionsQuery {
            q: "test".to_string(),
            limit: None,
        };

        assert_eq!(query.limit(), 5);
    }

    #[test]
    fn test_suggestions_query_limit_cap() {
        let query = SuggestionsQuery {
            q: "test".to_string(),
            limit: Some(50),
        };

        assert_eq!(query.limit(), 20); // Should be capped at 20
    }

    #[test]
    fn test_search_mode_serialization() {
        let mode = SearchMode::Semantic;
        let json = serde_json::to_string(&mode).unwrap();
        assert_eq!(json, "\"semantic\"");

        let mode = SearchMode::Hybrid;
        let json = serde_json::to_string(&mode).unwrap();
        assert_eq!(json, "\"hybrid\"");

        let mode = SearchMode::Fulltext;
        let json = serde_json::to_string(&mode).unwrap();
        assert_eq!(json, "\"fulltext\"");
    }

    #[test]
    fn test_search_mode_deserialization() {
        let mode: SearchMode = serde_json::from_str("\"semantic\"").unwrap();
        assert!(matches!(mode, SearchMode::Semantic));

        let mode: SearchMode = serde_json::from_str("\"hybrid\"").unwrap();
        assert!(matches!(mode, SearchMode::Hybrid));

        let mode: SearchMode = serde_json::from_str("\"fulltext\"").unwrap();
        assert!(matches!(mode, SearchMode::Fulltext));
    }
}

#[cfg(test)]
mod tests {
    use clio_searcher::models::{SearchMode, SearchRequest, SearchResponse};

    #[test]
    fn test_search_request_semantic_mode() {
        let request = SearchRequest {
            query: "machine learning algorithms".to_string(),
            source_types: None,
            content_types: None,
            limit: Some(10),
            offset: None,
            mode: Some(SearchMode::Semantic),
            include_facets: Some(false),
        };

        assert_eq!(request.search_mode(), &SearchMode::Semantic);
        assert_eq!(request.limit(), 10);
        assert!(!request.include_facets());
    }

    #[test]
    fn test_search_request_hybrid_mode() {
        let request = SearchRequest {
            query: "technical documentation".to_string(),
            source_types: Some(vec!["docs".to_string()]),
            content_types: None,
            limit: None,
            offset: None,
            mode: Some(SearchMode::Hybrid),
            include_facets: None,
        };

        assert_eq!(request.search_mode(), &SearchMode::Hybrid);
        assert_eq!(request.limit(), 20); // default
        assert!(request.include_facets()); // default true
        assert_eq!(request.source_types.as_ref().unwrap().len(), 1);
    }

    #[test]
    fn test_search_response_structure() {
        let response = SearchResponse {
            results: vec![],
            total_count: 42,
            query_time_ms: 150,
            has_more: true,
            query: "vector search test".to_string(),
            corrected_query: None,
            corrections: None,
            facets: None,
        };

        assert_eq!(response.total_count, 42);
        assert_eq!(response.query_time_ms, 150);
        assert!(response.has_more);
        assert_eq!(response.query, "vector search test");
        assert_eq!(response.results.len(), 0);
        assert!(response.facets.is_none());
    }

    #[test]
    fn test_search_modes_enum() {
        // Test that all search modes are available
        let modes = vec![
            SearchMode::Fulltext,
            SearchMode::Semantic,
            SearchMode::Hybrid,
        ];

        assert_eq!(modes.len(), 3);

        // Test serialization works
        for mode in modes {
            let serialized = serde_json::to_string(&mode).unwrap();
            let deserialized: SearchMode = serde_json::from_str(&serialized).unwrap();
            assert_eq!(mode, deserialized);
        }
    }
}

#[cfg(test)]
mod tests {
    use clio_searcher::models::{SearchMode, SearchRequest};

    #[test]
    fn test_search_request_defaults() {
        let request = SearchRequest {
            query: "test".to_string(),
            source_types: None,
            content_types: None,
            limit: None,
            offset: None,
            mode: None,
            include_facets: None,
        };

        assert_eq!(request.limit(), 20);
        assert_eq!(request.offset(), 0);
        assert!(matches!(request.search_mode(), SearchMode::Fulltext));
    }

    #[test]
    fn test_search_request_limits() {
        let request = SearchRequest {
            query: "test".to_string(),
            source_types: None,
            content_types: None,
            limit: Some(200),
            offset: Some(-10),
            mode: None,
            include_facets: None,
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
                source_types: None,
                content_types: None,
                limit: None,
                offset: None,
                mode: Some(mode.clone()),
                include_facets: None,
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
        use clio_searcher::models::SuggestionsQuery;

        let query = SuggestionsQuery {
            q: "test".to_string(),
            limit: None,
        };

        assert_eq!(query.limit(), 5);
    }

    #[test]
    fn test_suggestions_query_limit_cap() {
        use clio_searcher::models::SuggestionsQuery;

        let query = SuggestionsQuery {
            q: "test".to_string(),
            limit: Some(50),
        };

        assert_eq!(query.limit(), 20); // Should be capped at 20
    }
}

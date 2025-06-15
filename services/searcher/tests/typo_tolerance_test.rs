#[cfg(test)]
mod tests {
    use clio_searcher::models::{SearchResponse, WordCorrection};

    #[test]
    fn test_search_response_with_corrections() {
        let response = SearchResponse {
            results: vec![],
            total_count: 0,
            query_time_ms: 100,
            has_more: false,
            query: "orginal query".to_string(),
            corrected_query: Some("original query".to_string()),
            corrections: Some(vec![WordCorrection {
                original: "orginal".to_string(),
                corrected: "original".to_string(),
            }]),
        };

        assert_eq!(response.corrected_query, Some("original query".to_string()));
        assert!(response.corrections.is_some());

        if let Some(corrections) = response.corrections {
            assert_eq!(corrections.len(), 1);
            assert_eq!(corrections[0].original, "orginal");
            assert_eq!(corrections[0].corrected, "original");
        }
    }

    #[test]
    fn test_search_response_without_corrections() {
        let response = SearchResponse {
            results: vec![],
            total_count: 0,
            query_time_ms: 100,
            has_more: false,
            query: "correct query".to_string(),
            corrected_query: None,
            corrections: None,
        };

        assert!(response.corrected_query.is_none());
        assert!(response.corrections.is_none());
    }

    #[test]
    fn test_word_correction_serialization() {
        let correction = WordCorrection {
            original: "teh".to_string(),
            corrected: "the".to_string(),
        };

        let json = serde_json::to_string(&correction).unwrap();
        assert!(json.contains("\"original\":\"teh\""));
        assert!(json.contains("\"corrected\":\"the\""));

        let deserialized: WordCorrection = serde_json::from_str(&json).unwrap();
        assert_eq!(deserialized.original, "teh");
        assert_eq!(deserialized.corrected, "the");
    }

    #[test]
    fn test_search_response_serialization_skips_none_fields() {
        let response = SearchResponse {
            results: vec![],
            total_count: 0,
            query_time_ms: 100,
            has_more: false,
            query: "test".to_string(),
            corrected_query: None,
            corrections: None,
        };

        let json = serde_json::to_string(&response).unwrap();

        // These fields should not appear in JSON when they are None
        assert!(!json.contains("corrected_query"));
        assert!(!json.contains("corrections"));
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;
    use shared::db::repositories::DocumentRepository;
    use shared::models::Document;
    use shared::test_utils::BaseTestFixture;
    use sqlx::types::time::OffsetDateTime;
    use ulid::Ulid;

    #[tokio::test]
    async fn test_find_similar_words() {
        let fixture = BaseTestFixture::new().await.unwrap();
        let repo = DocumentRepository::new(fixture.db_pool().pool());

        // First, we need to ensure the unique_lexemes materialized view exists
        // This would normally be populated by the indexer after processing documents
        sqlx::query(
            r#"
            CREATE MATERIALIZED VIEW IF NOT EXISTS unique_lexemes AS
            SELECT 'document'::text as word, 10 as ndoc, 20 as nentry
            UNION ALL
            SELECT 'search'::text as word, 8 as ndoc, 15 as nentry
            UNION ALL 
            SELECT 'query'::text as word, 5 as ndoc, 10 as nentry
            "#,
        )
        .execute(fixture.db_pool().pool())
        .await
        .unwrap();

        // Test finding similar words with typos
        let similar = repo.find_similar_words("documnt", 2).await.unwrap();
        assert!(!similar.is_empty());
        assert_eq!(similar[0].0, "document");
        assert_eq!(similar[0].1, 1); // Levenshtein distance

        let similar = repo.find_similar_words("serch", 2).await.unwrap();
        assert!(!similar.is_empty());
        assert_eq!(similar[0].0, "search");

        // Test word that's too different
        let similar = repo
            .find_similar_words("completely_different", 2)
            .await
            .unwrap();
        assert!(similar.is_empty());
    }

    #[tokio::test]
    async fn test_search_with_typo_tolerance_no_correction_needed() {
        let fixture = BaseTestFixture::new().await.unwrap();
        let repo = DocumentRepository::new(fixture.db_pool().pool());

        // Create test documents
        let doc = Document {
            id: Ulid::new().to_string(),
            source_id: Ulid::new().to_string(),
            external_id: Ulid::new().to_string(),
            title: "Test document".to_string(),
            content: Some("Test document about search".to_string()),
            content_type: Some("text/plain".to_string()),
            file_size: None,
            file_extension: None,
            url: None,
            parent_id: None,
            metadata: json!({}),
            permissions: json!([]),
            created_at: OffsetDateTime::now_utc(),
            updated_at: OffsetDateTime::now_utc(),
            last_indexed_at: OffsetDateTime::now_utc(),
        };
        repo.create(doc).await.unwrap();

        // Create materialized view with the words
        sqlx::query(
            r#"
            CREATE MATERIALIZED VIEW IF NOT EXISTS unique_lexemes AS
            SELECT word, ndoc, nentry FROM ts_stat('SELECT tsv_content FROM documents')
            "#,
        )
        .execute(fixture.db_pool().pool())
        .await
        .unwrap();

        // Search with correct spelling should not return corrections
        let (results, corrected_query) = repo
            .search_with_typo_tolerance("search", 10, 2, 4)
            .await
            .unwrap();

        assert!(!results.is_empty());
        assert!(corrected_query.is_none());
    }

    #[tokio::test]
    async fn test_search_with_typo_tolerance_with_correction() {
        let fixture = BaseTestFixture::new().await.unwrap();
        let repo = DocumentRepository::new(fixture.db_pool().pool());

        // Create test documents
        for content in &[
            "Document about search functionality",
            "Another document with search terms",
        ] {
            let doc = Document {
                id: Ulid::new().to_string(),
                source_id: Ulid::new().to_string(),
                external_id: Ulid::new().to_string(),
                title: "Test document".to_string(),
                content: Some(content.to_string()),
                content_type: Some("text/plain".to_string()),
                file_size: None,
                file_extension: None,
                url: None,
                parent_id: None,
                metadata: json!({}),
                permissions: json!([]),
                created_at: OffsetDateTime::now_utc(),
                updated_at: OffsetDateTime::now_utc(),
                last_indexed_at: OffsetDateTime::now_utc(),
            };
            repo.create(doc).await.unwrap();
        }

        // Create materialized view
        sqlx::query(
            r#"
            CREATE MATERIALIZED VIEW IF NOT EXISTS unique_lexemes AS
            SELECT word, ndoc, nentry FROM ts_stat('SELECT tsv_content FROM documents')
            WHERE length(word) >= 3
            "#,
        )
        .execute(fixture.db_pool().pool())
        .await
        .unwrap();

        // Search with typo should return corrected query
        let (results, corrected_query) = repo
            .search_with_typo_tolerance("serch functionality", 10, 2, 4)
            .await
            .unwrap();

        assert!(corrected_query.is_some());
        if let Some(corrected) = corrected_query {
            assert!(corrected.contains("search"));
        }
    }

    #[tokio::test]
    async fn test_min_word_length_filtering() {
        let fixture = BaseTestFixture::new().await.unwrap();
        let repo = DocumentRepository::new(fixture.db_pool().pool());

        // Create test document
        let doc = Document {
            id: Ulid::new().to_string(),
            source_id: Ulid::new().to_string(),
            external_id: Ulid::new().to_string(),
            title: "Test document".to_string(),
            content: Some("The cat ran".to_string()),
            content_type: Some("text/plain".to_string()),
            file_size: None,
            file_extension: None,
            url: None,
            parent_id: None,
            metadata: json!({}),
            permissions: json!([]),
            created_at: OffsetDateTime::now_utc(),
            updated_at: OffsetDateTime::now_utc(),
            last_indexed_at: OffsetDateTime::now_utc(),
        };
        repo.create(doc).await.unwrap();

        // Create materialized view
        sqlx::query(
            r#"
            CREATE MATERIALIZED VIEW IF NOT EXISTS unique_lexemes AS
            SELECT word, ndoc, nentry FROM ts_stat('SELECT tsv_content FROM documents')
            "#,
        )
        .execute(fixture.db_pool().pool())
        .await
        .unwrap();

        // Search with short words that have typos - they should not be corrected
        let (_, corrected_query) = repo
            .search_with_typo_tolerance("teh cat ran", 10, 2, 4)
            .await
            .unwrap();

        // "teh" is only 3 characters, so it should not be corrected with min_word_length=4
        assert!(corrected_query.is_none());

        // But with min_word_length=3, it should be corrected
        let (_, corrected_query) = repo
            .search_with_typo_tolerance("teh cat ran", 10, 2, 3)
            .await
            .unwrap();

        assert!(corrected_query.is_some());
    }
}

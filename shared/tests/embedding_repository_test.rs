#[cfg(test)]
mod tests {
    use pgvector::Vector;
    use shared::db::repositories::EmbeddingRepository;
    use shared::models::Embedding;
    use shared::test_utils::BaseTestFixture;
    use sqlx::{PgPool, types::time::OffsetDateTime};
    use ulid::Ulid;

    const TEST_SOURCE_ID: &str = "01JGF7V3E0Y2R1X8P5Q7W9T4N7";

    async fn create_document(pool: &PgPool, doc_id: &str) {
        sqlx::query(
            r#"
            INSERT INTO documents (id, source_id, external_id, title, content, metadata, permissions, attributes, created_at, updated_at)
            VALUES ($1, $2, $3, 'Test Doc', 'content', '{}', '{"users":["u1"]}', '{}', NOW(), NOW())
            "#,
        )
        .bind(doc_id)
        .bind(TEST_SOURCE_ID)
        .bind(format!("ext-{}", doc_id))
        .execute(pool)
        .await
        .unwrap();
    }

    #[tokio::test]
    async fn test_bulk_create_single_document() {
        let fixture = BaseTestFixture::new().await.unwrap();
        let repo = EmbeddingRepository::new(fixture.db_pool().pool());

        // Create embeddings for a single document
        let document_id = Ulid::new().to_string();
        create_document(fixture.db_pool().pool(), &document_id).await;
        let embeddings = vec![
            Embedding {
                id: Ulid::new().to_string(),
                document_id: document_id.clone(),
                chunk_index: 0,
                chunk_start_offset: 0,
                chunk_end_offset: 100,
                embedding: Vector::from(vec![0.1, 0.2, 0.3]),
                model_name: "test-model".to_string(),
                dimensions: 3,
                created_at: OffsetDateTime::now_utc(),
            },
            Embedding {
                id: Ulid::new().to_string(),
                document_id: document_id.clone(),
                chunk_index: 1,
                chunk_start_offset: 100,
                chunk_end_offset: 200,
                embedding: Vector::from(vec![0.4, 0.5, 0.6]),
                model_name: "test-model".to_string(),
                dimensions: 3,
                created_at: OffsetDateTime::now_utc(),
            },
        ];

        // Bulk create embeddings
        repo.bulk_create(embeddings.clone()).await.unwrap();

        // Verify embeddings were created
        let stored_embeddings = repo.find_by_document_id(&document_id).await.unwrap();
        assert_eq!(stored_embeddings.len(), 2);
        assert_eq!(stored_embeddings[0].chunk_index, 0);
        assert_eq!(stored_embeddings[1].chunk_index, 1);
    }

    #[tokio::test]
    async fn test_bulk_create_multiple_documents() {
        let fixture = BaseTestFixture::new().await.unwrap();
        let repo = EmbeddingRepository::new(fixture.db_pool().pool());

        // Create embeddings for multiple documents
        let doc1_id = Ulid::new().to_string();
        let doc2_id = Ulid::new().to_string();
        let doc3_id = Ulid::new().to_string();
        for doc_id in [&doc1_id, &doc2_id, &doc3_id] {
            create_document(fixture.db_pool().pool(), doc_id).await;
        }

        let embeddings = vec![
            // Document 1 - 2 chunks
            Embedding {
                id: Ulid::new().to_string(),
                document_id: doc1_id.clone(),
                chunk_index: 0,
                chunk_start_offset: 0,
                chunk_end_offset: 100,
                embedding: Vector::from(vec![0.1, 0.2, 0.3]),
                model_name: "test-model".to_string(),
                dimensions: 3,
                created_at: OffsetDateTime::now_utc(),
            },
            Embedding {
                id: Ulid::new().to_string(),
                document_id: doc1_id.clone(),
                chunk_index: 1,
                chunk_start_offset: 100,
                chunk_end_offset: 200,
                embedding: Vector::from(vec![0.4, 0.5, 0.6]),
                model_name: "test-model".to_string(),
                dimensions: 3,
                created_at: OffsetDateTime::now_utc(),
            },
            // Document 2 - 3 chunks
            Embedding {
                id: Ulid::new().to_string(),
                document_id: doc2_id.clone(),
                chunk_index: 0,
                chunk_start_offset: 0,
                chunk_end_offset: 150,
                embedding: Vector::from(vec![0.7, 0.8, 0.9]),
                model_name: "test-model".to_string(),
                dimensions: 3,
                created_at: OffsetDateTime::now_utc(),
            },
            Embedding {
                id: Ulid::new().to_string(),
                document_id: doc2_id.clone(),
                chunk_index: 1,
                chunk_start_offset: 150,
                chunk_end_offset: 300,
                embedding: Vector::from(vec![1.0, 1.1, 1.2]),
                model_name: "test-model".to_string(),
                dimensions: 3,
                created_at: OffsetDateTime::now_utc(),
            },
            Embedding {
                id: Ulid::new().to_string(),
                document_id: doc2_id.clone(),
                chunk_index: 2,
                chunk_start_offset: 300,
                chunk_end_offset: 450,
                embedding: Vector::from(vec![1.3, 1.4, 1.5]),
                model_name: "test-model".to_string(),
                dimensions: 3,
                created_at: OffsetDateTime::now_utc(),
            },
            // Document 3 - 1 chunk
            Embedding {
                id: Ulid::new().to_string(),
                document_id: doc3_id.clone(),
                chunk_index: 0,
                chunk_start_offset: 0,
                chunk_end_offset: 50,
                embedding: Vector::from(vec![1.6, 1.7, 1.8]),
                model_name: "test-model".to_string(),
                dimensions: 3,
                created_at: OffsetDateTime::now_utc(),
            },
        ];

        // Bulk create all embeddings in one call
        repo.bulk_create(embeddings.clone()).await.unwrap();

        // Verify embeddings for each document
        let doc1_embeddings = repo.find_by_document_id(&doc1_id).await.unwrap();
        assert_eq!(doc1_embeddings.len(), 2);

        let doc2_embeddings = repo.find_by_document_id(&doc2_id).await.unwrap();
        assert_eq!(doc2_embeddings.len(), 3);

        let doc3_embeddings = repo.find_by_document_id(&doc3_id).await.unwrap();
        assert_eq!(doc3_embeddings.len(), 1);
    }

    #[tokio::test]
    async fn test_bulk_create_with_conflict_resolution() {
        let fixture = BaseTestFixture::new().await.unwrap();
        let repo = EmbeddingRepository::new(fixture.db_pool().pool());

        let document_id = Ulid::new().to_string();
        create_document(fixture.db_pool().pool(), &document_id).await;
        let embedding_id = Ulid::new().to_string();

        // Create initial embedding
        let initial_embedding = Embedding {
            id: embedding_id.clone(),
            document_id: document_id.clone(),
            chunk_index: 0,
            chunk_start_offset: 0,
            chunk_end_offset: 100,
            embedding: Vector::from(vec![0.1, 0.2, 0.3]),
            model_name: "test-model".to_string(),
            dimensions: 3,
            created_at: OffsetDateTime::now_utc(),
        };

        repo.bulk_create(vec![initial_embedding]).await.unwrap();

        // Create conflicting embedding (same document_id, chunk_index, model_name)
        let conflicting_embedding = Embedding {
            id: Ulid::new().to_string(), // Different ID
            document_id: document_id.clone(),
            chunk_index: 0,        // Same chunk_index
            chunk_start_offset: 0, // Different offsets
            chunk_end_offset: 150,
            embedding: Vector::from(vec![0.9, 0.8, 0.7]), // Different embedding
            model_name: "test-model".to_string(),         // Same model_name
            dimensions: 3,
            created_at: OffsetDateTime::now_utc(),
        };

        // This should update the existing embedding due to ON CONFLICT
        repo.bulk_create(vec![conflicting_embedding]).await.unwrap();

        // Verify the embedding was updated, not duplicated
        let stored_embeddings = repo.find_by_document_id(&document_id).await.unwrap();
        assert_eq!(stored_embeddings.len(), 1);
        assert_eq!(stored_embeddings[0].chunk_end_offset, 150); // Updated value
        assert_eq!(
            stored_embeddings[0].embedding,
            Vector::from(vec![0.9, 0.8, 0.7])
        ); // Updated embedding
    }

    #[tokio::test]
    async fn test_bulk_create_empty_input() {
        let fixture = BaseTestFixture::new().await.unwrap();
        let repo = EmbeddingRepository::new(fixture.db_pool().pool());

        // Should handle empty input gracefully
        let result = repo.bulk_create(vec![]).await;
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn test_bulk_create_large_batch() {
        let fixture = BaseTestFixture::new().await.unwrap();
        let repo = EmbeddingRepository::new(fixture.db_pool().pool());

        // Create a large batch of embeddings (simulating real-world bulk processing)
        let mut embeddings = Vec::new();
        let num_docs = 10;
        let chunks_per_doc = 20;

        for doc_idx in 0..num_docs {
            let document_id = Ulid::new().to_string();
            create_document(fixture.db_pool().pool(), &document_id).await;

            for chunk_idx in 0..chunks_per_doc {
                embeddings.push(Embedding {
                    id: Ulid::new().to_string(),
                    document_id: document_id.clone(),
                    chunk_index: chunk_idx,
                    chunk_start_offset: chunk_idx * 100,
                    chunk_end_offset: (chunk_idx + 1) * 100,
                    embedding: Vector::from(vec![
                        doc_idx as f32 * 0.1,
                        chunk_idx as f32 * 0.1,
                        (doc_idx + chunk_idx) as f32 * 0.1,
                    ]),
                    model_name: "test-model".to_string(),
                    dimensions: 3,
                    created_at: OffsetDateTime::now_utc(),
                });
            }
        }

        // Bulk create all embeddings (200 total)
        repo.bulk_create(embeddings.clone()).await.unwrap();

        // Verify total count by checking a few random documents
        let first_doc_id = &embeddings[0].document_id;
        let last_doc_id = &embeddings[embeddings.len() - 1].document_id;

        let first_doc_embeddings = repo.find_by_document_id(first_doc_id).await.unwrap();
        assert_eq!(first_doc_embeddings.len(), chunks_per_doc as usize);

        let last_doc_embeddings = repo.find_by_document_id(last_doc_id).await.unwrap();
        assert_eq!(last_doc_embeddings.len(), chunks_per_doc as usize);
    }

    #[tokio::test]
    async fn test_bulk_delete_by_document_ids() {
        let fixture = BaseTestFixture::new().await.unwrap();
        let repo = EmbeddingRepository::new(fixture.db_pool().pool());

        // Create embeddings for multiple documents
        let doc1_id = Ulid::new().to_string();
        let doc2_id = Ulid::new().to_string();
        let doc3_id = Ulid::new().to_string();
        for doc_id in [&doc1_id, &doc2_id, &doc3_id] {
            create_document(fixture.db_pool().pool(), doc_id).await;
        }

        let embeddings = vec![
            // Document 1 - 2 chunks
            Embedding {
                id: Ulid::new().to_string(),
                document_id: doc1_id.clone(),
                chunk_index: 0,
                chunk_start_offset: 0,
                chunk_end_offset: 100,
                embedding: Vector::from(vec![0.1, 0.2, 0.3]),
                model_name: "test-model".to_string(),
                dimensions: 3,
                created_at: OffsetDateTime::now_utc(),
            },
            Embedding {
                id: Ulid::new().to_string(),
                document_id: doc1_id.clone(),
                chunk_index: 1,
                chunk_start_offset: 100,
                chunk_end_offset: 200,
                embedding: Vector::from(vec![0.4, 0.5, 0.6]),
                model_name: "test-model".to_string(),
                dimensions: 3,
                created_at: OffsetDateTime::now_utc(),
            },
            // Document 2 - 1 chunk
            Embedding {
                id: Ulid::new().to_string(),
                document_id: doc2_id.clone(),
                chunk_index: 0,
                chunk_start_offset: 0,
                chunk_end_offset: 150,
                embedding: Vector::from(vec![0.7, 0.8, 0.9]),
                model_name: "test-model".to_string(),
                dimensions: 3,
                created_at: OffsetDateTime::now_utc(),
            },
            // Document 3 - 1 chunk
            Embedding {
                id: Ulid::new().to_string(),
                document_id: doc3_id.clone(),
                chunk_index: 0,
                chunk_start_offset: 0,
                chunk_end_offset: 50,
                embedding: Vector::from(vec![1.0, 1.1, 1.2]),
                model_name: "test-model".to_string(),
                dimensions: 3,
                created_at: OffsetDateTime::now_utc(),
            },
        ];

        // Create all embeddings
        repo.bulk_create(embeddings.clone()).await.unwrap();

        // Verify all embeddings exist
        let doc1_embeddings = repo.find_by_document_id(&doc1_id).await.unwrap();
        let doc2_embeddings = repo.find_by_document_id(&doc2_id).await.unwrap();
        let doc3_embeddings = repo.find_by_document_id(&doc3_id).await.unwrap();
        assert_eq!(doc1_embeddings.len(), 2);
        assert_eq!(doc2_embeddings.len(), 1);
        assert_eq!(doc3_embeddings.len(), 1);

        // Bulk delete embeddings for doc1 and doc3 (leaving doc2)
        let document_ids_to_delete = vec![doc1_id.clone(), doc3_id.clone()];
        let deleted_count = repo
            .bulk_delete_by_document_ids(&document_ids_to_delete)
            .await
            .unwrap();
        assert_eq!(deleted_count, 3); // 2 from doc1 + 1 from doc3

        // Verify deletions
        let doc1_embeddings_after = repo.find_by_document_id(&doc1_id).await.unwrap();
        let doc2_embeddings_after = repo.find_by_document_id(&doc2_id).await.unwrap();
        let doc3_embeddings_after = repo.find_by_document_id(&doc3_id).await.unwrap();

        assert_eq!(doc1_embeddings_after.len(), 0); // Deleted
        assert_eq!(doc2_embeddings_after.len(), 1); // Preserved
        assert_eq!(doc3_embeddings_after.len(), 0); // Deleted
    }

    #[tokio::test]
    async fn test_bulk_delete_empty_input() {
        let fixture = BaseTestFixture::new().await.unwrap();
        let repo = EmbeddingRepository::new(fixture.db_pool().pool());

        // Should handle empty input gracefully
        let deleted_count = repo.bulk_delete_by_document_ids(&[]).await.unwrap();
        assert_eq!(deleted_count, 0);
    }
}

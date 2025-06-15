use crate::{db::error::DatabaseError, models::Document};
use sqlx::PgPool;

pub struct DocumentRepository {
    pool: PgPool,
}

impl DocumentRepository {
    pub fn new(pool: &PgPool) -> Self {
        Self { pool: pool.clone() }
    }

    pub async fn find_by_id(&self, id: &str) -> Result<Option<Document>, DatabaseError> {
        let document = sqlx::query_as::<_, Document>(
            r#"
            SELECT id, source_id, external_id, title, content, content_type,
                   file_size, file_extension, url, parent_id,
                   metadata, permissions, created_at, updated_at, last_indexed_at
            FROM documents
            WHERE id = $1
            "#,
        )
        .bind(id)
        .fetch_optional(&self.pool)
        .await?;

        Ok(document)
    }

    pub async fn find_all(&self, limit: i64, offset: i64) -> Result<Vec<Document>, DatabaseError> {
        let documents = sqlx::query_as::<_, Document>(
            r#"
            SELECT id, source_id, external_id, title, content, content_type,
                   file_size, file_extension, url, parent_id,
                   metadata, permissions, created_at, updated_at, last_indexed_at
            FROM documents
            ORDER BY created_at DESC
            LIMIT $1 OFFSET $2
            "#,
        )
        .bind(limit)
        .bind(offset)
        .fetch_all(&self.pool)
        .await?;

        Ok(documents)
    }

    pub async fn search(&self, query: &str, limit: i64) -> Result<Vec<Document>, DatabaseError> {
        let documents = sqlx::query_as::<_, Document>(
            r#"
            SELECT id, source_id, external_id, title, content, content_type,
                   file_size, file_extension, url, parent_id,
                   metadata, permissions, created_at, updated_at, last_indexed_at
            FROM documents
            WHERE tsv_content @@ websearch_to_tsquery('english', $1)
            ORDER BY ts_rank(tsv_content, websearch_to_tsquery('english', $1)) DESC
            LIMIT $2
            "#,
        )
        .bind(query)
        .bind(limit)
        .fetch_all(&self.pool)
        .await?;

        Ok(documents)
    }

    pub async fn find_similar_words(
        &self,
        word: &str,
        max_distance: i32,
    ) -> Result<Vec<(String, i32)>, DatabaseError> {
        let similar_words = sqlx::query_as::<_, (String, i32)>(
            r#"
            SELECT word, levenshtein_less_equal(lower(word), lower($1), $2) as distance
            FROM unique_lexemes
            WHERE levenshtein_less_equal(lower(word), lower($1), $2) < $2
              AND length(word) >= 3
            ORDER BY distance, ndoc DESC
            LIMIT 5
            "#,
        )
        .bind(word)
        .bind(max_distance)
        .fetch_all(&self.pool)
        .await?;

        Ok(similar_words)
    }

    pub async fn search_with_typo_tolerance(
        &self,
        query: &str,
        limit: i64,
        max_distance: i32,
        min_word_length: usize,
    ) -> Result<(Vec<Document>, Option<String>), DatabaseError> {
        // First, try to search with the original query
        let original_results = self.search(query, limit).await?;

        // If we get reasonable results, return them without correction
        if !original_results.is_empty() && original_results.len() >= (limit / 2) as usize {
            return Ok((original_results, None));
        }

        // Tokenize the query
        let words: Vec<&str> = query.split_whitespace().collect();
        let mut corrected_words = Vec::new();
        let mut any_correction_made = false;

        // For each word, check if it exists in our lexeme dictionary
        for word in words {
            // Skip very short words
            if word.len() < min_word_length {
                corrected_words.push(word.to_string());
                continue;
            }

            // Check if the word exists in our lexeme dictionary
            let exists = sqlx::query_scalar::<_, bool>(
                "SELECT EXISTS(SELECT 1 FROM unique_lexemes WHERE lower(word) = lower($1))",
            )
            .bind(word)
            .fetch_one(&self.pool)
            .await?;

            if exists {
                corrected_words.push(word.to_string());
            } else {
                // Find similar words
                let similar = self.find_similar_words(word, max_distance).await?;

                if let Some((corrected_word, _)) = similar.first() {
                    corrected_words.push(corrected_word.clone());
                    any_correction_made = true;
                } else {
                    // No correction found, use original word
                    corrected_words.push(word.to_string());
                }
            }
        }

        // If no corrections were made, return original results
        if !any_correction_made {
            return Ok((original_results, None));
        }

        // Construct corrected query
        let corrected_query = corrected_words.join(" ");

        // Search with corrected query
        let corrected_results = self.search(&corrected_query, limit).await?;

        // Return the better result set
        if corrected_results.len() > original_results.len() {
            Ok((corrected_results, Some(corrected_query)))
        } else {
            Ok((original_results, None))
        }
    }

    pub async fn find_by_source(&self, source_id: &str) -> Result<Vec<Document>, DatabaseError> {
        let documents = sqlx::query_as::<_, Document>(
            r#"
            SELECT id, source_id, external_id, title, content, content_type,
                   file_size, file_extension, url, parent_id,
                   metadata, permissions, created_at, updated_at, last_indexed_at
            FROM documents
            WHERE source_id = $1
            ORDER BY created_at DESC
            "#,
        )
        .bind(source_id)
        .fetch_all(&self.pool)
        .await?;

        Ok(documents)
    }

    pub async fn find_by_external_id(
        &self,
        source_id: &str,
        external_id: &str,
    ) -> Result<Option<Document>, DatabaseError> {
        let document = sqlx::query_as::<_, Document>(
            r#"
            SELECT id, source_id, external_id, title, content, content_type,
                   file_size, file_extension, url, parent_id,
                   metadata, permissions, created_at, updated_at, last_indexed_at
            FROM documents
            WHERE source_id = $1 AND external_id = $2
            "#,
        )
        .bind(source_id)
        .bind(external_id)
        .fetch_optional(&self.pool)
        .await?;

        Ok(document)
    }

    pub async fn create(&self, document: Document) -> Result<Document, DatabaseError> {
        let created_document = sqlx::query_as::<_, Document>(
            r#"
            INSERT INTO documents (id, source_id, external_id, title, content, metadata, permissions)
            VALUES ($1, $2, $3, $4, $5, $6, $7)
            RETURNING id, source_id, external_id, title, content, content_type,
                      file_size, file_extension, url, parent_id,
                      metadata, permissions, created_at, updated_at, last_indexed_at
            "#
        )
        .bind(&document.id)
        .bind(&document.source_id)
        .bind(&document.external_id)
        .bind(&document.title)
        .bind(&document.content)
        .bind(&document.metadata)
        .bind(&document.permissions)
        .fetch_one(&self.pool)
        .await
        .map_err(|e| match e {
            sqlx::Error::Database(db_err) if db_err.is_unique_violation() => {
                DatabaseError::ConstraintViolation("Document with this external_id already exists for this source".to_string())
            }
            _ => DatabaseError::from(e),
        })?;

        Ok(created_document)
    }

    pub async fn update(
        &self,
        id: &str,
        document: Document,
    ) -> Result<Option<Document>, DatabaseError> {
        let updated_document = sqlx::query_as::<_, Document>(
            r#"
            UPDATE documents
            SET title = $2, content = $3, metadata = $4, permissions = $5
            WHERE id = $1
            RETURNING id, source_id, external_id, title, content, content_type,
                      file_size, file_extension, url, parent_id,
                      metadata, permissions, created_at, updated_at, last_indexed_at
            "#,
        )
        .bind(id)
        .bind(&document.title)
        .bind(&document.content)
        .bind(&document.metadata)
        .bind(&document.permissions)
        .fetch_optional(&self.pool)
        .await?;

        Ok(updated_document)
    }

    pub async fn delete(&self, id: &str) -> Result<bool, DatabaseError> {
        let result = sqlx::query("DELETE FROM documents WHERE id = $1")
            .bind(id)
            .execute(&self.pool)
            .await?;

        Ok(result.rows_affected() > 0)
    }

    pub async fn update_search_vector(&self, _id: &str) -> Result<(), DatabaseError> {
        // The tsv_content column is automatically generated, so this method is now a no-op
        // We keep it for compatibility but it doesn't need to do anything
        Ok(())
    }

    pub async fn mark_as_indexed(&self, id: &str) -> Result<(), DatabaseError> {
        sqlx::query("UPDATE documents SET last_indexed_at = CURRENT_TIMESTAMP WHERE id = $1")
            .bind(id)
            .execute(&self.pool)
            .await?;

        Ok(())
    }

    pub async fn upsert(&self, document: Document) -> Result<Document, DatabaseError> {
        let upserted_document = sqlx::query_as::<_, Document>(
            r#"
            INSERT INTO documents (id, source_id, external_id, title, content, content_type, file_size, file_extension, url, parent_id, metadata, permissions, created_at, updated_at, last_indexed_at)
            VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13, $14, $15)
            ON CONFLICT (source_id, external_id)
            DO UPDATE SET
                title = EXCLUDED.title,
                content = EXCLUDED.content,
                metadata = EXCLUDED.metadata,
                permissions = EXCLUDED.permissions,
                updated_at = EXCLUDED.updated_at,
                last_indexed_at = EXCLUDED.last_indexed_at
            RETURNING id, source_id, external_id, title, content, content_type,
                      file_size, file_extension, url, parent_id,
                      metadata, permissions, created_at, updated_at, last_indexed_at
            "#
        )
        .bind(&document.id)
        .bind(&document.source_id)
        .bind(&document.external_id)
        .bind(&document.title)
        .bind(&document.content)
        .bind(&document.content_type)
        .bind(&document.file_size)
        .bind(&document.file_extension)
        .bind(&document.url)
        .bind(&document.parent_id)
        .bind(&document.metadata)
        .bind(&document.permissions)
        .bind(&document.created_at)
        .bind(&document.updated_at)
        .bind(&document.last_indexed_at)
        .fetch_one(&self.pool)
        .await?;

        Ok(upserted_document)
    }
}

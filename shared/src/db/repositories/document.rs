use crate::{
    db::error::DatabaseError,
    models::{Document, Facet, FacetValue},
};
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
            ORDER BY (
                -- Base FTS ranking with custom weights (D=0.1, C=0.2, B=0.4, A=1.0)
                ts_rank_cd('{0.1, 0.2, 0.4, 1.0}', tsv_content, websearch_to_tsquery('english', $1)) *
                -- Recency boost: newer documents get slight boost (max 30% boost for very recent)
                (1.0 + GREATEST(-1.0, (EXTRACT(EPOCH FROM (NOW() - updated_at)) / 86400.0 / -365.0)) * 0.3) *
                -- Document type boost based on actual Google Drive content types
                CASE 
                    WHEN content_type = 'application/vnd.google-apps.document' THEN 1.3  -- Google Docs highest
                    WHEN content_type = 'application/vnd.google-apps.spreadsheet' THEN 1.2  -- Google Sheets
                    WHEN content_type = 'application/pdf' THEN 1.2  -- PDFs are valuable
                    WHEN content_type = 'text/html' THEN 1.1  -- HTML content
                    WHEN content_type = 'text/plain' THEN 1.0  -- Plain text baseline
                    WHEN content_type = 'text/csv' THEN 0.9  -- CSVs less searchable
                    ELSE 1.0  -- Default for unknown types
                END *
                -- Title exact match boost (case-insensitive partial match)
                CASE 
                    WHEN title ILIKE '%' || $1 || '%' THEN 1.4
                    ELSE 1.0
                END
            ) DESC
            LIMIT $2
            "#,
        )
        .bind(query)
        .bind(limit)
        .fetch_all(&self.pool)
        .await?;

        Ok(documents)
    }

    pub async fn search_with_filters(
        &self,
        query: &str,
        sources: Option<&[String]>,
        content_types: Option<&[String]>,
        limit: i64,
        offset: i64,
    ) -> Result<Vec<Document>, DatabaseError> {
        let base_query = r#"
            SELECT id, source_id, external_id, title, content, content_type,
                   file_size, file_extension, url, parent_id,
                   metadata, permissions, created_at, updated_at, last_indexed_at
            FROM documents
            WHERE tsv_content @@ websearch_to_tsquery('english', $1)
        "#;

        let has_sources = sources.map_or(false, |s| !s.is_empty());
        let has_content_types = content_types.map_or(false, |ct| !ct.is_empty());

        let source_filter = if has_sources {
            " AND source_id = ANY($2)"
        } else {
            ""
        };

        let content_type_filter = if has_content_types {
            if has_sources {
                " AND content_type = ANY($3)"
            } else {
                " AND content_type = ANY($2)"
            }
        } else {
            ""
        };

        let (limit_param, offset_param) = match (has_sources, has_content_types) {
            (true, true) => ("$4", "$5"),
            (true, false) | (false, true) => ("$3", "$4"),
            (false, false) => ("$2", "$3"),
        };

        let full_query = format!(
            r#"{}{}{} ORDER BY (
                -- Base FTS ranking with custom weights (D=0.1, C=0.2, B=0.4, A=1.0)
                ts_rank_cd('{{0.1, 0.2, 0.4, 1.0}}', tsv_content, websearch_to_tsquery('english', $1)) *
                -- Recency boost: newer documents get slight boost (max 30% boost for very recent)
                (1.0 + GREATEST(-1.0, (EXTRACT(EPOCH FROM (NOW() - updated_at)) / 86400.0 / -365.0)) * 0.3) *
                -- Document type boost based on actual Google Drive content types
                CASE 
                    WHEN content_type = 'application/vnd.google-apps.document' THEN 1.3
                    WHEN content_type = 'application/vnd.google-apps.spreadsheet' THEN 1.2
                    WHEN content_type = 'application/pdf' THEN 1.2
                    WHEN content_type = 'text/html' THEN 1.1
                    WHEN content_type = 'text/plain' THEN 1.0
                    WHEN content_type = 'text/csv' THEN 0.9
                    ELSE 1.0
                END *
                -- Title exact match boost
                CASE 
                    WHEN title ILIKE '%' || $1 || '%' THEN 1.4
                    ELSE 1.0
                END
            ) DESC LIMIT {} OFFSET {}"#,
            base_query, source_filter, content_type_filter, limit_param, offset_param
        );

        let mut query = sqlx::query_as::<_, Document>(&full_query).bind(query);

        if let Some(src) = sources {
            if !src.is_empty() {
                query = query.bind(src);
            }
        }

        if let Some(ct) = content_types {
            if !ct.is_empty() {
                query = query.bind(ct);
            }
        }

        query = query.bind(limit).bind(offset);

        let documents = query.fetch_all(&self.pool).await?;

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

    pub async fn search_with_typo_tolerance_and_filters(
        &self,
        query: &str,
        sources: Option<&[String]>,
        content_types: Option<&[String]>,
        limit: i64,
        offset: i64,
        max_distance: i32,
        min_word_length: usize,
    ) -> Result<(Vec<Document>, Option<String>), DatabaseError> {
        // First, try to search with the original query
        let original_results = self
            .search_with_filters(query, sources, content_types, limit, offset)
            .await?;

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
        let corrected_results = self
            .search_with_filters(&corrected_query, sources, content_types, limit, offset)
            .await?;

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

    pub async fn get_facet_counts(&self, query: &str) -> Result<Vec<Facet>, DatabaseError> {
        let facet_rows = sqlx::query_as::<_, (String, String, i64)>(
            r#"
            SELECT 'source_type' as facet, s.source_type as value, count(*) as count
            FROM documents d 
            JOIN sources s ON d.source_id = s.id
            WHERE d.tsv_content @@ websearch_to_tsquery('english', $1::text)
            GROUP BY s.source_type 
            ORDER BY count DESC
            "#,
        )
        .bind(query)
        .fetch_all(&self.pool)
        .await?;

        // Group the results by facet name
        let mut facets_map: std::collections::HashMap<String, Vec<FacetValue>> =
            std::collections::HashMap::new();

        for (facet_name, value, count) in facet_rows {
            facets_map
                .entry(facet_name)
                .or_insert_with(Vec::new)
                .push(FacetValue { value, count });
        }

        // Convert to Vec<Facet>
        let facets: Vec<Facet> = facets_map
            .into_iter()
            .map(|(name, values)| Facet { name, values })
            .collect();

        Ok(facets)
    }

    pub async fn get_facet_counts_with_filters(
        &self,
        query: &str,
        sources: Option<&[String]>,
        content_types: Option<&[String]>,
    ) -> Result<Vec<Facet>, DatabaseError> {
        let mut where_conditions =
            vec!["d.tsv_content @@ websearch_to_tsquery('english', $1::text)".to_string()];
        let mut bind_index = 2;

        if let Some(src) = sources {
            if !src.is_empty() {
                where_conditions.push(format!("d.source_id = ANY(${})", bind_index));
                bind_index += 1;
            }
        }

        if let Some(ct) = content_types {
            if !ct.is_empty() {
                where_conditions.push(format!("d.content_type = ANY(${})", bind_index));
            }
        }

        let where_clause = where_conditions.join(" AND ");

        let query_str = format!(
            r#"
            SELECT 'source_type' as facet, s.source_type as value, count(*) as count
            FROM documents d 
            JOIN sources s ON d.source_id = s.id
            WHERE {}
            GROUP BY s.source_type 
            ORDER BY count DESC
            "#,
            where_clause
        );

        let mut query = sqlx::query_as::<_, (String, String, i64)>(&query_str).bind(query);

        if let Some(src) = sources {
            if !src.is_empty() {
                query = query.bind(src);
            }
        }

        if let Some(ct) = content_types {
            if !ct.is_empty() {
                query = query.bind(ct);
            }
        }

        let facet_rows = query.fetch_all(&self.pool).await?;

        // Group the results by facet name
        let mut facets_map: std::collections::HashMap<String, Vec<FacetValue>> =
            std::collections::HashMap::new();

        for (facet_name, value, count) in facet_rows {
            facets_map
                .entry(facet_name)
                .or_insert_with(Vec::new)
                .push(FacetValue { value, count });
        }

        // Convert to Vec<Facet>
        let facets: Vec<Facet> = facets_map
            .into_iter()
            .map(|(name, values)| Facet { name, values })
            .collect();

        Ok(facets)
    }
}

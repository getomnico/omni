use crate::{
    db::error::DatabaseError,
    models::{Document, Facet, FacetValue},
    SourceType,
};
use sqlx::{FromRow, PgPool};
use tracing::debug;

#[derive(FromRow)]
pub struct DocumentWithScores {
    #[sqlx(flatten)]
    pub document: Document,
    pub score: f32,
    #[sqlx(default)]
    pub base_rank: f32,
    #[sqlx(default)]
    pub recency_boost: f32,
    #[sqlx(default)]
    pub title_boost: f32,
}

pub struct DocumentRepository {
    pool: PgPool,
}

impl DocumentRepository {
    pub fn new(pool: &PgPool) -> Self {
        Self { pool: pool.clone() }
    }

    /// Generate SQL condition to check if user has permission to access document
    fn generate_permission_filter(&self, user_email: &str) -> String {
        format!(
            r#"(
                id @@@ paradedb.term('permissions.public', true) OR
                id @@@ paradedb.term('permissions.users', '{}') OR
                id @@@ paradedb.term('permissions.groups', '{}') -- TODO: Add group membership lookup here
            )"#,
            user_email, user_email
        )
    }

    pub async fn find_by_id(&self, id: &str) -> Result<Option<Document>, DatabaseError> {
        let document = sqlx::query_as::<_, Document>(
            r#"
            SELECT id, source_id, external_id, title, content_id, content_type,
                   file_size, file_extension, url,
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

    pub async fn find_by_ids(&self, ids: &[String]) -> Result<Vec<Document>, DatabaseError> {
        if ids.is_empty() {
            return Ok(vec![]);
        }

        let documents = sqlx::query_as::<_, Document>(
            r#"
            SELECT id, source_id, external_id, title, content_id, content_type,
                   file_size, file_extension, url,
                   metadata, permissions, created_at, updated_at, last_indexed_at
            FROM documents
            WHERE id = ANY($1)
            "#,
        )
        .bind(ids)
        .fetch_all(&self.pool)
        .await?;

        Ok(documents)
    }

    pub async fn find_all(&self, limit: i64, offset: i64) -> Result<Vec<Document>, DatabaseError> {
        let documents = sqlx::query_as::<_, Document>(
            r#"
            SELECT id, source_id, external_id, title, content_id, content_type,
                   file_size, file_extension, url,
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

    pub async fn fetch_random_documents(
        &self,
        user_email: &str,
        count: usize,
    ) -> Result<Vec<Document>, DatabaseError> {
        let permission_filter = &self.generate_permission_filter(user_email);

        let query = format!(
            r#"
            SELECT *
            FROM documents d
            WHERE d.content_id IS NOT NULL
                AND {}
            ORDER BY RANDOM()
            LIMIT $1
        "#,
            permission_filter
        );

        let documents = sqlx::query_as::<_, Document>(&query)
            .bind(count as i32)
            .fetch_all(&self.pool)
            .await?;

        Ok(documents)
    }

    pub async fn search(&self, query: &str, limit: i64) -> Result<Vec<Document>, DatabaseError> {
        let documents = sqlx::query_as::<_, Document>(
            r#"
            SELECT id, source_id, external_id, title, content_id, content_type,
                   file_size, file_extension, url,
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
        source_types: Option<&[SourceType]>,
        content_types: Option<&[String]>,
        limit: i64,
        offset: i64,
        user_email: Option<&str>,
    ) -> Result<Vec<DocumentWithScores>, DatabaseError> {
        let has_sources = source_types.map_or(false, |s| !s.is_empty());
        let has_content_types = content_types.map_or(false, |ct| !ct.is_empty());

        let mut filters = vec!["(title @@@ $1 OR content @@@ $2)".to_string()];
        let mut param_idx = 3; // Params 1 and 2 are for the query (title query and content query)

        if has_sources {
            filters.push(format!(
                "source_id IN (SELECT id FROM sources WHERE source_type = ANY(${}))",
                param_idx
            ));
            param_idx += 1;
        }

        if has_content_types {
            filters.push(format!("content_type = ANY(${})", param_idx));
            param_idx += 1;
        }

        if let Some(email) = user_email {
            filters.push(self.generate_permission_filter(email));
        }

        let where_clause = filters.join(" AND ");

        let full_query = format!(
            r#"
            SELECT id, source_id, external_id, title, content_id, content_type,
                   file_size, file_extension, url,
                   metadata, permissions, created_at, updated_at, last_indexed_at,
                   paradedb.score(id) as score
            FROM documents
            WHERE {}
            ORDER BY score DESC
            LIMIT ${} OFFSET ${}"#,
            where_clause,
            param_idx,
            param_idx + 1
        );

        let title_query = format!("{}^2", query);
        let mut query = sqlx::query_as::<_, DocumentWithScores>(&full_query)
            .bind(title_query)
            .bind(query);

        if let Some(src) = source_types {
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

        let results = query.fetch_all(&self.pool).await?;

        Ok(results)
    }

    #[deprecated(note = "Use search_with_filters instead")]
    pub async fn search_with_filters_v0(
        &self,
        query: &str,
        source_types: Option<&[SourceType]>,
        content_types: Option<&[String]>,
        limit: i64,
        offset: i64,
        user_email: Option<&str>,
    ) -> Result<Vec<DocumentWithScores>, DatabaseError> {
        let has_sources = source_types.map_or(false, |s| !s.is_empty());
        let has_content_types = content_types.map_or(false, |ct| !ct.is_empty());
        let has_user_filter = user_email.is_some();

        let source_filter = if has_sources {
            " AND source_id IN (SELECT id FROM sources WHERE source_type = ANY($2))"
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

        let permission_filter = if has_user_filter {
            if let Some(email) = user_email {
                format!(" AND {}", self.generate_permission_filter(email))
            } else {
                "".to_string()
            }
        } else {
            "".to_string()
        };

        let (limit_param, offset_param) = match (has_sources, has_content_types) {
            (true, true) => ("$4", "$5"),
            (true, false) | (false, true) => ("$3", "$4"),
            (false, false) => ("$2", "$3"),
        };

        let full_query = format!(
            r#"
            WITH candidates AS (
                SELECT *
                FROM documents
                WHERE tsv_content @@ websearch_to_tsquery('english', $1)
                {}{}{}
                LIMIT 10000
            )
            SELECT id, source_id, external_id, title, content_id, content_type,
                   file_size, file_extension, url,
                   metadata, permissions, created_at, updated_at, last_indexed_at,
                   ts_rank_cd('{{0.1, 0.2, 0.4, 1.0}}', tsv_content, websearch_to_tsquery('english', $1), 32) as base_rank,
                   (1.0 + GREATEST(-1.0, (EXTRACT(EPOCH FROM (NOW() - (regexp_replace(metadata->>'updated_at', '^\+00', ''))::timestamptz)) / 86400.0 / -365.0)) * 0.3)::real as recency_boost,
                   (CASE WHEN title ILIKE '%' || $1 || '%' THEN 1.4 ELSE 1.0 END)::real as title_boost,
                   -- Base FTS ranking with custom weights (D=0.1, C=0.2, B=0.4, A=1.0)
                   (ts_rank_cd('{{0.1, 0.2, 0.4, 1.0}}', tsv_content, websearch_to_tsquery('english', $1), 32) *
                   -- Recency boost: newer documents get slight boost (max 30% boost for very recent)
                   (1.0 + GREATEST(-1.0, (EXTRACT(EPOCH FROM (NOW() - (regexp_replace(metadata->>'updated_at', '^\+00', ''))::timestamptz)) / 86400.0 / -365.0)) * 0.3) *
                   -- Title exact match boost
                   CASE
                       WHEN title ILIKE '%' || $1 || '%' THEN 1.4
                       ELSE 1.0
                   END)::real as score
            FROM candidates
            ORDER BY score DESC
            LIMIT {} OFFSET {}"#,
            source_filter, content_type_filter, permission_filter, limit_param, offset_param
        );

        let mut query = sqlx::query_as::<_, DocumentWithScores>(&full_query).bind(query);

        if let Some(src) = source_types {
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

        let results = query.fetch_all(&self.pool).await?;

        Ok(results)
    }

    pub async fn find_by_source(&self, source_id: &str) -> Result<Vec<Document>, DatabaseError> {
        let documents = sqlx::query_as::<_, Document>(
            r#"
            SELECT id, source_id, external_id, title, content_id, content_type,
                   file_size, file_extension, url,
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
            SELECT id, source_id, external_id, title, content_id, content_type,
                   file_size, file_extension, url,
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
            INSERT INTO documents (id, source_id, external_id, title, content_id, metadata, permissions)
            VALUES ($1, $2, $3, $4, $5, $6, $7)
            RETURNING id, source_id, external_id, title, content_id, content_type,
                      file_size, file_extension, url,
                      metadata, permissions, created_at, updated_at, last_indexed_at
            "#
        )
        .bind(&document.id)
        .bind(&document.source_id)
        .bind(&document.external_id)
        .bind(&document.title)
        .bind(&document.content_id)
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

    /// Directly populates content to use the BM25 index
    pub async fn update(
        &self,
        id: &str,
        document: Document,
        content: &str,
    ) -> Result<Option<Document>, DatabaseError> {
        let updated_document = sqlx::query_as::<_, Document>(
            r#"
            UPDATE documents
            SET 
                title = $2, 
                content_id = $3, 
                metadata = $4, 
                permissions = $5,
                content = $6
            WHERE id = $1
            RETURNING id, source_id, external_id, title, content_id, content_type,
                      file_size, file_extension, url,
                      metadata, permissions, created_at, updated_at, last_indexed_at
            "#,
        )
        .bind(id)
        .bind(&document.title)
        .bind(&document.content_id)
        .bind(&document.metadata)
        .bind(&document.permissions)
        .bind(content)
        .fetch_optional(&self.pool)
        .await?;

        Ok(updated_document)
    }

    #[deprecated(note = "Use update instead.")]
    pub async fn update_v0(
        &self,
        id: &str,
        document: Document,
        content: &str,
    ) -> Result<Option<Document>, DatabaseError> {
        let updated_document = sqlx::query_as::<_, Document>(
            r#"
            UPDATE documents
            SET title = $2, content_id = $3, metadata = $4, permissions = $5,
                tsv_content = setweight(to_tsvector('english', $2), 'A') || setweight(to_tsvector('english', $6), 'B')
            WHERE id = $1
            RETURNING id, source_id, external_id, title, content_id, content_type,
                      file_size, file_extension, url,
                      metadata, permissions, created_at, updated_at, last_indexed_at
            "#,
        )
        .bind(id)
        .bind(&document.title)
        .bind(&document.content_id)
        .bind(&document.metadata)
        .bind(&document.permissions)
        .bind(content)
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

    pub async fn upsert(
        &self,
        document: Document,
        content: &str,
    ) -> Result<Document, DatabaseError> {
        let upserted_document = sqlx::query_as::<_, Document>(
            r#"
            INSERT INTO documents (id, source_id, external_id, title, content_id, content_type, file_size, file_extension, url, metadata, permissions, created_at, updated_at, last_indexed_at, tsv_content)
            VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13, $14,
                    setweight(to_tsvector('english', $4), 'A') || setweight(to_tsvector('english', $15), 'B'))
            ON CONFLICT (source_id, external_id)
            DO UPDATE SET
                title = EXCLUDED.title,
                content_id = EXCLUDED.content_id,
                metadata = EXCLUDED.metadata,
                permissions = EXCLUDED.permissions,
                updated_at = EXCLUDED.updated_at,
                last_indexed_at = EXCLUDED.last_indexed_at,
                tsv_content = setweight(to_tsvector('english', EXCLUDED.title), 'A') || setweight(to_tsvector('english', $15), 'B')
            RETURNING id, source_id, external_id, title, content_id, content_type,
                      file_size, file_extension, url,
                      metadata, permissions, created_at, updated_at, last_indexed_at
            "#
        )
        .bind(&document.id)
        .bind(&document.source_id)
        .bind(&document.external_id)
        .bind(&document.title)
        .bind(&document.content_id)
        .bind(&document.content_type)
        .bind(&document.file_size)
        .bind(&document.file_extension)
        .bind(&document.url)
        .bind(&document.metadata)
        .bind(&document.permissions)
        .bind(&document.created_at)
        .bind(&document.updated_at)
        .bind(&document.last_indexed_at)
        .bind(content)
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
        source_types: Option<&[SourceType]>,
        content_types: Option<&[String]>,
    ) -> Result<Vec<Facet>, DatabaseError> {
        let mut filters = vec!["(title @@@ $1 OR content @@@ $2)".to_string()];
        let mut bind_index = 3;

        if let Some(src) = source_types {
            if !src.is_empty() {
                filters.push(format!("s.source_type = ANY(${})", bind_index));
                bind_index += 1;
            }
        }

        if let Some(ct) = content_types {
            if !ct.is_empty() {
                filters.push(format!("d.content_type = ANY(${})", bind_index));
            }
        }

        let where_clause = filters.join(" AND ");

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

        let title_query = format!("{}^2", query);
        let mut query = sqlx::query_as::<_, (String, String, i64)>(&query_str)
            .bind(title_query)
            .bind(query);

        if let Some(src) = source_types {
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

    #[deprecated(note = "Use get_facet_counts_with_filters instead")]
    pub async fn get_facet_counts_with_filters_v0(
        &self,
        query: &str,
        source_types: Option<&[SourceType]>,
        content_types: Option<&[String]>,
    ) -> Result<Vec<Facet>, DatabaseError> {
        let mut where_conditions =
            vec!["d.tsv_content @@ websearch_to_tsquery('english', $1::text)".to_string()];
        let mut bind_index = 2;

        if let Some(src) = source_types {
            if !src.is_empty() {
                where_conditions.push(format!("s.source_type = ANY(${})", bind_index));
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

        if let Some(src) = source_types {
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

    /// Directly populates the content field since we use the ParadeDB BM25 index now
    pub async fn batch_upsert(
        &self,
        documents: Vec<Document>,
        contents: Vec<String>,
    ) -> Result<Vec<Document>, DatabaseError> {
        if documents.is_empty() {
            return Ok(vec![]);
        }

        // Build arrays for the batch upsert
        let ids: Vec<String> = documents.iter().map(|d| d.id.clone()).collect();
        let source_ids: Vec<String> = documents.iter().map(|d| d.source_id.clone()).collect();
        let external_ids: Vec<String> = documents.iter().map(|d| d.external_id.clone()).collect();
        let titles: Vec<String> = documents.iter().map(|d| d.title.clone()).collect();
        let content_ids: Vec<Option<String>> =
            documents.iter().map(|d| d.content_id.clone()).collect();
        let content_types: Vec<Option<String>> =
            documents.iter().map(|d| d.content_type.clone()).collect();
        let file_sizes: Vec<Option<i64>> = documents.iter().map(|d| d.file_size).collect();
        let file_extensions: Vec<Option<String>> =
            documents.iter().map(|d| d.file_extension.clone()).collect();
        let urls: Vec<Option<String>> = documents.iter().map(|d| d.url.clone()).collect();
        let metadata: Vec<serde_json::Value> =
            documents.iter().map(|d| d.metadata.clone()).collect();
        let permissions: Vec<serde_json::Value> =
            documents.iter().map(|d| d.permissions.clone()).collect();
        let created_ats: Vec<sqlx::types::time::OffsetDateTime> =
            documents.iter().map(|d| d.created_at).collect();
        let updated_ats: Vec<sqlx::types::time::OffsetDateTime> =
            documents.iter().map(|d| d.updated_at).collect();
        let last_indexed_ats: Vec<sqlx::types::time::OffsetDateTime> =
            documents.iter().map(|d| d.last_indexed_at).collect();

        let upserted_documents = sqlx::query_as::<_, Document>(
            r#"
            INSERT INTO documents (
                id, 
                source_id, 
                external_id, 
                title, 
                content_id, 
                content_type, 
                file_size, 
                file_extension, 
                url, 
                metadata, 
                permissions, 
                created_at, 
                updated_at, 
                last_indexed_at, 
                content
            )
            SELECT *
            FROM UNNEST(
                $1::text[], $2::text[], $3::text[], $4::text[], $5::text[], $6::text[],
                $7::bigint[], $8::text[], $9::text[], $10::jsonb[], $11::jsonb[],
                $12::timestamptz[], $13::timestamptz[], $14::timestamptz[], $15::text[]
            ) AS t(id, source_id, external_id, title, content_id, content_type, file_size, file_extension, url, metadata, permissions, created_at, updated_at, last_indexed_at, content)
            ON CONFLICT (source_id, external_id)
            DO UPDATE SET
                title = EXCLUDED.title,
                content_id = EXCLUDED.content_id,
                metadata = EXCLUDED.metadata,
                permissions = EXCLUDED.permissions,
                updated_at = EXCLUDED.updated_at,
                last_indexed_at = EXCLUDED.last_indexed_at,
                content = EXCLUDED.content
            RETURNING id, source_id, external_id, title, content_id, content_type,
                      file_size, file_extension, url,
                      metadata, permissions, created_at, updated_at, last_indexed_at
            "#
        )
        .bind(&ids)
        .bind(&source_ids)
        .bind(&external_ids)
        .bind(&titles)
        .bind(&content_ids)
        .bind(&content_types)
        .bind(&file_sizes)
        .bind(&file_extensions)
        .bind(&urls)
        .bind(&metadata)
        .bind(&permissions)
        .bind(&created_ats)
        .bind(&updated_ats)
        .bind(&last_indexed_ats)
        .bind(&contents)
        .fetch_all(&self.pool)
        .await?;

        Ok(upserted_documents)
    }

    // Batch operations for improved performance
    #[deprecated(note = "Use batch_upsert instead.")]
    pub async fn batch_upsert_v0(
        &self,
        documents: Vec<Document>,
        contents: Vec<String>,
    ) -> Result<Vec<Document>, DatabaseError> {
        if documents.is_empty() {
            return Ok(vec![]);
        }

        // Build arrays for the batch upsert
        let ids: Vec<String> = documents.iter().map(|d| d.id.clone()).collect();
        let source_ids: Vec<String> = documents.iter().map(|d| d.source_id.clone()).collect();
        let external_ids: Vec<String> = documents.iter().map(|d| d.external_id.clone()).collect();
        let titles: Vec<String> = documents.iter().map(|d| d.title.clone()).collect();
        let content_ids: Vec<Option<String>> =
            documents.iter().map(|d| d.content_id.clone()).collect();
        let content_types: Vec<Option<String>> =
            documents.iter().map(|d| d.content_type.clone()).collect();
        let file_sizes: Vec<Option<i64>> = documents.iter().map(|d| d.file_size).collect();
        let file_extensions: Vec<Option<String>> =
            documents.iter().map(|d| d.file_extension.clone()).collect();
        let urls: Vec<Option<String>> = documents.iter().map(|d| d.url.clone()).collect();
        let metadata: Vec<serde_json::Value> =
            documents.iter().map(|d| d.metadata.clone()).collect();
        let permissions: Vec<serde_json::Value> =
            documents.iter().map(|d| d.permissions.clone()).collect();
        let created_ats: Vec<sqlx::types::time::OffsetDateTime> =
            documents.iter().map(|d| d.created_at).collect();
        let updated_ats: Vec<sqlx::types::time::OffsetDateTime> =
            documents.iter().map(|d| d.updated_at).collect();
        let last_indexed_ats: Vec<sqlx::types::time::OffsetDateTime> =
            documents.iter().map(|d| d.last_indexed_at).collect();

        let upserted_documents = sqlx::query_as::<_, Document>(
            r#"
            INSERT INTO documents (id, source_id, external_id, title, content_id, content_type, file_size, file_extension, url, metadata, permissions, created_at, updated_at, last_indexed_at, tsv_content)
            SELECT
                id, source_id, external_id, title, content_id, content_type, file_size, file_extension, url, metadata, permissions, created_at, updated_at, last_indexed_at,
                setweight(to_tsvector('english', title), 'A') || setweight(to_tsvector('english', content), 'B') as tsv_content
            FROM UNNEST(
                $1::text[], $2::text[], $3::text[], $4::text[], $5::text[], $6::text[],
                $7::bigint[], $8::text[], $9::text[], $10::jsonb[], $11::jsonb[],
                $12::timestamptz[], $13::timestamptz[], $14::timestamptz[], $15::text[]
            ) AS t(id, source_id, external_id, title, content_id, content_type, file_size, file_extension, url, metadata, permissions, created_at, updated_at, last_indexed_at, content)
            ON CONFLICT (source_id, external_id)
            DO UPDATE SET
                title = EXCLUDED.title,
                content_id = EXCLUDED.content_id,
                metadata = EXCLUDED.metadata,
                permissions = EXCLUDED.permissions,
                updated_at = EXCLUDED.updated_at,
                last_indexed_at = EXCLUDED.last_indexed_at,
                tsv_content = setweight(to_tsvector('english', EXCLUDED.title), 'A') || setweight(to_tsvector('english', (
                    SELECT content FROM UNNEST($15::text[]) WITH ORDINALITY AS c(content, ord)
                    WHERE ord = (SELECT ord FROM UNNEST($3::text[]) WITH ORDINALITY AS e(external_id, ord) WHERE e.external_id = EXCLUDED.external_id LIMIT 1)
                    LIMIT 1
                )), 'B')
            RETURNING id, source_id, external_id, title, content_id, content_type,
                      file_size, file_extension, url,
                      metadata, permissions, created_at, updated_at, last_indexed_at
            "#
        )
        .bind(&ids)
        .bind(&source_ids)
        .bind(&external_ids)
        .bind(&titles)
        .bind(&content_ids)
        .bind(&content_types)
        .bind(&file_sizes)
        .bind(&file_extensions)
        .bind(&urls)
        .bind(&metadata)
        .bind(&permissions)
        .bind(&created_ats)
        .bind(&updated_ats)
        .bind(&last_indexed_ats)
        .bind(&contents)
        .fetch_all(&self.pool)
        .await?;

        Ok(upserted_documents)
    }

    pub async fn batch_update_search_vectors(
        &self,
        document_ids: Vec<String>,
    ) -> Result<(), DatabaseError> {
        if document_ids.is_empty() {
            return Ok(());
        }

        // The tsv_content column is automatically generated, so this is now a no-op
        // We keep it for compatibility but it doesn't need to do anything
        Ok(())
    }

    pub async fn batch_mark_as_indexed(
        &self,
        document_ids: Vec<String>,
    ) -> Result<(), DatabaseError> {
        if document_ids.is_empty() {
            return Ok(());
        }

        sqlx::query(
            "UPDATE documents 
             SET last_indexed_at = CURRENT_TIMESTAMP 
             WHERE id = ANY($1)",
        )
        .bind(&document_ids)
        .execute(&self.pool)
        .await?;

        Ok(())
    }

    pub async fn batch_delete(&self, document_ids: Vec<String>) -> Result<i64, DatabaseError> {
        if document_ids.is_empty() {
            return Ok(0);
        }

        let result = sqlx::query("DELETE FROM documents WHERE id = ANY($1)")
            .bind(&document_ids)
            .execute(&self.pool)
            .await?;

        Ok(result.rows_affected() as i64)
    }
}

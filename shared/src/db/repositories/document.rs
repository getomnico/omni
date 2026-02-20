use crate::{
    db::error::DatabaseError,
    models::{AttributeFilter, Document, Facet, FacetValue},
    SourceType,
};
use serde_json::Value as JsonValue;
use sqlx::{FromRow, PgPool};
use std::collections::HashMap;
use tracing::debug;

#[derive(FromRow)]
pub struct SearchHit {
    #[sqlx(flatten)]
    pub document: Document,
    pub score: f32,
    #[sqlx(default)]
    pub content_snippets: Option<Vec<String>>,
}

#[derive(FromRow)]
pub struct TitleEntry {
    pub id: String,
    pub title: String,
    pub url: Option<String>,
    pub source_id: String,
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
                permissions @@@ 'public:true' OR
                permissions @@@ 'users:{}' OR
                permissions @@@ 'groups:{}'
            )"#,
            user_email, user_email
        )
    }

    pub async fn find_by_id(&self, id: &str) -> Result<Option<Document>, DatabaseError> {
        let document = sqlx::query_as::<_, Document>(
            r#"
            SELECT id, source_id, external_id, title, content_id, content_type,
                   file_size, file_extension, url,
                   metadata, permissions, attributes, created_at, updated_at, last_indexed_at
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
                   metadata, permissions, attributes, created_at, updated_at, last_indexed_at
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
                   metadata, permissions, attributes, created_at, updated_at, last_indexed_at
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

    pub async fn list_all_ids(&self) -> Result<Vec<String>, DatabaseError> {
        let rows = sqlx::query_scalar::<_, String>("SELECT id FROM documents")
            .fetch_all(&self.pool)
            .await?;
        Ok(rows)
    }

    pub async fn fetch_all_title_entries(&self) -> Result<Vec<TitleEntry>, DatabaseError> {
        let entries = sqlx::query_as::<_, TitleEntry>(
            r#"
            SELECT d.id, d.title, d.url, d.source_id
            FROM documents d
            JOIN sources s ON d.source_id = s.id
            WHERE NOT s.is_deleted
            "#,
        )
        .fetch_all(&self.pool)
        .await?;

        Ok(entries)
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

    pub async fn fetch_active_source_ids(
        &self,
        source_types: Option<&[SourceType]>,
    ) -> Result<Vec<String>, DatabaseError> {
        let source_ids: Vec<String> = if let Some(source_types) = source_types {
            sqlx::query_scalar(
                r#"SELECT id FROM sources WHERE source_type = ANY($1) AND NOT is_deleted"#,
            )
            .bind(source_types)
            .fetch_all(&self.pool)
            .await?
        } else {
            sqlx::query_scalar(r#"SELECT id FROM sources WHERE NOT is_deleted"#)
                .fetch_all(&self.pool)
                .await?
        };

        Ok(source_ids)
    }

    fn build_common_filters(
        &self,
        filters: &mut Vec<String>,
        param_idx: &mut usize,
        source_ids: &[String],
        content_types: Option<&[String]>,
        attribute_filters: Option<&HashMap<String, AttributeFilter>>,
        user_email: Option<&str>,
    ) {
        if !source_ids.is_empty() {
            filters.push(format!("source_id = ANY(${})", param_idx));
            *param_idx += 1;
        }

        let has_content_types = content_types.is_some_and(|ct| !ct.is_empty());
        if has_content_types {
            filters.push(format!("content_type = ANY(${})", param_idx));
            *param_idx += 1;
        }

        if let Some(attr_filters) = attribute_filters {
            for (key, filter) in attr_filters {
                match filter {
                    AttributeFilter::Exact(value) => {
                        let term_value = json_value_to_term_string(value);
                        filters.push(format!(
                            "attributes @@@ '{}:{}'",
                            key.replace('\'', "''"),
                            term_value.replace('\'', "''")
                        ));
                    }
                    AttributeFilter::AnyOf(values) => {
                        let conditions: Vec<String> = values
                            .iter()
                            .map(|v| {
                                let term_value = json_value_to_term_string(v);
                                format!(
                                    "attributes @@@ '{}:{}'",
                                    key.replace('\'', "''"),
                                    term_value.replace('\'', "''")
                                )
                            })
                            .collect();
                        if !conditions.is_empty() {
                            filters.push(format!("({})", conditions.join(" OR ")));
                        }
                    }
                    AttributeFilter::Range { gte, lte } => {
                        if let Some(gte_val) = gte {
                            let gte_str = json_value_to_term_string(gte_val);
                            filters.push(format!(
                                "attributes->>'{}' >= '{}'",
                                key.replace('\'', "''"),
                                gte_str.replace('\'', "''")
                            ));
                        }
                        if let Some(lte_val) = lte {
                            let lte_str = json_value_to_term_string(lte_val);
                            filters.push(format!(
                                "attributes->>'{}' <= '{}'",
                                key.replace('\'', "''"),
                                lte_str.replace('\'', "''")
                            ));
                        }
                    }
                }
            }
        }

        if let Some(email) = user_email {
            filters.push(self.generate_permission_filter(email));
        }
    }

    pub async fn search(
        &self,
        query: &str,
        source_ids: &[String],
        content_types: Option<&[String]>,
        attribute_filters: Option<&HashMap<String, AttributeFilter>>,
        limit: i64,
        offset: i64,
        user_email: Option<&str>,
        document_id: Option<&str>,
    ) -> Result<Vec<SearchHit>, DatabaseError> {
        if source_ids.is_empty() {
            return Ok(vec![]);
        }

        let mut filters = vec!["(title ||| $1 OR content ||| $2)".to_string()];
        let mut param_idx = 3;

        self.build_common_filters(
            &mut filters,
            &mut param_idx,
            source_ids,
            content_types,
            attribute_filters,
            user_email,
        );

        // Document ID will be set when running a search query within a single document.
        // Seems silly to use this function to search through the contents of a single doc, but
        // it's the easiest way right now to get the search results in the desired format.
        if document_id.is_some() {
            filters.push(format!("id = ${}", param_idx));
            param_idx += 1;
        }

        let where_clause = filters.join(" AND ");

        let full_query = format!(
            r#"
            SELECT id, source_id, external_id, title, content_id, content_type,
                   file_size, file_extension, url,
                   metadata, permissions, attributes, created_at, updated_at, last_indexed_at,
                   pdb.score(id) as score,
                   pdb.snippets(content, start_tag => '**', end_tag => '**', max_num_chars => 200, "limit" => 3, sort_by => 'score') as content_snippets
            FROM documents
            WHERE {}
            ORDER BY score DESC
            LIMIT ${} OFFSET ${}"#,
            where_clause,
            param_idx,
            param_idx + 1
        );
        debug!("Full search query: {}", full_query);

        let title_query = format!("{}::pdb.boost(2)", query);
        let mut query = sqlx::query_as::<_, SearchHit>(&full_query)
            .bind(title_query)
            .bind(query)
            .bind(source_ids);

        if let Some(ct) = content_types {
            if !ct.is_empty() {
                query = query.bind(ct);
            }
        }

        if let Some(doc_id) = document_id {
            query = query.bind(doc_id);
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
                   metadata, permissions, attributes, created_at, updated_at, last_indexed_at
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
                   metadata, permissions, attributes, created_at, updated_at, last_indexed_at
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
            INSERT INTO documents (id, source_id, external_id, title, content_id, content_type, metadata, permissions, attributes)
            VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9)
            RETURNING id, source_id, external_id, title, content_id, content_type,
                      file_size, file_extension, url,
                      metadata, permissions, attributes, created_at, updated_at, last_indexed_at
            "#
        )
        .bind(&document.id)
        .bind(&document.source_id)
        .bind(&document.external_id)
        .bind(&document.title)
        .bind(&document.content_id)
        .bind(&document.content_type)
        .bind(&document.metadata)
        .bind(&document.permissions)
        .bind(&document.attributes)
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
                attributes = $6,
                content = $7
            WHERE id = $1
            RETURNING id, source_id, external_id, title, content_id, content_type,
                      file_size, file_extension, url,
                      metadata, permissions, attributes, created_at, updated_at, last_indexed_at
            "#,
        )
        .bind(id)
        .bind(&document.title)
        .bind(&document.content_id)
        .bind(&document.metadata)
        .bind(&document.permissions)
        .bind(&document.attributes)
        .bind(content)
        .fetch_optional(&self.pool)
        .await?;

        Ok(updated_document)
    }

    /// Partial update using COALESCE — only overwrites fields that are Some
    pub async fn update_fields(
        &self,
        id: &str,
        title: Option<&str>,
        content_id: Option<&str>,
        metadata: Option<&JsonValue>,
        permissions: Option<&JsonValue>,
    ) -> Result<Option<Document>, DatabaseError> {
        let updated_document = sqlx::query_as::<_, Document>(
            r#"
            UPDATE documents
            SET title = COALESCE($2, title),
                content_id = COALESCE($3, content_id),
                metadata = COALESCE($4, metadata),
                permissions = COALESCE($5, permissions),
                updated_at = $6
            WHERE id = $1
            RETURNING id, source_id, external_id, title, content_id, content_type,
                      file_size, file_extension, url,
                      metadata, permissions, attributes, created_at, updated_at, last_indexed_at
            "#,
        )
        .bind(id)
        .bind(title)
        .bind(content_id)
        .bind(metadata)
        .bind(permissions)
        .bind(sqlx::types::time::OffsetDateTime::now_utc())
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

    /// Upserts a document with content for BM25 indexing
    pub async fn upsert(
        &self,
        document: Document,
        content: &str,
    ) -> Result<Document, DatabaseError> {
        let upserted_document = sqlx::query_as::<_, Document>(
            r#"
            INSERT INTO documents (id, source_id, external_id, title, content_id, content_type, file_size, file_extension, url, metadata, permissions, attributes, created_at, updated_at, last_indexed_at, content)
            VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13, $14, $15, $16)
            ON CONFLICT (source_id, external_id)
            DO UPDATE SET
                title = EXCLUDED.title,
                content_id = EXCLUDED.content_id,
                metadata = EXCLUDED.metadata,
                permissions = EXCLUDED.permissions,
                attributes = EXCLUDED.attributes,
                updated_at = EXCLUDED.updated_at,
                last_indexed_at = CURRENT_TIMESTAMP,
                content = EXCLUDED.content
            RETURNING id, source_id, external_id, title, content_id, content_type,
                      file_size, file_extension, url,
                      metadata, permissions, attributes, created_at, updated_at, last_indexed_at
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
        .bind(&document.attributes)
        .bind(&document.created_at)
        .bind(&document.updated_at)
        .bind(&document.last_indexed_at)
        .bind(content)
        .fetch_one(&self.pool)
        .await?;

        Ok(upserted_document)
    }

    pub async fn get_facet_counts(
        &self,
        query: &str,
        source_ids: &[String],
        content_types: Option<&[String]>,
        attribute_filters: Option<&HashMap<String, AttributeFilter>>,
        user_email: Option<&str>,
    ) -> Result<Vec<Facet>, DatabaseError> {
        if source_ids.is_empty() {
            return Ok(vec![]);
        }

        let mut filters = vec!["(title ||| $1 OR content ||| $2)".to_string()];
        let mut param_idx = 3;

        self.build_common_filters(
            &mut filters,
            &mut param_idx,
            source_ids,
            content_types,
            attribute_filters,
            user_email,
        );

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

        let title_query = format!("{}::pdb.boost(2)", query);
        let mut query = sqlx::query_as::<_, (String, String, i64)>(&query_str)
            .bind(title_query)
            .bind(query)
            .bind(source_ids);

        if let Some(ct) = content_types {
            if !ct.is_empty() {
                query = query.bind(ct);
            }
        }

        let facet_rows = query.fetch_all(&self.pool).await?;

        let mut facets_map: std::collections::HashMap<String, Vec<FacetValue>> =
            std::collections::HashMap::new();

        for (facet_name, value, count) in facet_rows {
            facets_map
                .entry(facet_name)
                .or_insert_with(Vec::new)
                .push(FacetValue { value, count });
        }

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
        let attributes: Vec<serde_json::Value> =
            documents.iter().map(|d| d.attributes.clone()).collect();
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
                attributes,
                created_at,
                updated_at,
                last_indexed_at,
                content
            )
            SELECT *
            FROM UNNEST(
                $1::text[], $2::text[], $3::text[], $4::text[], $5::text[], $6::text[],
                $7::bigint[], $8::text[], $9::text[], $10::jsonb[], $11::jsonb[], $12::jsonb[],
                $13::timestamptz[], $14::timestamptz[], $15::timestamptz[], $16::text[]
            ) AS t(id, source_id, external_id, title, content_id, content_type, file_size, file_extension, url, metadata, permissions, attributes, created_at, updated_at, last_indexed_at, content)
            ON CONFLICT (source_id, external_id)
            DO UPDATE SET
                title = EXCLUDED.title,
                content_id = EXCLUDED.content_id,
                metadata = EXCLUDED.metadata,
                permissions = EXCLUDED.permissions,
                attributes = EXCLUDED.attributes,
                updated_at = EXCLUDED.updated_at,
                last_indexed_at = CURRENT_TIMESTAMP,
                content = EXCLUDED.content
            RETURNING id, source_id, external_id, title, content_id, content_type,
                      file_size, file_extension, url,
                      metadata, permissions, attributes, created_at, updated_at, last_indexed_at
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
        .bind(&attributes)
        .bind(&created_ats)
        .bind(&updated_ats)
        .bind(&last_indexed_ats)
        .bind(&contents)
        .fetch_all(&self.pool)
        .await?;

        Ok(upserted_documents)
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

/// Convert a JSON value to a string suitable for ParadeDB term queries
fn json_value_to_term_string(value: &JsonValue) -> String {
    match value {
        JsonValue::String(s) => s.clone(),
        JsonValue::Number(n) => n.to_string(),
        JsonValue::Bool(b) => b.to_string(),
        JsonValue::Null => "null".to_string(),
        // For arrays and objects, serialize to JSON string
        _ => value.to_string(),
    }
}

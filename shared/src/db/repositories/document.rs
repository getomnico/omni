use crate::{
    db::error::DatabaseError,
    models::{AttributeFilter, DateFilter, Document},
    SourceType,
};
use serde_json::Value as JsonValue;
use sqlx::{FromRow, PgPool};
use std::collections::HashMap;
use time::{self, OffsetDateTime};

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

    /// Generate SQL condition to check if user has permission to access document.
    /// Checks: public access, direct user access, domain-wide access, and group membership.
    fn generate_permission_filter(&self, user_email: &str, user_groups: &[String]) -> String {
        generate_permission_filter(user_email, user_groups)
    }

    pub async fn find_by_id(&self, id: &str) -> Result<Option<Document>, DatabaseError> {
        let document = sqlx::query_as::<_, Document>(
            r#"
            SELECT id, source_id, external_id, title, content_id, content_type,
                   file_size, file_extension, url,
                   metadata, permissions, attributes, content_fingerprint,
                   created_at, updated_at, last_indexed_at
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
                   metadata, permissions, attributes, content_fingerprint,
                   created_at, updated_at, last_indexed_at
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
                   metadata, permissions, attributes, content_fingerprint,
                   created_at, updated_at, last_indexed_at
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
        user_groups: &[String],
        count: usize,
    ) -> Result<Vec<Document>, DatabaseError> {
        let permission_filter = &self.generate_permission_filter(user_email, user_groups);

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

    pub async fn fetch_active_sources(&self) -> Result<Vec<(String, SourceType)>, DatabaseError> {
        let rows: Vec<(String, SourceType)> =
            sqlx::query_as(r#"SELECT id, source_type FROM sources WHERE NOT is_deleted"#)
                .fetch_all(&self.pool)
                .await?;

        Ok(rows)
    }

    pub async fn fetch_all_permission_users(&self) -> Result<Vec<String>, DatabaseError> {
        let users: Vec<String> = sqlx::query_scalar(
            r#"SELECT DISTINCT lower(elem)
               FROM documents, jsonb_array_elements_text(permissions->'users') AS elem
               WHERE permissions->'users' IS NOT NULL"#,
        )
        .fetch_all(&self.pool)
        .await?;

        Ok(users)
    }

    pub async fn fetch_max_last_indexed_at(&self) -> Result<Option<OffsetDateTime>, DatabaseError> {
        let max_ts: Option<OffsetDateTime> =
            sqlx::query_scalar(r#"SELECT MAX(last_indexed_at) FROM documents"#)
                .fetch_one(&self.pool)
                .await?;

        Ok(max_ts)
    }

    fn build_common_filters(
        &self,
        filters: &mut Vec<String>,
        param_idx: &mut usize,
        source_ids: &[String],
        content_types: Option<&[String]>,
        attribute_filters: Option<&HashMap<String, AttributeFilter>>,
        user_email: Option<&str>,
        user_groups: &[String],
        date_filter: Option<&DateFilter>,
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

        if let Some(df) = date_filter {
            if let Some(after) = &df.after {
                let iso = after
                    .format(&time::format_description::well_known::Rfc3339)
                    .unwrap_or_default();
                filters.push(format!(
                    "metadata->>'updated_at' >= '{}'",
                    iso.replace('\'', "''")
                ));
            }
            if let Some(before) = &df.before {
                let iso = before
                    .format(&time::format_description::well_known::Rfc3339)
                    .unwrap_or_default();
                filters.push(format!(
                    "metadata->>'updated_at' <= '{}'",
                    iso.replace('\'', "''")
                ));
            }
        }

        if let Some(email) = user_email {
            filters.push(self.generate_permission_filter(email, user_groups));
        }
    }

    pub async fn find_by_source(&self, source_id: &str) -> Result<Vec<Document>, DatabaseError> {
        let documents = sqlx::query_as::<_, Document>(
            r#"
            SELECT id, source_id, external_id, title, content_id, content_type,
                   file_size, file_extension, url,
                   metadata, permissions, attributes, content_fingerprint,
                   created_at, updated_at, last_indexed_at
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
                   metadata, permissions, attributes, content_fingerprint,
                   created_at, updated_at, last_indexed_at
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

    pub async fn find_by_external_ids(
        &self,
        pairs: &[(String, String)], // Vec of (source_id, external_id)
    ) -> Result<Vec<Document>, DatabaseError> {
        if pairs.is_empty() {
            return Ok(vec![]);
        }

        let source_ids: Vec<&str> = pairs.iter().map(|(s, _)| s.as_str()).collect();
        let external_ids: Vec<&str> = pairs.iter().map(|(_, e)| e.as_str()).collect();

        let documents = sqlx::query_as::<_, Document>(
            r#"
            SELECT id, source_id, external_id, title, content_id, content_type,
                   file_size, file_extension, url,
                   metadata, permissions, attributes, content_fingerprint,
                   created_at, updated_at, last_indexed_at
            FROM documents
            WHERE (source_id, external_id) IN (
                SELECT * FROM UNNEST($1::text[], $2::text[])
            )
            "#,
        )
        .bind(&source_ids)
        .bind(&external_ids)
        .fetch_all(&self.pool)
        .await?;

        Ok(documents)
    }

    /// Find another document with the same content_fingerprint that has completed embedding.
    /// Used for embedding cloning: instead of regenerating embeddings for a duplicate,
    /// we clone from an already-embedded document with the same fingerprint.
    pub async fn find_embedded_duplicate(
        &self,
        content_fingerprint: &str,
        exclude_document_id: &str,
    ) -> Result<Option<String>, DatabaseError> {
        let doc_id: Option<String> = sqlx::query_scalar(
            r#"
            SELECT id FROM documents
            WHERE content_fingerprint = $1
              AND id != $2
              AND embedding_status = 'completed'
            LIMIT 1
            "#,
        )
        .bind(content_fingerprint)
        .bind(exclude_document_id)
        .fetch_optional(&self.pool)
        .await?;

        Ok(doc_id)
    }

    pub async fn create(&self, document: Document) -> Result<Document, DatabaseError> {
        let created_document = sqlx::query_as::<_, Document>(
            r#"
            INSERT INTO documents (id, source_id, external_id, title, content_id, content_type, metadata, permissions, attributes, content_fingerprint)
            VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10)
            RETURNING id, source_id, external_id, title, content_id, content_type,
                      file_size, file_extension, url,
                      metadata, permissions, attributes, content_fingerprint,
                      created_at, updated_at, last_indexed_at
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
        .bind(&document.content_fingerprint)
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
                      metadata, permissions, attributes, content_fingerprint,
                      created_at, updated_at, last_indexed_at
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
                      metadata, permissions, attributes, content_fingerprint,
                      created_at, updated_at, last_indexed_at
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
            INSERT INTO documents (id, source_id, external_id, title, content_id, content_type, file_size, file_extension, url, metadata, permissions, attributes, content_fingerprint, created_at, updated_at, last_indexed_at, content)
            VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13, $14, $15, $16, $17)
            ON CONFLICT (source_id, external_id)
            DO UPDATE SET
                title = COALESCE(NULLIF(EXCLUDED.title, ''), documents.title),
                content_id = EXCLUDED.content_id,
                metadata = EXCLUDED.metadata,
                permissions = COALESCE(EXCLUDED.permissions, documents.permissions),
                attributes = COALESCE(EXCLUDED.attributes, documents.attributes),
                content_fingerprint = COALESCE(EXCLUDED.content_fingerprint, documents.content_fingerprint),
                updated_at = EXCLUDED.updated_at,
                last_indexed_at = CURRENT_TIMESTAMP,
                content = EXCLUDED.content
            RETURNING id, source_id, external_id, title, content_id, content_type,
                      file_size, file_extension, url,
                      metadata, permissions, attributes, content_fingerprint,
                      created_at, updated_at, last_indexed_at
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
        .bind(&document.content_fingerprint)
        .bind(&document.created_at)
        .bind(&document.updated_at)
        .bind(&document.last_indexed_at)
        .bind(content)
        .fetch_one(&self.pool)
        .await?;

        Ok(upserted_document)
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
        let content_fingerprints: Vec<Option<String>> = documents
            .iter()
            .map(|d| d.content_fingerprint.clone())
            .collect();
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
                content_fingerprint,
                created_at,
                updated_at,
                last_indexed_at,
                content
            )
            SELECT *
            FROM UNNEST(
                $1::text[], $2::text[], $3::text[], $4::text[], $5::text[], $6::text[],
                $7::bigint[], $8::text[], $9::text[], $10::jsonb[], $11::jsonb[], $12::jsonb[],
                $13::text[],
                $14::timestamptz[], $15::timestamptz[], $16::timestamptz[], $17::text[]
            ) AS t(id, source_id, external_id, title, content_id, content_type, file_size, file_extension, url, metadata, permissions, attributes, content_fingerprint, created_at, updated_at, last_indexed_at, content)
            ON CONFLICT (source_id, external_id)
            DO UPDATE SET
                title = COALESCE(NULLIF(EXCLUDED.title, ''), documents.title),
                content_id = EXCLUDED.content_id,
                metadata = EXCLUDED.metadata,
                permissions = COALESCE(EXCLUDED.permissions, documents.permissions),
                attributes = COALESCE(EXCLUDED.attributes, documents.attributes),
                content_fingerprint = COALESCE(EXCLUDED.content_fingerprint, documents.content_fingerprint),
                updated_at = EXCLUDED.updated_at,
                last_indexed_at = CURRENT_TIMESTAMP,
                content = EXCLUDED.content
            RETURNING id, source_id, external_id, title, content_id, content_type,
                      file_size, file_extension, url,
                      metadata, permissions, attributes, content_fingerprint,
                      created_at, updated_at, last_indexed_at
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
        .bind(&content_fingerprints)
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

/// Generate SQL condition to check if user has permission to access document.
/// Checks: public access, direct user access, domain-wide access, and group membership.
///
/// Uses JSONB operators (`@>`, `?`) for exact matching instead of BM25 `@@@`
/// because BM25 tokenizes emails (e.g. "alice@example.com" → ["alice", "example", "com"]),
/// causing false positives when different addresses share tokens.
pub fn generate_permission_filter(user_email: &str, user_groups: &[String]) -> String {
    let escaped_email = user_email.replace('\'', "''");

    let mut conditions = vec![
        "permissions @> '{\"public\": true}'::jsonb".to_string(),
        format!("permissions->'users' ? '{}'", escaped_email),
    ];

    // Domain-wide access: exact JSON containment check to avoid BM25 tokenizer
    // splitting email addresses (e.g. "engineering@example.com" matching "example.com")
    if let Some(domain) = user_email.split('@').nth(1) {
        if !domain.is_empty() {
            let escaped_domain = domain.replace('\'', "''");
            conditions.push(format!("permissions->'groups' ? '{}'", escaped_domain));
        }
    }

    // Group membership: match each group the user belongs to
    for group_email in user_groups {
        let escaped_group = group_email.replace('\'', "''");
        conditions.push(format!("permissions->'groups' ? '{}'", escaped_group));
    }

    format!("({})", conditions.join(" OR "))
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

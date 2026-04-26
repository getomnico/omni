use crate::{db::error::DatabaseError, models::Document, DatabasePool, SourceType};
use serde_json::Value as JsonValue;
use sqlx::{FromRow, PgPool};
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
        db_pool: &DatabasePool,
        user_id: &str,
        count: usize,
    ) -> Result<Vec<Document>, DatabaseError> {
        let mut conn = db_pool.acquire_user(user_id).await?;

        let documents = sqlx::query_as::<_, Document>(
            r#"
            SELECT *
            FROM documents d
            WHERE d.content_id IS NOT NULL
            ORDER BY RANDOM()
            LIMIT $1
        "#,
        )
        .bind(count as i32)
        .fetch_all(&mut *conn)
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
                   metadata, permissions, attributes, created_at, updated_at, last_indexed_at
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
                title = COALESCE(NULLIF(EXCLUDED.title, ''), documents.title),
                content_id = EXCLUDED.content_id,
                metadata = EXCLUDED.metadata,
                permissions = COALESCE(EXCLUDED.permissions, documents.permissions),
                attributes = COALESCE(EXCLUDED.attributes, documents.attributes),
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
                title = COALESCE(NULLIF(EXCLUDED.title, ''), documents.title),
                content_id = EXCLUDED.content_id,
                metadata = EXCLUDED.metadata,
                permissions = COALESCE(EXCLUDED.permissions, documents.permissions),
                attributes = COALESCE(EXCLUDED.attributes, documents.attributes),
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

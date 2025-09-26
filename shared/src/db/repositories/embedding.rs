use crate::{
    db::error::DatabaseError,
    models::{ChunkResult, Document, Embedding},
    SourceType,
};
use pgvector::Vector;
use sqlx::{PgPool, Row};
use std::collections::HashSet;

pub struct EmbeddingRepository {
    pool: PgPool,
}

impl EmbeddingRepository {
    pub fn new(pool: &PgPool) -> Self {
        Self { pool: pool.clone() }
    }

    /// Generate SQL condition to check if user has permission to access document
    fn generate_permission_filter(&self, user_email: &str) -> String {
        format!(
            r#"(
                (d.permissions->>'public')::boolean = true OR
                d.permissions->'users' ? '{}' OR
                d.permissions->'groups' ? ANY(
                    -- TODO: Add group membership lookup here
                    ARRAY['{}']::text[]
                )
            )"#,
            user_email, user_email
        )
    }

    pub async fn find_by_document_id(
        &self,
        document_id: &str,
    ) -> Result<Vec<Embedding>, DatabaseError> {
        let embeddings = sqlx::query_as::<_, Embedding>(
            r#"
            SELECT id, document_id, chunk_index, chunk_start_offset, chunk_end_offset, embedding, model_name, created_at
            FROM embeddings
            WHERE document_id = $1
            ORDER BY chunk_index
            "#,
        )
        .bind(document_id)
        .fetch_all(&self.pool)
        .await?;

        Ok(embeddings)
    }

    pub async fn create(&self, embedding: Embedding) -> Result<Embedding, DatabaseError> {
        let created_embedding = sqlx::query_as::<_, Embedding>(
            r#"
            INSERT INTO embeddings (id, document_id, chunk_index, chunk_start_offset, chunk_end_offset, embedding, model_name)
            VALUES ($1, $2, $3, $4, $5, $6, $7)
            RETURNING id, document_id, chunk_index, chunk_start_offset, chunk_end_offset, embedding, model_name, created_at
            "#,
        )
        .bind(&embedding.id)
        .bind(&embedding.document_id)
        .bind(&embedding.chunk_index)
        .bind(&embedding.chunk_start_offset)
        .bind(&embedding.chunk_end_offset)
        .bind(&embedding.embedding)
        .bind(&embedding.model_name)
        .fetch_one(&self.pool)
        .await
        .map_err(|e| match e {
            sqlx::Error::Database(db_err) if db_err.is_unique_violation() => {
                DatabaseError::ConstraintViolation(
                    "Embedding already exists for this document and chunk".to_string(),
                )
            }
            _ => DatabaseError::from(e),
        })?;

        Ok(created_embedding)
    }

    pub async fn bulk_create(&self, embeddings: Vec<Embedding>) -> Result<(), DatabaseError> {
        if embeddings.is_empty() {
            return Ok(());
        }

        // Extract vectors for bulk insert using UNNEST
        let ids: Vec<String> = embeddings.iter().map(|e| e.id.clone()).collect();
        let document_ids: Vec<String> = embeddings.iter().map(|e| e.document_id.clone()).collect();
        let chunk_indices: Vec<i32> = embeddings.iter().map(|e| e.chunk_index).collect();
        let chunk_start_offsets: Vec<i32> =
            embeddings.iter().map(|e| e.chunk_start_offset).collect();
        let chunk_end_offsets: Vec<i32> = embeddings.iter().map(|e| e.chunk_end_offset).collect();
        let embedding_vectors: Vec<Vector> =
            embeddings.iter().map(|e| e.embedding.clone()).collect();
        let model_names: Vec<String> = embeddings.iter().map(|e| e.model_name.clone()).collect();

        let mut tx = self.pool.begin().await?;

        sqlx::query(
            r#"
            INSERT INTO embeddings (id, document_id, chunk_index, chunk_start_offset, chunk_end_offset, embedding, model_name)
            SELECT * FROM UNNEST($1::text[], $2::text[], $3::int4[], $4::int4[], $5::int4[], $6::vector[], $7::text[])
            ON CONFLICT (document_id, chunk_index, model_name) DO UPDATE
            SET chunk_start_offset = EXCLUDED.chunk_start_offset,
                chunk_end_offset = EXCLUDED.chunk_end_offset,
                embedding = EXCLUDED.embedding
            "#,
        )
        .bind(&ids)
        .bind(&document_ids)
        .bind(&chunk_indices)
        .bind(&chunk_start_offsets)
        .bind(&chunk_end_offsets)
        .bind(&embedding_vectors)
        .bind(&model_names)
        .execute(&mut *tx)
        .await?;

        tx.commit().await?;
        Ok(())
    }

    pub async fn find_similar_with_filters(
        &self,
        embedding: Vec<f32>,
        source_types: Option<&[SourceType]>,
        content_types: Option<&[String]>,
        limit: i64,
        offset: i64,
        user_email: Option<&str>,
    ) -> Result<Vec<ChunkResult>, DatabaseError> {
        let vector = Vector::from(embedding);

        let mut where_conditions = Vec::new();

        let mut bind_index = 4; // Starting after $1 (vector) and $2 (limit) and $3 (offset)

        if let Some(src) = source_types {
            if !src.is_empty() {
                where_conditions.push(format!(
                    "d.source_id IN (SELECT id FROM sources WHERE source_type = ANY(${}))",
                    bind_index
                ));
                bind_index += 1;
            }
        }

        if let Some(ct) = content_types {
            if !ct.is_empty() {
                where_conditions.push(format!("d.content_type = ANY(${})", bind_index));
                // bind_index would be incremented here for additional filters
            }
        }

        // Add permission filtering if user email is provided
        if let Some(email) = user_email {
            where_conditions.push(self.generate_permission_filter(email));
        }

        let where_clause = if where_conditions.len() > 0 {
            format!("WHERE {}", where_conditions.join(" AND "))
        } else {
            "".to_string()
        };

        let query_str = format!(
            r#"
            SELECT
                e.document_id,
                e.embedding <=> $1 as distance,
                e.chunk_start_offset,
                e.chunk_end_offset,
                e.chunk_index
            FROM embeddings e
            JOIN documents d ON e.document_id = d.id
            {}
            ORDER BY e.embedding <=> $1
            LIMIT $2 OFFSET $3
            "#,
            where_clause
        );

        let mut query = sqlx::query(&query_str)
            .bind(&vector)
            .bind(limit)
            .bind(offset);

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

        let results = query.fetch_all(&self.pool).await?;
        let chunk_results: Vec<ChunkResult> = results
            .into_iter()
            .map(|row| {
                let distance: Option<f64> = row.get("distance");
                let similarity = (1.0 - distance.unwrap_or(1.0)) as f32;
                ChunkResult {
                    document_id: row.get("document_id"),
                    similarity_score: similarity,
                    chunk_start_offset: row.get("chunk_start_offset"),
                    chunk_end_offset: row.get("chunk_end_offset"),
                    chunk_index: row.get("chunk_index"),
                }
            })
            .collect();

        Ok(chunk_results)
    }

    /// Find surrounding chunks for multiple center chunks from the same document with context window
    pub async fn find_surrounding_chunks_for_document(
        &self,
        document_id: &str,
        center_chunk_indices: &[i32],
        context_window: i32,
    ) -> Result<Vec<Embedding>, DatabaseError> {
        if center_chunk_indices.is_empty() {
            return Ok(vec![]);
        }

        // Build set of all required chunk indices (center chunks + their surrounding context)
        let mut all_indices = HashSet::new();
        for &center_index in center_chunk_indices {
            for offset in -context_window..=context_window {
                let chunk_index = center_index + offset;
                if chunk_index >= 0 {
                    all_indices.insert(chunk_index);
                }
            }
        }

        let indices: Vec<i32> = all_indices.into_iter().collect();

        let embeddings = sqlx::query_as::<_, Embedding>(
            r#"
            SELECT id, document_id, chunk_index, chunk_start_offset, chunk_end_offset, embedding, model_name, created_at
            FROM embeddings
            WHERE document_id = $1 AND chunk_index = ANY($2)
            ORDER BY chunk_index
            "#,
        )
        .bind(document_id)
        .bind(&indices)
        .fetch_all(&self.pool)
        .await?;

        Ok(embeddings)
    }

    pub async fn find_similar_chunks(
        &self,
        embedding: Vec<f32>,
        limit: i64,
    ) -> Result<Vec<(Document, f32, String)>, DatabaseError> {
        let vector = Vector::from(embedding);

        let results = sqlx::query(
            r#"
            SELECT 
                d.id, d.source_id, d.external_id, d.title, d.content_id,
                d.content_type, d.file_size, d.file_extension, d.url,
                d.metadata, d.permissions, d.created_at, d.updated_at, d.last_indexed_at,
                e.embedding <=> $1 as distance,
                e.chunk_start_offset,
                e.chunk_end_offset
            FROM embeddings e
            JOIN documents d ON e.document_id = d.id
            ORDER BY e.embedding <=> $1
            LIMIT $2
            "#,
        )
        .bind(&vector)
        .bind(limit)
        .fetch_all(&self.pool)
        .await?;

        let chunks_with_scores = results
            .into_iter()
            .map(|row| {
                let doc = Document {
                    id: row.get("id"),
                    source_id: row.get("source_id"),
                    external_id: row.get("external_id"),
                    title: row.get("title"),
                    content_id: row.get("content_id"),
                    content_type: row.get("content_type"),
                    file_size: row.get("file_size"),
                    file_extension: row.get("file_extension"),
                    url: row.get("url"),
                    metadata: row.get("metadata"),
                    permissions: row.get("permissions"),
                    created_at: row.get("created_at"),
                    updated_at: row.get("updated_at"),
                    last_indexed_at: row.get("last_indexed_at"),
                };
                let distance: Option<f64> = row.get("distance");
                let similarity = (1.0 - distance.unwrap_or(1.0)) as f32;
                // Extract chunk text from document content using offsets
                let _chunk_start_offset: i32 = row.get("chunk_start_offset");
                let _chunk_end_offset: i32 = row.get("chunk_end_offset");
                // TODO: Extract chunk text from LOB storage if needed for debugging
                let chunk_text = String::new();
                (doc, similarity, chunk_text)
            })
            .collect();

        Ok(chunks_with_scores)
    }

    pub async fn delete_by_document_id(&self, document_id: &str) -> Result<bool, DatabaseError> {
        let result = sqlx::query("DELETE FROM embeddings WHERE document_id = $1")
            .bind(document_id)
            .execute(&self.pool)
            .await?;

        Ok(result.rows_affected() > 0)
    }

    /// Bulk delete embeddings for multiple documents in a single query
    pub async fn bulk_delete_by_document_ids(
        &self,
        document_ids: &[String],
    ) -> Result<u64, DatabaseError> {
        if document_ids.is_empty() {
            return Ok(0);
        }

        let result = sqlx::query("DELETE FROM embeddings WHERE document_id = ANY($1)")
            .bind(document_ids)
            .execute(&self.pool)
            .await?;

        Ok(result.rows_affected())
    }
}

use crate::{
    db::error::DatabaseError,
    models::{Document, Embedding},
};
use pgvector::Vector;
use sqlx::{PgPool, Row};

pub struct EmbeddingRepository {
    pool: PgPool,
}

impl EmbeddingRepository {
    pub fn new(pool: &PgPool) -> Self {
        Self { pool: pool.clone() }
    }

    /// Extract chunk text from document content using character offsets
    pub fn extract_chunk_text(
        document_content: &str,
        start_offset: i32,
        end_offset: i32,
    ) -> String {
        let start = start_offset as usize;
        let end = end_offset as usize;

        if start >= document_content.len() || end > document_content.len() || start >= end {
            return String::new();
        }

        document_content[start..end].to_string()
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

        let mut tx = self.pool.begin().await?;

        for embedding in embeddings {
            sqlx::query(
                r#"
                INSERT INTO embeddings (id, document_id, chunk_index, chunk_start_offset, chunk_end_offset, embedding, model_name)
                VALUES ($1, $2, $3, $4, $5, $6, $7)
                ON CONFLICT (document_id, chunk_index, model_name) DO UPDATE
                SET chunk_start_offset = EXCLUDED.chunk_start_offset,
                    chunk_end_offset = EXCLUDED.chunk_end_offset,
                    embedding = EXCLUDED.embedding
                "#,
            )
            .bind(&embedding.id)
            .bind(&embedding.document_id)
            .bind(&embedding.chunk_index)
            .bind(&embedding.chunk_start_offset)
            .bind(&embedding.chunk_end_offset)
            .bind(&embedding.embedding)
            .bind(&embedding.model_name)
            .execute(&mut *tx)
            .await?;
        }

        tx.commit().await?;
        Ok(())
    }

    pub async fn find_similar(
        &self,
        embedding: Vec<f32>,
        limit: i64,
    ) -> Result<Vec<(Document, f32)>, DatabaseError> {
        let vector = Vector::from(embedding);

        // Find the best matching chunk for each document to avoid duplicates
        let results = sqlx::query(
            r#"
            WITH ranked_embeddings AS (
                SELECT 
                    e.document_id,
                    e.embedding <=> $1 as distance,
                    e.chunk_start_offset,
                    e.chunk_end_offset,
                    ROW_NUMBER() OVER (PARTITION BY e.document_id ORDER BY e.embedding <=> $1) as rn
                FROM embeddings e
            )
            SELECT 
                d.id, d.source_id, d.external_id, d.title, d.content,
                d.content_type, d.file_size, d.file_extension, d.url, d.parent_id,
                d.metadata, d.permissions, d.created_at, d.updated_at, d.last_indexed_at,
                re.distance,
                re.chunk_start_offset,
                re.chunk_end_offset
            FROM ranked_embeddings re
            JOIN documents d ON re.document_id = d.id
            WHERE re.rn = 1
            ORDER BY re.distance
            LIMIT $2
            "#,
        )
        .bind(&vector)
        .bind(limit)
        .fetch_all(&self.pool)
        .await?;

        let documents_with_scores = results
            .into_iter()
            .map(|row| {
                let doc = Document {
                    id: row.get("id"),
                    source_id: row.get("source_id"),
                    external_id: row.get("external_id"),
                    title: row.get("title"),
                    content: row.get("content"),
                    content_type: row.get("content_type"),
                    file_size: row.get("file_size"),
                    file_extension: row.get("file_extension"),
                    url: row.get("url"),
                    parent_id: row.get("parent_id"),
                    metadata: row.get("metadata"),
                    permissions: row.get("permissions"),
                    created_at: row.get("created_at"),
                    updated_at: row.get("updated_at"),
                    last_indexed_at: row.get("last_indexed_at"),
                };
                let distance: Option<f32> = row.get("distance");
                let similarity = 1.0 - distance.unwrap_or(1.0);
                (doc, similarity)
            })
            .collect();

        Ok(documents_with_scores)
    }

    pub async fn find_similar_with_filters(
        &self,
        embedding: Vec<f32>,
        sources: Option<&[String]>,
        content_types: Option<&[String]>,
        limit: i64,
        offset: i64,
    ) -> Result<Vec<(Document, f32)>, DatabaseError> {
        let vector = Vector::from(embedding);

        let mut where_conditions = Vec::new();
        where_conditions.push("re.rn = 1".to_string());

        let mut bind_index = 3; // Starting after $1 (vector) and $2 (limit)

        if let Some(src) = sources {
            if !src.is_empty() {
                where_conditions.push(format!("d.source_id = ANY(${})", bind_index));
                bind_index += 1;
            }
        }

        if let Some(ct) = content_types {
            if !ct.is_empty() {
                where_conditions.push(format!("d.content_type = ANY(${})", bind_index));
                bind_index += 1;
            }
        }

        let where_clause = where_conditions.join(" AND ");

        let query_str = format!(
            r#"
            WITH ranked_embeddings AS (
                SELECT 
                    e.document_id,
                    e.embedding <=> $1 as distance,
                    e.chunk_start_offset,
                    e.chunk_end_offset,
                    ROW_NUMBER() OVER (PARTITION BY e.document_id ORDER BY e.embedding <=> $1) as rn
                FROM embeddings e
            )
            SELECT 
                d.id, d.source_id, d.external_id, d.title, d.content,
                d.content_type, d.file_size, d.file_extension, d.url, d.parent_id,
                d.metadata, d.permissions, d.created_at, d.updated_at, d.last_indexed_at,
                re.distance,
                re.chunk_text
            FROM ranked_embeddings re
            JOIN documents d ON re.document_id = d.id
            WHERE {}
            ORDER BY re.distance
            LIMIT $2 OFFSET ${}
            "#,
            where_clause, bind_index
        );

        let mut query = sqlx::query(&query_str).bind(&vector).bind(limit);

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

        query = query.bind(offset);

        let results = query.fetch_all(&self.pool).await?;

        let documents_with_scores = results
            .into_iter()
            .map(|row| {
                let doc = Document {
                    id: row.get("id"),
                    source_id: row.get("source_id"),
                    external_id: row.get("external_id"),
                    title: row.get("title"),
                    content: row.get("content"),
                    content_type: row.get("content_type"),
                    file_size: row.get("file_size"),
                    file_extension: row.get("file_extension"),
                    url: row.get("url"),
                    parent_id: row.get("parent_id"),
                    metadata: row.get("metadata"),
                    permissions: row.get("permissions"),
                    created_at: row.get("created_at"),
                    updated_at: row.get("updated_at"),
                    last_indexed_at: row.get("last_indexed_at"),
                };
                let distance: Option<f32> = row.get("distance");
                let similarity = 1.0 - distance.unwrap_or(1.0);
                (doc, similarity)
            })
            .collect();

        Ok(documents_with_scores)
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
                d.id, d.source_id, d.external_id, d.title, d.content,
                d.content_type, d.file_size, d.file_extension, d.url, d.parent_id,
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
                    content: row.get("content"),
                    content_type: row.get("content_type"),
                    file_size: row.get("file_size"),
                    file_extension: row.get("file_extension"),
                    url: row.get("url"),
                    parent_id: row.get("parent_id"),
                    metadata: row.get("metadata"),
                    permissions: row.get("permissions"),
                    created_at: row.get("created_at"),
                    updated_at: row.get("updated_at"),
                    last_indexed_at: row.get("last_indexed_at"),
                };
                let distance: Option<f32> = row.get("distance");
                let similarity = 1.0 - distance.unwrap_or(1.0);
                // Extract chunk text from document content using offsets
                let chunk_start_offset: i32 = row.get("chunk_start_offset");
                let chunk_end_offset: i32 = row.get("chunk_end_offset");
                let chunk_text = if let Some(content) = &doc.content {
                    Self::extract_chunk_text(content, chunk_start_offset, chunk_end_offset)
                } else {
                    String::new()
                };
                (doc, similarity, chunk_text)
            })
            .collect();

        Ok(chunks_with_scores)
    }

    /// Find multiple relevant chunks per document for RAG context
    /// Returns up to max_chunks_per_doc chunks per document, with similarity threshold
    pub async fn find_rag_chunks(
        &self,
        embedding: Vec<f32>,
        max_chunks_per_doc: i32,
        similarity_threshold: f32,
        max_total_chunks: i64,
    ) -> Result<Vec<(Document, f32, String)>, DatabaseError> {
        let vector = Vector::from(embedding);
        let distance_threshold = 1.0 - similarity_threshold;

        let results = sqlx::query(
            r#"
            WITH ranked_chunks AS (
                SELECT 
                    e.document_id,
                    e.embedding <=> $1 as distance,
                    e.chunk_start_offset,
                    e.chunk_end_offset,
                    ROW_NUMBER() OVER (PARTITION BY e.document_id ORDER BY e.embedding <=> $1) as chunk_rank
                FROM embeddings e
                WHERE e.embedding <=> $1 <= $4
            )
            SELECT 
                d.id, d.source_id, d.external_id, d.title, d.content,
                d.content_type, d.file_size, d.file_extension, d.url, d.parent_id,
                d.metadata, d.permissions, d.created_at, d.updated_at, d.last_indexed_at,
                rc.distance,
                rc.chunk_start_offset,
                rc.chunk_end_offset
            FROM ranked_chunks rc
            JOIN documents d ON rc.document_id = d.id
            WHERE rc.chunk_rank <= $2
            ORDER BY rc.distance
            LIMIT $3
            "#,
        )
        .bind(&vector)
        .bind(max_chunks_per_doc)
        .bind(max_total_chunks)
        .bind(distance_threshold)
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
                    content: row.get("content"),
                    content_type: row.get("content_type"),
                    file_size: row.get("file_size"),
                    file_extension: row.get("file_extension"),
                    url: row.get("url"),
                    parent_id: row.get("parent_id"),
                    metadata: row.get("metadata"),
                    permissions: row.get("permissions"),
                    created_at: row.get("created_at"),
                    updated_at: row.get("updated_at"),
                    last_indexed_at: row.get("last_indexed_at"),
                };
                let distance: Option<f32> = row.get("distance");
                let similarity = 1.0 - distance.unwrap_or(1.0);
                // Extract chunk text from document content using offsets
                let chunk_start_offset: i32 = row.get("chunk_start_offset");
                let chunk_end_offset: i32 = row.get("chunk_end_offset");
                let chunk_text = if let Some(content) = &doc.content {
                    Self::extract_chunk_text(content, chunk_start_offset, chunk_end_offset)
                } else {
                    String::new()
                };
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
}

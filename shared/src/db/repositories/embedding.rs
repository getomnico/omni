use sqlx::{PgPool, Row};
use pgvector::Vector;
use crate::{
    db::error::DatabaseError,
    models::{Embedding, Document},
};

pub struct EmbeddingRepository {
    pool: PgPool,
}

impl EmbeddingRepository {
    pub fn new(pool: &PgPool) -> Self {
        Self {
            pool: pool.clone(),
        }
    }
    
    pub async fn find_by_document_id(&self, document_id: &str) -> Result<Option<Embedding>, DatabaseError> {
        let embedding = sqlx::query_as::<_, Embedding>(
            r#"
            SELECT id, document_id, embedding, model_name, created_at
            FROM embeddings
            WHERE document_id = $1
            "#
        )
        .bind(document_id)
        .fetch_optional(&self.pool)
        .await?;
        
        Ok(embedding)
    }
    
    pub async fn create(&self, embedding: Embedding) -> Result<Embedding, DatabaseError> {
        let created_embedding = sqlx::query_as::<_, Embedding>(
            r#"
            INSERT INTO embeddings (id, document_id, embedding, model_name)
            VALUES ($1, $2, $3, $4)
            RETURNING id, document_id, embedding, model_name, created_at
            "#
        )
        .bind(&embedding.id)
        .bind(&embedding.document_id)
        .bind(&embedding.embedding)
        .bind(&embedding.model_name)
        .fetch_one(&self.pool)
        .await
        .map_err(|e| match e {
            sqlx::Error::Database(db_err) if db_err.is_unique_violation() => {
                DatabaseError::ConstraintViolation("Embedding already exists for this document".to_string())
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
                INSERT INTO embeddings (id, document_id, embedding, model_name)
                VALUES ($1, $2, $3, $4)
                ON CONFLICT (document_id) DO UPDATE
                SET embedding = EXCLUDED.embedding,
                    model_name = EXCLUDED.model_name
                "#
            )
            .bind(&embedding.id)
            .bind(&embedding.document_id)
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
        limit: i64
    ) -> Result<Vec<(Document, f32)>, DatabaseError> {
        let vector = Vector::from(embedding);
        
        let results = sqlx::query(
            r#"
            SELECT 
                d.id, d.source_id, d.external_id, d.title, d.content,
                d.metadata, d.permissions,
                d.search_vector::text as search_vector,
                d.indexed_at, d.created_at, d.updated_at,
                e.embedding <=> $1 as distance
            FROM embeddings e
            JOIN documents d ON e.document_id = d.id
            ORDER BY e.embedding <=> $1
            LIMIT $2
            "#
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
                    metadata: row.get("metadata"),
                    permissions: row.get("permissions"),
                    search_vector: row.get("search_vector"),
                    indexed_at: row.get("indexed_at"),
                    created_at: row.get("created_at"),
                    updated_at: row.get("updated_at"),
                };
                let distance: Option<f32> = row.get("distance");
                let similarity = 1.0 - distance.unwrap_or(1.0);
                (doc, similarity)
            })
            .collect();
        
        Ok(documents_with_scores)
    }
    
    pub async fn delete_by_document_id(&self, document_id: &str) -> Result<bool, DatabaseError> {
        let result = sqlx::query("DELETE FROM embeddings WHERE document_id = $1")
            .bind(document_id)
            .execute(&self.pool)
            .await?;
        
        Ok(result.rows_affected() > 0)
    }
}
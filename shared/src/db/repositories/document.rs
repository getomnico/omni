use sqlx::PgPool;
use crate::{
    db::error::DatabaseError,
    models::Document,
};

pub struct DocumentRepository {
    pool: PgPool,
}

impl DocumentRepository {
    pub fn new(pool: &PgPool) -> Self {
        Self {
            pool: pool.clone(),
        }
    }
    
    pub async fn find_by_id(&self, id: &str) -> Result<Option<Document>, DatabaseError> {
        let document = sqlx::query_as::<_, Document>(
            r#"
            SELECT id, source_id, external_id, title, content, 
                   metadata, permissions,
                   search_vector::text as search_vector, indexed_at, created_at, updated_at
            FROM documents
            WHERE id = $1
            "#
        )
        .bind(id)
        .fetch_optional(&self.pool)
        .await?;
        
        Ok(document)
    }
    
    pub async fn find_all(&self, limit: i64, offset: i64) -> Result<Vec<Document>, DatabaseError> {
        let documents = sqlx::query_as::<_, Document>(
            r#"
            SELECT id, source_id, external_id, title, content, 
                   metadata, permissions,
                   search_vector::text as search_vector, indexed_at, created_at, updated_at
            FROM documents
            ORDER BY created_at DESC
            LIMIT $1 OFFSET $2
            "#
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
            SELECT id, source_id, external_id, title, content, 
                   metadata, permissions,
                   search_vector::text as search_vector, indexed_at, created_at, updated_at
            FROM documents
            WHERE search_vector @@ plainto_tsquery('english', $1)
            ORDER BY ts_rank(search_vector, plainto_tsquery('english', $1)) DESC
            LIMIT $2
            "#
        )
        .bind(query)
        .bind(limit)
        .fetch_all(&self.pool)
        .await?;
        
        Ok(documents)
    }
    
    pub async fn find_by_source(&self, source_id: &str) -> Result<Vec<Document>, DatabaseError> {
        let documents = sqlx::query_as::<_, Document>(
            r#"
            SELECT id, source_id, external_id, title, content, 
                   metadata, permissions,
                   search_vector::text as search_vector, indexed_at, created_at, updated_at
            FROM documents
            WHERE source_id = $1
            ORDER BY created_at DESC
            "#
        )
        .bind(source_id)
        .fetch_all(&self.pool)
        .await?;
        
        Ok(documents)
    }
    
    pub async fn create(&self, document: Document) -> Result<Document, DatabaseError> {
        let created_document = sqlx::query_as::<_, Document>(
            r#"
            INSERT INTO documents (id, source_id, external_id, title, content, metadata, permissions)
            VALUES ($1, $2, $3, $4, $5, $6, $7)
            RETURNING id, source_id, external_id, title, content, 
                      metadata, permissions,
                      search_vector::text as search_vector, indexed_at, created_at, updated_at
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
    
    pub async fn update(&self, id: &str, document: Document) -> Result<Option<Document>, DatabaseError> {
        let updated_document = sqlx::query_as::<_, Document>(
            r#"
            UPDATE documents
            SET title = $2, content = $3, metadata = $4, permissions = $5
            WHERE id = $1
            RETURNING id, source_id, external_id, title, content, 
                      metadata, permissions,
                      search_vector::text as search_vector, indexed_at, created_at, updated_at
            "#
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
    
    pub async fn update_search_vector(&self, id: &str) -> Result<(), DatabaseError> {
        sqlx::query(
            r#"
            UPDATE documents 
            SET search_vector = to_tsvector('english', coalesce(title, '') || ' ' || coalesce(content, ''))
            WHERE id = $1
            "#
        )
        .bind(id)
        .execute(&self.pool)
        .await?;
        
        Ok(())
    }
    
    pub async fn mark_as_indexed(&self, id: &str) -> Result<(), DatabaseError> {
        sqlx::query("UPDATE documents SET indexed_at = CURRENT_TIMESTAMP WHERE id = $1")
            .bind(id)
            .execute(&self.pool)
            .await?;
        
        Ok(())
    }
}
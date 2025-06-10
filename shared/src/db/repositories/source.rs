use async_trait::async_trait;
use sqlx::PgPool;
use crate::{
    db::error::DatabaseError,
    models::Source,
    traits::Repository,
};

pub struct SourceRepository {
    pool: PgPool,
}

impl SourceRepository {
    pub fn new(pool: &PgPool) -> Self {
        Self {
            pool: pool.clone(),
        }
    }
    
    pub async fn find_by_type(&self, source_type: &str) -> Result<Vec<Source>, DatabaseError> {
        let sources = sqlx::query_as::<_, Source>(
            r#"
            SELECT id, name, source_type, configuration, credentials, enabled, 
                   last_sync_at, created_at, updated_at
            FROM sources
            WHERE source_type = $1
            ORDER BY created_at DESC
            "#
        )
        .bind(source_type)
        .fetch_all(&self.pool)
        .await?;
        
        Ok(sources)
    }
    
    pub async fn find_active_sources(&self) -> Result<Vec<Source>, DatabaseError> {
        let sources = sqlx::query_as::<_, Source>(
            r#"
            SELECT id, name, source_type, configuration, credentials, enabled, 
                   last_sync_at, created_at, updated_at
            FROM sources
            WHERE enabled = true
            ORDER BY created_at DESC
            "#
        )
        .fetch_all(&self.pool)
        .await?;
        
        Ok(sources)
    }
    
    pub async fn update_last_sync(&self, id: &str) -> Result<(), DatabaseError> {
        sqlx::query("UPDATE sources SET last_sync_at = CURRENT_TIMESTAMP WHERE id = $1")
            .bind(id)
            .execute(&self.pool)
            .await?;
        
        Ok(())
    }
}

#[async_trait]
impl Repository<Source, String> for SourceRepository {
    async fn find_by_id(&self, id: String) -> Result<Option<Source>, DatabaseError> {
        let source = sqlx::query_as::<_, Source>(
            r#"
            SELECT id, name, source_type, configuration, credentials, enabled, 
                   last_sync_at, created_at, updated_at
            FROM sources
            WHERE id = $1
            "#
        )
        .bind(id)
        .fetch_optional(&self.pool)
        .await?;
        
        Ok(source)
    }
    
    async fn find_all(&self, limit: i64, offset: i64) -> Result<Vec<Source>, DatabaseError> {
        let sources = sqlx::query_as::<_, Source>(
            r#"
            SELECT id, name, source_type, configuration, credentials, enabled, 
                   last_sync_at, created_at, updated_at
            FROM sources
            ORDER BY created_at DESC
            LIMIT $1 OFFSET $2
            "#
        )
        .bind(limit)
        .bind(offset)
        .fetch_all(&self.pool)
        .await?;
        
        Ok(sources)
    }
    
    async fn create(&self, source: Source) -> Result<Source, DatabaseError> {
        let created_source = sqlx::query_as::<_, Source>(
            r#"
            INSERT INTO sources (id, name, source_type, configuration, credentials, enabled)
            VALUES ($1, $2, $3, $4, $5, $6)
            RETURNING id, name, source_type, configuration, credentials, enabled, 
                      last_sync_at, created_at, updated_at
            "#
        )
        .bind(&source.id)
        .bind(&source.name)
        .bind(&source.source_type)
        .bind(&source.configuration)
        .bind(&source.credentials)
        .bind(source.enabled)
        .fetch_one(&self.pool)
        .await
        .map_err(|e| match e {
            sqlx::Error::Database(db_err) if db_err.is_unique_violation() => {
                DatabaseError::ConstraintViolation("Source name already exists".to_string())
            }
            _ => DatabaseError::from(e),
        })?;
        
        Ok(created_source)
    }
    
    async fn update(&self, id: String, source: Source) -> Result<Option<Source>, DatabaseError> {
        let updated_source = sqlx::query_as::<_, Source>(
            r#"
            UPDATE sources
            SET name = $2, source_type = $3, configuration = $4, 
                credentials = $5, enabled = $6
            WHERE id = $1
            RETURNING id, name, source_type, configuration, credentials, enabled, 
                      last_sync_at, created_at, updated_at
            "#
        )
        .bind(&id)
        .bind(&source.name)
        .bind(&source.source_type)
        .bind(&source.configuration)
        .bind(&source.credentials)
        .bind(source.enabled)
        .fetch_optional(&self.pool)
        .await?;
        
        Ok(updated_source)
    }
    
    async fn delete(&self, id: String) -> Result<bool, DatabaseError> {
        let result = sqlx::query("DELETE FROM sources WHERE id = $1")
            .bind(&id)
            .execute(&self.pool)
            .await?;
        
        Ok(result.rows_affected() > 0)
    }
}
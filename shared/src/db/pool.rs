use sqlx::{postgres::PgPoolOptions, PgPool};
use std::time::Duration;
use crate::db::error::DatabaseError;

#[derive(Clone)]
pub struct DatabasePool {
    pool: PgPool,
}

impl DatabasePool {
    pub async fn new(database_url: &str) -> Result<Self, DatabaseError> {
        let pool = PgPoolOptions::new()
            .max_connections(10)
            .acquire_timeout(Duration::from_secs(3))
            .connect(database_url)
            .await?;
        
        Ok(Self { pool })
    }
    
    pub async fn new_with_options(
        database_url: &str,
        max_connections: u32,
        timeout_seconds: u64,
    ) -> Result<Self, DatabaseError> {
        let pool = PgPoolOptions::new()
            .max_connections(max_connections)
            .acquire_timeout(Duration::from_secs(timeout_seconds))
            .connect(database_url)
            .await?;
        
        Ok(Self { pool })
    }
    
    pub fn pool(&self) -> &PgPool {
        &self.pool
    }
    
    pub async fn close(&self) {
        self.pool.close().await;
    }
}
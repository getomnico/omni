use crate::db::error::DatabaseError;
use sqlx::PgPool;

#[derive(Clone)]
pub struct EmbeddingProviderRepository {
    pool: PgPool,
}

impl EmbeddingProviderRepository {
    pub fn new(pool: &PgPool) -> Self {
        Self { pool: pool.clone() }
    }

    pub async fn has_active_provider(&self) -> Result<bool, DatabaseError> {
        let row: (bool,) = sqlx::query_as(
            "SELECT EXISTS(SELECT 1 FROM embedding_providers WHERE is_current = TRUE AND is_deleted = FALSE)",
        )
        .fetch_one(&self.pool)
        .await?;

        Ok(row.0)
    }
}

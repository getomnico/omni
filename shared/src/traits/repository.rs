use crate::db::error::DatabaseError;
use async_trait::async_trait;

#[async_trait]
pub trait Repository<T, ID>
where
    T: Send + Sync,
    ID: Send + Sync,
{
    async fn find_by_id(&self, id: ID) -> Result<Option<T>, DatabaseError>;
    async fn find_all(&self, limit: i64, offset: i64) -> Result<Vec<T>, DatabaseError>;
    async fn create(&self, entity: T) -> Result<T, DatabaseError>;
    async fn update(&self, id: ID, entity: T) -> Result<Option<T>, DatabaseError>;
    async fn delete(&self, id: ID) -> Result<bool, DatabaseError>;
}

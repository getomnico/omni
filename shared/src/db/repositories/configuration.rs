use crate::db::error::DatabaseError;
use serde_json::Value as JsonValue;
use sqlx::{PgPool, Row};

pub struct ConfigurationRepository {
    pool: PgPool,
}

impl ConfigurationRepository {
    pub fn new(pool: &PgPool) -> Self {
        Self { pool: pool.clone() }
    }

    pub async fn find_user_configuration_rows(
        &self,
        user_id: &str,
    ) -> Result<Vec<(String, JsonValue)>, DatabaseError> {
        let rows = sqlx::query(
            "SELECT key, value FROM configuration WHERE scope = 'user' AND user_id = $1",
        )
        .bind(user_id)
        .fetch_all(&self.pool)
        .await?;

        rows.into_iter()
            .map(|row| {
                let key: String = row.try_get("key")?;
                let value: JsonValue = row.try_get("value")?;
                Ok((key, value))
            })
            .collect::<Result<Vec<_>, sqlx::Error>>()
            .map_err(DatabaseError::from)
    }
}

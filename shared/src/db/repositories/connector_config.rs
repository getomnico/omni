use anyhow::Result;
use sqlx::PgPool;
use tracing::debug;

use crate::models::ConnectorConfigRow;

pub struct ConnectorConfigRepository {
    pool: PgPool,
}

impl ConnectorConfigRepository {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }

    pub async fn get_by_provider(&self, provider: &str) -> Result<Option<ConnectorConfigRow>> {
        debug!("Getting connector config for provider: {}", provider);

        let config = sqlx::query_as::<_, ConnectorConfigRow>(
            "SELECT provider, config, updated_at, updated_by FROM connector_configs WHERE provider = $1",
        )
        .bind(provider)
        .fetch_optional(&self.pool)
        .await?;

        Ok(config)
    }

    pub async fn upsert(
        &self,
        provider: &str,
        config: serde_json::Value,
        updated_by: Option<&str>,
    ) -> Result<ConnectorConfigRow> {
        debug!("Upserting connector config for provider: {}", provider);

        let row = sqlx::query_as::<_, ConnectorConfigRow>(
            r#"
            INSERT INTO connector_configs (provider, config, updated_by, updated_at)
            VALUES ($1, $2, $3, NOW())
            ON CONFLICT (provider) DO UPDATE SET
                config = $2,
                updated_by = $3,
                updated_at = NOW()
            RETURNING provider, config, updated_at, updated_by
            "#,
        )
        .bind(provider)
        .bind(config)
        .bind(updated_by)
        .fetch_one(&self.pool)
        .await?;

        Ok(row)
    }
}

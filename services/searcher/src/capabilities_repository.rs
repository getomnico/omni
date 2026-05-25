use crate::models::{CapabilitySearchResult, CapabilityUpsert};
use shared::db::error::DatabaseError;
use sqlx::{FromRow, PgPool};

#[derive(FromRow)]
struct CapabilityHit {
    id: String,
    data: serde_json::Value,
    score: f32,
}

pub struct AgentCapabilitiesRepository {
    pool: PgPool,
}

impl AgentCapabilitiesRepository {
    pub fn new(pool: &PgPool) -> Self {
        Self { pool: pool.clone() }
    }

    pub async fn upsert_many(&self, items: &[CapabilityUpsert]) -> Result<(), DatabaseError> {
        if items.is_empty() {
            return Ok(());
        }

        let ids: Vec<&str> = items.iter().map(|i| i.id.as_str()).collect();
        let data: Vec<serde_json::Value> = items.iter().map(|i| i.data.clone()).collect();

        sqlx::query(
            r#"
            INSERT INTO agent_capabilities (id, data, created_at, updated_at)
            SELECT u.id, u.data, NOW(), NOW()
            FROM UNNEST($1::varchar[], $2::jsonb[]) AS u(id, data)
            ON CONFLICT (id) DO UPDATE SET
                data = EXCLUDED.data,
                updated_at = NOW()
            "#,
        )
        .bind(ids)
        .bind(data)
        .execute(&self.pool)
        .await?;

        Ok(())
    }

    pub async fn search(
        &self,
        capability_type: &str,
        query: &str,
        limit: i64,
        allowed_ids: Option<&[String]>,
        allowed_source_ids: Option<&[String]>,
    ) -> Result<Vec<CapabilitySearchResult>, DatabaseError> {
        if query.trim().is_empty() {
            return Ok(vec![]);
        }

        let limit = limit.clamp(1, 50);
        let allowed_ids_empty = allowed_ids.map(|ids| ids.is_empty()).unwrap_or(false);
        let allowed_sources_empty = allowed_source_ids
            .map(|ids| ids.is_empty())
            .unwrap_or(false);
        if allowed_ids_empty || allowed_sources_empty {
            return Ok(vec![]);
        }

        let allowed_ids = allowed_ids.map(|v| v.to_vec());
        let allowed_source_ids = allowed_source_ids.map(|v| v.to_vec());

        let rows = sqlx::query_as::<_, CapabilityHit>(
            r#"
            SELECT id, data, pdb.score(id) as score
            FROM agent_capabilities
            WHERE id @@@ pdb.parse($1, lenient => true)
              AND data->>'capability_type' = $2
              AND ($3::text[] IS NULL OR id = ANY($3))
              AND ($4::text[] IS NULL OR data->>'source_id' = ANY($4))
            ORDER BY score DESC
            LIMIT $5
            "#,
        )
        .bind(query)
        .bind(capability_type)
        .bind(allowed_ids)
        .bind(allowed_source_ids)
        .bind(limit)
        .fetch_all(&self.pool)
        .await?;

        Ok(rows
            .into_iter()
            .map(|row| CapabilitySearchResult {
                id: row.id,
                data: row.data,
                score: row.score,
            })
            .collect())
    }
}

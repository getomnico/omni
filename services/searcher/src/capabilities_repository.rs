use crate::models::{CapabilitySearchResult, CapabilityUpsert};
use shared::db::error::DatabaseError;
use sqlx::{FromRow, PgPool};

#[derive(FromRow)]
struct CapabilityHit {
    id: String,
    capability_type: String,
    name: String,
    description: String,
    user_id: Option<String>,
    source_id: Option<String>,
    source_type: Option<String>,
    search_text: String,
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
        let capability_types: Vec<&str> =
            items.iter().map(|i| i.capability_type.as_str()).collect();
        let names: Vec<&str> = items.iter().map(|i| i.name.as_str()).collect();
        let descriptions: Vec<&str> = items.iter().map(|i| i.description.as_str()).collect();
        let user_ids: Vec<Option<&str>> = items.iter().map(|i| i.user_id.as_deref()).collect();
        let source_ids: Vec<Option<&str>> = items.iter().map(|i| i.source_id.as_deref()).collect();
        let source_types: Vec<Option<&str>> =
            items.iter().map(|i| i.source_type.as_deref()).collect();
        let search_texts: Vec<&str> = items.iter().map(|i| i.search_text.as_str()).collect();
        let data: Vec<serde_json::Value> = items.iter().map(|i| i.data.clone()).collect();

        sqlx::query(
            r#"
            INSERT INTO agent_capabilities (
                id, capability_type, name, description, user_id, source_id, source_type,
                search_text, data, created_at, updated_at
            )
            SELECT u.id, u.capability_type, u.name, u.description, u.user_id,
                   u.source_id, u.source_type, u.search_text, u.data, NOW(), NOW()
            FROM UNNEST(
                $1::varchar[], $2::text[], $3::text[], $4::text[], $5::text[],
                $6::text[], $7::text[], $8::text[], $9::jsonb[]
            ) AS u(id, capability_type, name, description, user_id, source_id,
                   source_type, search_text, data)
            ON CONFLICT (id) DO UPDATE SET
                capability_type = EXCLUDED.capability_type,
                name = EXCLUDED.name,
                description = EXCLUDED.description,
                user_id = EXCLUDED.user_id,
                source_id = EXCLUDED.source_id,
                source_type = EXCLUDED.source_type,
                search_text = EXCLUDED.search_text,
                data = EXCLUDED.data,
                updated_at = NOW()
            "#,
        )
        .bind(ids)
        .bind(capability_types)
        .bind(names)
        .bind(descriptions)
        .bind(user_ids)
        .bind(source_ids)
        .bind(source_types)
        .bind(search_texts)
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
            SELECT id, capability_type, name, description, user_id, source_id,
                   source_type, search_text, data, pdb.score(id) as score
            FROM agent_capabilities
            WHERE search_text ||| $1
              AND capability_type = $2
              AND ($3::text[] IS NULL OR id = ANY($3))
              AND ($4::text[] IS NULL OR source_id = ANY($4))
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
                capability_type: row.capability_type,
                name: row.name,
                description: row.description,
                user_id: row.user_id,
                source_id: row.source_id,
                source_type: row.source_type,
                search_text: row.search_text,
                data: row.data,
                score: row.score,
            })
            .collect())
    }
}

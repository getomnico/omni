use crate::models::{CapabilitySearchResult, CapabilityUpsert};
use shared::db::error::DatabaseError;
use sqlx::{FromRow, PgPool};

#[derive(FromRow)]
struct CapabilityHit {
    capability_id: String,
    capability_type: String,
    item_id: String,
    title: String,
    description: String,
    body: String,
    source_id: Option<String>,
    source_type: Option<String>,
    metadata: serde_json::Value,
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

        let capability_ids: Vec<&str> = items.iter().map(|i| i.capability_id.as_str()).collect();
        let capability_types: Vec<&str> =
            items.iter().map(|i| i.capability_type.as_str()).collect();
        let item_ids: Vec<&str> = items.iter().map(|i| i.item_id.as_str()).collect();
        let titles: Vec<&str> = items.iter().map(|i| i.title.as_str()).collect();
        let descriptions: Vec<&str> = items.iter().map(|i| i.description.as_str()).collect();
        let bodies: Vec<&str> = items.iter().map(|i| i.body.as_str()).collect();
        let source_ids: Vec<Option<&str>> = items.iter().map(|i| i.source_id.as_deref()).collect();
        let source_types: Vec<Option<&str>> =
            items.iter().map(|i| i.source_type.as_deref()).collect();
        let visibilities: Vec<serde_json::Value> =
            items.iter().map(|i| i.visibility.clone()).collect();
        let metadatas: Vec<serde_json::Value> = items.iter().map(|i| i.metadata.clone()).collect();

        sqlx::query(
            r#"
            INSERT INTO agent_capabilities (
                capability_id, capability_type, item_id, title, description, body,
                source_id, source_type, visibility, metadata, updated_at
            )
            SELECT u.*, NOW()
            FROM UNNEST(
                $1::varchar[], $2::varchar[], $3::text[], $4::text[], $5::text[],
                $6::text[], $7::text[], $8::text[], $9::jsonb[], $10::jsonb[]
            ) AS u(
                capability_id, capability_type, item_id, title, description, body,
                source_id, source_type, visibility, metadata
            )
            ON CONFLICT (capability_id) DO UPDATE SET
                capability_type = EXCLUDED.capability_type,
                item_id = EXCLUDED.item_id,
                title = EXCLUDED.title,
                description = EXCLUDED.description,
                body = EXCLUDED.body,
                source_id = EXCLUDED.source_id,
                source_type = EXCLUDED.source_type,
                visibility = EXCLUDED.visibility,
                metadata = EXCLUDED.metadata,
                updated_at = NOW()
            "#,
        )
        .bind(capability_ids)
        .bind(capability_types)
        .bind(item_ids)
        .bind(titles)
        .bind(descriptions)
        .bind(bodies)
        .bind(source_ids)
        .bind(source_types)
        .bind(visibilities)
        .bind(metadatas)
        .execute(&self.pool)
        .await?;

        Ok(())
    }

    pub async fn search(
        &self,
        capability_type: &str,
        query: &str,
        limit: i64,
        allowed_item_ids: Option<&[String]>,
        allowed_source_ids: Option<&[String]>,
    ) -> Result<Vec<CapabilitySearchResult>, DatabaseError> {
        if query.trim().is_empty() {
            return Ok(vec![]);
        }

        let limit = limit.clamp(1, 50);
        let allowed_items_empty = allowed_item_ids.map(|ids| ids.is_empty()).unwrap_or(false);
        let allowed_sources_empty = allowed_source_ids
            .map(|ids| ids.is_empty())
            .unwrap_or(false);
        if allowed_items_empty || allowed_sources_empty {
            return Ok(vec![]);
        }

        let allowed_item_ids = allowed_item_ids.map(|v| v.to_vec());
        let allowed_source_ids = allowed_source_ids.map(|v| v.to_vec());

        let rows = sqlx::query_as::<_, CapabilityHit>(
            r#"
            SELECT capability_id, capability_type, item_id, title, description, LEFT(body, 500) AS body,
                   source_id, source_type, metadata, pdb.score(capability_id) as score
            FROM agent_capabilities
            WHERE capability_id @@@ pdb.parse($1, lenient => true)
              AND capability_type = $2
              AND ($3::text[] IS NULL OR item_id = ANY($3))
              AND ($4::text[] IS NULL OR source_id = ANY($4))
            ORDER BY score DESC
            LIMIT $5
            "#,
        )
        .bind(query)
        .bind(capability_type)
        .bind(allowed_item_ids)
        .bind(allowed_source_ids)
        .bind(limit)
        .fetch_all(&self.pool)
        .await?;

        Ok(rows
            .into_iter()
            .map(|row| CapabilitySearchResult {
                capability_id: row.capability_id,
                capability_type: row.capability_type,
                item_id: row.item_id,
                title: row.title,
                description: row.description,
                body: row.body,
                source_id: row.source_id,
                source_type: row.source_type,
                metadata: row.metadata,
                score: row.score,
            })
            .collect())
    }
}

use crate::{db::error::DatabaseError, models::Source, traits::Repository};
use async_trait::async_trait;
use sqlx::PgPool;
use std::collections::HashMap;
use time::OffsetDateTime;

#[derive(Clone)]
pub struct SourceRepository {
    pool: PgPool,
}

impl SourceRepository {
    pub fn new(pool: &PgPool) -> Self {
        Self { pool: pool.clone() }
    }

    pub async fn find_by_type(&self, source_type: &str) -> Result<Vec<Source>, DatabaseError> {
        let sources = sqlx::query_as::<_, Source>(
            r#"
            SELECT id, name, source_type, config, is_active, is_deleted,
                   user_filter_mode, user_whitelist, user_blacklist,
                   connector_state, sync_interval_seconds, created_at, updated_at, created_by
            FROM sources
            WHERE source_type = $1 AND is_deleted = false
            ORDER BY created_at DESC
            "#,
        )
        .bind(source_type)
        .fetch_all(&self.pool)
        .await?;

        Ok(sources)
    }

    pub async fn find_all_sources(&self) -> Result<Vec<Source>, DatabaseError> {
        let sources = sqlx::query_as::<_, Source>(
            r#"
            SELECT id, name, source_type, config, is_active, is_deleted,
                   user_filter_mode, user_whitelist, user_blacklist,
                   connector_state, sync_interval_seconds, created_at, updated_at, created_by
            FROM sources
            WHERE is_deleted = false
            ORDER BY created_at DESC
            "#,
        )
        .fetch_all(&self.pool)
        .await?;

        Ok(sources)
    }

    pub async fn find_active_sources(&self) -> Result<Vec<Source>, DatabaseError> {
        let sources = sqlx::query_as::<_, Source>(
            r#"
            SELECT id, name, source_type, config, is_active, is_deleted,
                   user_filter_mode, user_whitelist, user_blacklist,
                   connector_state, sync_interval_seconds, created_at, updated_at, created_by
            FROM sources
            WHERE is_active = true AND is_deleted = false
            ORDER BY created_at DESC
            "#,
        )
        .fetch_all(&self.pool)
        .await?;

        Ok(sources)
    }

    pub async fn update_user_filter_settings(
        &self,
        id: &str,
        user_filter_mode: crate::models::UserFilterMode,
        user_whitelist: serde_json::Value,
        user_blacklist: serde_json::Value,
    ) -> Result<(), DatabaseError> {
        sqlx::query(
            r#"
            UPDATE sources
            SET user_filter_mode = $2, user_whitelist = $3, user_blacklist = $4, updated_at = CURRENT_TIMESTAMP
            WHERE id = $1
            "#
        )
        .bind(id)
        .bind(user_filter_mode)
        .bind(user_whitelist)
        .bind(user_blacklist)
        .execute(&self.pool)
        .await?;

        Ok(())
    }

    pub async fn find_active_by_types(
        &self,
        source_types: Vec<crate::models::SourceType>,
    ) -> Result<Vec<Source>, DatabaseError> {
        let mut query_builder = sqlx::QueryBuilder::new(
            r#"
            SELECT id, name, source_type, config, is_active, is_deleted,
                   user_filter_mode, user_whitelist, user_blacklist,
                   connector_state, sync_interval_seconds, created_at, updated_at, created_by
            FROM sources
            WHERE is_active = true AND is_deleted = false
            "#,
        );

        if !source_types.is_empty() {
            query_builder.push(" AND source_type IN (");
            let mut separated = query_builder.separated(", ");
            for source_type in source_types {
                separated.push_bind(source_type);
            }
            query_builder.push(")");
        }

        query_builder.push(" ORDER BY created_at DESC");

        let sources = query_builder
            .build_query_as::<Source>()
            .fetch_all(&self.pool)
            .await?;

        Ok(sources)
    }

    pub async fn update_connector_state(
        &self,
        id: &str,
        connector_state: serde_json::Value,
    ) -> Result<(), DatabaseError> {
        sqlx::query(
            "UPDATE sources SET connector_state = $1, updated_at = CURRENT_TIMESTAMP WHERE id = $2",
        )
        .bind(&connector_state)
        .bind(id)
        .execute(&self.pool)
        .await?;

        Ok(())
    }

    pub async fn get_document_count(&self, id: &str) -> Result<i64, DatabaseError> {
        let result: (i64,) = sqlx::query_as("SELECT COUNT(*) FROM documents WHERE source_id = $1")
            .bind(id)
            .fetch_one(&self.pool)
            .await?;
        Ok(result.0)
    }

    pub async fn get_document_counts_by_source(&self) -> Result<Vec<(String, i64)>, DatabaseError> {
        let results: Vec<(String, i64)> =
            sqlx::query_as("SELECT source_id, COUNT(*) FROM documents GROUP BY source_id")
                .fetch_all(&self.pool)
                .await?;
        Ok(results)
    }

    pub async fn fetch_source_type_map(
        &self,
        source_ids: &[String],
    ) -> Result<HashMap<String, String>, DatabaseError> {
        if source_ids.is_empty() {
            return Ok(HashMap::new());
        }
        let rows: Vec<(String, String)> =
            sqlx::query_as("SELECT id, source_type::text FROM sources WHERE id = ANY($1)")
                .bind(source_ids)
                .fetch_all(&self.pool)
                .await?;

        Ok(rows.into_iter().collect())
    }

    pub async fn find_due_for_sync(
        &self,
        now: OffsetDateTime,
    ) -> Result<Vec<Source>, DatabaseError> {
        let sources = sqlx::query_as::<_, Source>(
            r#"
            SELECT s.id, s.name, s.source_type, s.config, s.is_active, s.is_deleted,
                   s.user_filter_mode, s.user_whitelist, s.user_blacklist,
                   s.connector_state, s.sync_interval_seconds, s.created_at, s.updated_at, s.created_by
            FROM sources s
            LEFT JOIN LATERAL (
                SELECT completed_at FROM sync_runs
                WHERE source_id = s.id AND status = 'completed'
                ORDER BY completed_at DESC LIMIT 1
            ) lr ON true
            WHERE s.is_active = true AND s.is_deleted = false
              AND s.sync_interval_seconds IS NOT NULL
              AND (lr.completed_at IS NULL
                   OR lr.completed_at + (s.sync_interval_seconds || ' seconds')::interval <= $1)
            ORDER BY lr.completed_at ASC NULLS FIRST
            LIMIT 10
            "#,
        )
        .bind(now)
        .fetch_all(&self.pool)
        .await?;

        Ok(sources)
    }
}

#[async_trait]
impl Repository<Source, String> for SourceRepository {
    async fn find_by_id(&self, id: String) -> Result<Option<Source>, DatabaseError> {
        let source = sqlx::query_as::<_, Source>(
            r#"
            SELECT id, name, source_type, config, is_active, is_deleted,
                   user_filter_mode, user_whitelist, user_blacklist,
                   connector_state, sync_interval_seconds, created_at, updated_at, created_by
            FROM sources
            WHERE id = $1
            "#,
        )
        .bind(id)
        .fetch_optional(&self.pool)
        .await?;

        Ok(source)
    }

    async fn find_all(&self, limit: i64, offset: i64) -> Result<Vec<Source>, DatabaseError> {
        let sources = sqlx::query_as::<_, Source>(
            r#"
            SELECT id, name, source_type, config, is_active, is_deleted,
                   user_filter_mode, user_whitelist, user_blacklist,
                   connector_state, sync_interval_seconds, created_at, updated_at, created_by
            FROM sources
            WHERE is_deleted = false
            ORDER BY created_at DESC
            LIMIT $1 OFFSET $2
            "#,
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
            INSERT INTO sources (id, name, source_type, config, is_active, created_by)
            VALUES ($1, $2, $3, $4, $5, $6)
            RETURNING id, name, source_type, config, is_active, is_deleted,
                      user_filter_mode, user_whitelist, user_blacklist,
                      connector_state, sync_interval_seconds, created_at, updated_at, created_by
            "#,
        )
        .bind(&source.id)
        .bind(&source.name)
        .bind(&source.source_type)
        .bind(&source.config)
        .bind(source.is_active)
        .bind(&source.created_by)
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
            SET name = $2, source_type = $3, config = $4, is_active = $5, updated_at = CURRENT_TIMESTAMP
            WHERE id = $1
            RETURNING id, name, source_type, config, is_active, is_deleted,
                      user_filter_mode, user_whitelist, user_blacklist,
                      connector_state, sync_interval_seconds, created_at, updated_at, created_by
            "#,
        )
        .bind(&id)
        .bind(&source.name)
        .bind(&source.source_type)
        .bind(&source.config)
        .bind(source.is_active)
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

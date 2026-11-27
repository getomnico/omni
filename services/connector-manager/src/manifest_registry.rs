use redis::AsyncCommands;
use shared::DatabasePool;
use thiserror::Error;

use crate::models::ConnectorManifest;

#[derive(Debug, Error)]
pub enum ManifestRegistryError {
    #[error("database error: {0}")]
    Database(#[from] sqlx::Error),
    #[error("redis error: {0}")]
    Redis(#[from] redis::RedisError),
    #[error("manifest serialization error: {0}")]
    Serialization(#[from] serde_json::Error),
    #[error("invalid connector manifest: {0}")]
    Invalid(String),
}

fn is_valid_source_type_slug(value: &str) -> bool {
    let mut bytes = value.bytes();
    value.len() <= 50
        && matches!(bytes.next(), Some(b'a'..=b'z'))
        && bytes.all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || matches!(byte, b'_' | b'-'))
}

fn same_identity(left: &ConnectorManifest, right: &ConnectorManifest) -> bool {
    left.connector_id == right.connector_id
        && left.connector_url == right.connector_url
        && left.integration_type == right.integration_type
        && left.source_types.iter().collect::<std::collections::HashSet<_>>()
            == right.source_types.iter().collect::<std::collections::HashSet<_>>()
}

/// Serialize manifest ownership checks and publication across native and remote MCP registrations.
pub async fn publish_manifest(
    db_pool: &DatabasePool,
    redis_client: &redis::Client,
    manifest: &ConnectorManifest,
    ttl_seconds: u64,
    preserve_existing_identity: bool,
) -> Result<(), ManifestRegistryError> {
    let mut unique_types = std::collections::HashSet::new();
    for source_type in &manifest.source_types {
        if !is_valid_source_type_slug(source_type) || !unique_types.insert(source_type) {
            return Err(ManifestRegistryError::Invalid(format!(
                "invalid or duplicate source type slug: {source_type:?}"
            )));
        }
    }
    if manifest.integration_type == shared::models::IntegrationType::RemoteMcp
        && (manifest.source_types.len() != 1
            || manifest.connector_id
                != format!("remote_mcp:{}", manifest.source_types[0]))
    {
        return Err(ManifestRegistryError::Invalid(
            "remote MCP manifest identity must match its single source type".to_string(),
        ));
    }
    let expected_key = format!("connector:manifest:{}", manifest.connector_id);
    let mut tx = db_pool.pool().begin().await?;
    sqlx::query("SELECT pg_advisory_xact_lock(504, 1)")
        .execute(&mut *tx)
        .await?;

    let mut conn = redis_client.get_multiplexed_async_connection().await?;
    let mut cursor = 0_u64;
    let mut manifests = Vec::new();
    loop {
        let (next_cursor, keys): (u64, Vec<String>) = redis::cmd("SCAN")
            .cursor_arg(cursor)
            .arg("MATCH")
            .arg("connector:manifest:*")
            .arg("COUNT")
            .arg(100)
            .query_async(&mut conn)
            .await?;
        for key in keys {
            let value: Option<String> = conn.get(&key).await?;
            if let Some(value) = value {
                let existing: ConnectorManifest = serde_json::from_str(&value)?;
                if key != expected_key {
                    manifests.push(existing);
                } else if existing.integration_type != manifest.integration_type {
                    return Err(ManifestRegistryError::Invalid(format!(
                        "connector '{}' cannot change integration type",
                        manifest.connector_id
                    )));
                } else if preserve_existing_identity && !same_identity(&existing, manifest) {
                    return Err(ManifestRegistryError::Invalid(format!(
                        "connector '{}' refresh changed its registration identity",
                        manifest.connector_id
                    )));
                }
            }
        }
        if next_cursor == 0 {
            break;
        }
        cursor = next_cursor;
    }

    for existing in manifests {
        for source_type in &manifest.source_types {
            if existing.source_types.contains(source_type) {
                return Err(ManifestRegistryError::Invalid(format!(
                    "source type '{}' is already registered by '{}'",
                    source_type, existing.connector_id
                )));
            }
        }
    }

    let value = serde_json::to_string(manifest)?;
    let _: () = conn
        .set_ex(expected_key, value, ttl_seconds)
        .await?;
    tx.commit().await?;
    Ok(())
}

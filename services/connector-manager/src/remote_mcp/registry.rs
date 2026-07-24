use crate::remote_mcp::gateway::{GatewayError, RemoteMcpGateway, REMOTE_MCP_CONNECTOR_ID_PREFIX};
use futures::{stream, StreamExt};
use redis::AsyncCommands;
use serde_json::json;
use shared::db::repositories::SourceRepository;
use shared::models::{IntegrationType, Source};
use shared::DatabasePool;
use std::collections::{HashMap, HashSet};
use std::env;
use std::time::Duration;
use tracing::{info, warn};

const MAX_MCP_STARTUP_DISCOVERY: usize = 4;
const REMOTE_MCP_RECONCILE_INTERVAL_SECONDS: u64 = 240;

pub async fn startup_register_remote_mcp_sources(gateway: RemoteMcpGateway, db_pool: DatabasePool) {
    match active_usable_remote_mcp_sources(&db_pool).await {
        Ok(sources) => {
            stream::iter(sources)
                .for_each_concurrent(MAX_MCP_STARTUP_DISCOVERY, |source| {
                    let gateway = gateway.clone();
                    async move {
                        if let Err(error) = gateway.discover_and_register(&source.id).await {
                            warn!(source_id = %source.id, error = %redact_gateway_error(&error), "Remote MCP startup registration failed");
                        }
                    }
                })
                .await;
        }
        Err(error) => warn!(error = %error, "Remote MCP startup source query failed"),
    }
}

pub async fn run_remote_mcp_registry_loop(gateway: RemoteMcpGateway, db_pool: DatabasePool) {
    let mut interval =
        tokio::time::interval(Duration::from_secs(REMOTE_MCP_RECONCILE_INTERVAL_SECONDS));
    loop {
        interval.tick().await;
        if let Err(error) = reconcile_remote_mcp_sources(&gateway, &db_pool).await {
            warn!(error = %redact_gateway_error(&error), "Remote MCP registry reconciliation failed");
        }
    }
}

pub async fn reconcile_remote_mcp_sources(
    gateway: &RemoteMcpGateway,
    db_pool: &DatabasePool,
) -> Result<(), GatewayError> {
    let usable_sources = active_usable_remote_mcp_sources(db_pool).await?;
    let active_source_types: HashSet<String> = usable_sources
        .iter()
        .map(|source| source.source_type.clone())
        .collect();
    let active_source_ids: HashSet<String> = usable_sources
        .iter()
        .map(|source| source.id.clone())
        .collect();

    for source in usable_sources {
        if let Err(error) = gateway.refresh_catalog(&source.id).await {
            warn!(source_id = %source.id, source_type = %source.source_type, error = %redact_gateway_error(&error), "Remote MCP catalog refresh failed; existing Redis manifest left untouched");
        }
    }

    remove_stale_remote_mcp_manifests(gateway, &active_source_types).await?;
    remove_stale_remote_mcp_capabilities(db_pool, &active_source_ids).await?;
    Ok(())
}

async fn active_usable_remote_mcp_sources(
    db_pool: &DatabasePool,
) -> Result<Vec<Source>, GatewayError> {
    let repo = SourceRepository::new(db_pool.pool());
    let sources = repo.find_active_sources().await?;
    Ok(reject_conflicting_remote_mcp_sources(sources))
}

fn reject_conflicting_remote_mcp_sources(sources: Vec<Source>) -> Vec<Source> {
    let native_source_types: HashSet<String> = sources
        .iter()
        .filter(|source| source.integration_type == IntegrationType::Connector)
        .map(|source| source.source_type.clone())
        .collect();
    let remote_sources = sources
        .into_iter()
        .filter(|source| source.integration_type == IntegrationType::RemoteMcp);

    let mut by_slug: HashMap<String, Vec<Source>> = HashMap::new();
    for source in remote_sources {
        by_slug
            .entry(source.source_type.clone())
            .or_default()
            .push(source);
    }

    let mut usable = Vec::new();
    for (source_type, mut sources) in by_slug {
        if native_source_types.contains(&source_type) {
            let ids: Vec<String> = sources.iter().map(|source| source.id.clone()).collect();
            warn!(source_type = %source_type, source_ids = ?ids, "Native/MCP slug conflict detected; skipping remote MCP rows");
            continue;
        }
        if sources.len() > 1 {
            let ids: Vec<String> = sources.iter().map(|source| source.id.clone()).collect();
            warn!(source_type = %source_type, source_ids = ?ids, "Remote MCP slug conflict detected; skipping conflicted rows");
            continue;
        }
        if let Some(source) = sources.pop() {
            usable.push(source);
        }
    }
    usable
}

async fn remove_stale_remote_mcp_manifests(
    gateway: &RemoteMcpGateway,
    active_source_types: &HashSet<String>,
) -> Result<(), GatewayError> {
    let redis_client = gateway_redis_client(gateway);
    let mut conn = redis_client.get_multiplexed_async_connection().await?;
    let keys = scan_redis_keys(
        &mut conn,
        &format!("connector:manifest:{REMOTE_MCP_CONNECTOR_ID_PREFIX}*"),
    )
    .await?;
    for key in keys {
        if let Some(source_type) = source_type_from_manifest_key(&key) {
            if !active_source_types.contains(source_type) {
                let _: () = conn.del(&key).await?;
                info!(source_type = %source_type, "Removed stale remote MCP manifest from Redis");
            }
        }
    }
    Ok(())
}

async fn remove_stale_remote_mcp_capabilities(
    db_pool: &DatabasePool,
    active_source_ids: &HashSet<String>,
) -> Result<(), GatewayError> {
    let all_remote_source_ids: Vec<String> = sqlx::query_scalar(
        r#"
        SELECT id
        FROM sources
        WHERE integration_type = 'remote_mcp'
        "#,
    )
    .fetch_all(db_pool.pool())
    .await
    .map_err(shared::db::error::DatabaseError::from)?;

    let stale_source_ids = stale_remote_mcp_source_ids(&all_remote_source_ids, active_source_ids);
    if stale_source_ids.is_empty() {
        return Ok(());
    }

    publish_empty_remote_mcp_capability_catalogs(&stale_source_ids).await;
    Ok(())
}

async fn publish_empty_remote_mcp_capability_catalogs(source_ids: &[String]) {
    let Ok(searcher_url) = env::var("SEARCHER_URL") else {
        warn!(
            source_ids = ?source_ids,
            "Stale remote MCP capability publishers detected but SEARCHER_URL is unset; AI/searcher capability sync will prune them on next publication."
        );
        return;
    };
    let client = reqwest::Client::new();
    let endpoint = format!("{}/capabilities/sync", searcher_url.trim_end_matches('/'));
    for source_id in source_ids {
        for capability_type in ["resource", "prompt"] {
            let response = client
                .post(&endpoint)
                .timeout(Duration::from_secs(10))
                .json(&json!({
                    "publisher_id": source_id,
                    "capability_type": capability_type,
                    "capabilities": [],
                }))
                .send()
                .await;
            match response {
                Ok(resp) if resp.status().is_success() => info!(
                    source_id = %source_id,
                    capability_type,
                    "Pruned stale remote MCP capabilities through searcher sync path"
                ),
                Ok(resp) => warn!(
                    source_id = %source_id,
                    capability_type,
                    status = %resp.status(),
                    "Searcher rejected stale remote MCP capability prune"
                ),
                Err(error) => warn!(
                    source_id = %source_id,
                    capability_type,
                    error = %error,
                    "Failed to prune stale remote MCP capabilities through searcher sync path"
                ),
            }
        }
    }
}

fn stale_remote_mcp_source_ids(
    all_remote_source_ids: &[String],
    active_source_ids: &HashSet<String>,
) -> Vec<String> {
    all_remote_source_ids
        .iter()
        .filter(|source_id| !active_source_ids.contains(source_id.as_str()))
        .cloned()
        .collect()
}

async fn scan_redis_keys(
    conn: &mut redis::aio::MultiplexedConnection,
    pattern: &str,
) -> redis::RedisResult<Vec<String>> {
    let mut cursor: u64 = 0;
    let mut keys = Vec::new();
    loop {
        let (next_cursor, batch): (u64, Vec<String>) = redis::cmd("SCAN")
            .cursor_arg(cursor)
            .arg("MATCH")
            .arg(pattern)
            .arg("COUNT")
            .arg(100)
            .query_async(conn)
            .await?;
        keys.extend(batch);
        if next_cursor == 0 {
            break;
        }
        cursor = next_cursor;
    }
    Ok(keys)
}

fn gateway_redis_client(gateway: &RemoteMcpGateway) -> redis::Client {
    // Keep Redis ownership inside the gateway API while allowing registry
    // cleanup to use the same configured client without adding global state.
    gateway.redis_client_for_registry()
}

pub fn source_type_from_manifest_key(key: &str) -> Option<&str> {
    key.strip_prefix(&format!(
        "connector:manifest:{REMOTE_MCP_CONNECTOR_ID_PREFIX}"
    ))
    .filter(|source_type| !source_type.is_empty())
}

fn redact_gateway_error(error: &GatewayError) -> String {
    match error {
        GatewayError::Protocol(message) => format!("protocol error: {}", redact_secretish(message)),
        other => other.to_string(),
    }
}

fn redact_secretish(value: &str) -> String {
    value
        .replace("Bearer ", "Bearer [redacted]")
        .replace("access_token", "[redacted_token]")
        .replace("token", "[redacted_token]")
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use shared::models::{SourceScope, UserFilterMode};
    use time::OffsetDateTime;

    fn source(id: &str, slug: &str) -> Source {
        Source {
            id: id.to_string(),
            name: slug.to_string(),
            source_type: slug.to_string(),
            integration_type: IntegrationType::RemoteMcp,
            config: json!({"endpoint_url":"https://example.com/mcp"}),
            is_active: true,
            is_deleted: false,
            scope: SourceScope::Org,
            user_filter_mode: UserFilterMode::All,
            user_whitelist: None,
            user_blacklist: None,
            connector_state: None,
            checkpoint: None,
            sync_interval_seconds: None,
            created_at: OffsetDateTime::now_utc(),
            updated_at: OffsetDateTime::now_utc(),
            created_by: "admin".to_string(),
        }
    }

    #[test]
    fn parses_remote_mcp_manifest_keys() {
        assert_eq!(
            source_type_from_manifest_key("connector:manifest:remote_mcp:acme"),
            Some("acme")
        );
        assert_eq!(
            source_type_from_manifest_key("connector:manifest:slack"),
            None
        );
    }

    #[test]
    fn conflicting_remote_mcp_slugs_are_skipped() {
        let usable = reject_conflicting_remote_mcp_sources(vec![
            source("one", "acme"),
            source("two", "acme"),
            source("three", "beta"),
        ]);
        assert_eq!(usable.len(), 1);
        assert_eq!(usable[0].source_type, "beta");
    }

    #[test]
    fn native_slug_conflicts_skip_remote_mcp_rows() {
        let mut native = source("native", "acme");
        native.integration_type = IntegrationType::Connector;
        let usable = reject_conflicting_remote_mcp_sources(vec![native, source("remote", "acme")]);
        assert!(usable.is_empty());
    }

    #[test]
    fn manifest_key_matches_registry_key_parser() {
        let key = crate::remote_mcp::gateway::manifest_key("acme");
        assert_eq!(source_type_from_manifest_key(&key), Some("acme"));
    }

    #[test]
    fn stale_remote_mcp_source_ids_exclude_active_ids() {
        let active_source_ids = HashSet::from(["active".to_string()]);
        assert_eq!(
            stale_remote_mcp_source_ids(
                &[
                    "active".to_string(),
                    "deleted".to_string(),
                    "inactive".to_string()
                ],
                &active_source_ids,
            ),
            vec!["deleted".to_string(), "inactive".to_string()]
        );
    }
}

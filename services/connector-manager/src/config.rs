use shared::models::SourceType;
use shared::{DatabaseConfig, RedisConfig};
use std::collections::HashMap;
use std::env;
use std::process;

#[derive(Debug, Clone)]
pub struct ConnectorManagerConfig {
    pub database: DatabaseConfig,
    pub redis: RedisConfig,
    pub port: u16,
    pub connector_urls: HashMap<SourceType, String>,
    pub max_concurrent_syncs: usize,
    pub max_concurrent_syncs_per_type: usize,
    pub scheduler_interval_seconds: u64,
    pub stale_sync_timeout_minutes: u64,
}

impl ConnectorManagerConfig {
    pub fn from_env() -> Self {
        let database = DatabaseConfig::from_env();
        let redis = RedisConfig::from_env();

        let port_str = env::var("PORT").unwrap_or_else(|_| "8090".to_string());
        let port = port_str.parse::<u16>().unwrap_or_else(|_| {
            eprintln!("ERROR: Invalid port number in 'PORT': '{}'", port_str);
            process::exit(1);
        });

        let mut connector_urls = HashMap::new();

        if let Ok(url) = env::var("CONNECTOR_GOOGLE_URL") {
            connector_urls.insert(SourceType::GoogleDrive, url.clone());
            connector_urls.insert(SourceType::Gmail, url);
        }
        if let Ok(url) = env::var("CONNECTOR_SLACK_URL") {
            connector_urls.insert(SourceType::Slack, url);
        }
        if let Ok(url) = env::var("CONNECTOR_ATLASSIAN_URL") {
            connector_urls.insert(SourceType::Confluence, url.clone());
            connector_urls.insert(SourceType::Jira, url);
        }
        if let Ok(url) = env::var("CONNECTOR_WEB_URL") {
            connector_urls.insert(SourceType::Web, url);
        }
        if let Ok(url) = env::var("CONNECTOR_GITHUB_URL") {
            connector_urls.insert(SourceType::Github, url);
        }
        if let Ok(url) = env::var("CONNECTOR_NOTION_URL") {
            connector_urls.insert(SourceType::Notion, url);
        }
        if let Ok(url) = env::var("CONNECTOR_HUBSPOT_URL") {
            connector_urls.insert(SourceType::Hubspot, url);
        }
        if let Ok(url) = env::var("CONNECTOR_MICROSOFT_URL") {
            connector_urls.insert(SourceType::Microsoft, url);
        }

        let max_concurrent_syncs = env::var("MAX_CONCURRENT_SYNCS")
            .unwrap_or_else(|_| "10".to_string())
            .parse::<usize>()
            .unwrap_or(10);

        let max_concurrent_syncs_per_type = env::var("MAX_CONCURRENT_SYNCS_PER_TYPE")
            .unwrap_or_else(|_| "3".to_string())
            .parse::<usize>()
            .unwrap_or(3);

        let scheduler_interval_seconds = env::var("SCHEDULER_INTERVAL_SECONDS")
            .unwrap_or_else(|_| "30".to_string())
            .parse::<u64>()
            .unwrap_or(30);

        let stale_sync_timeout_minutes = env::var("STALE_SYNC_TIMEOUT_MINUTES")
            .unwrap_or_else(|_| "10".to_string())
            .parse::<u64>()
            .unwrap_or(10);

        Self {
            database,
            redis,
            port,
            connector_urls,
            max_concurrent_syncs,
            max_concurrent_syncs_per_type,
            scheduler_interval_seconds,
            stale_sync_timeout_minutes,
        }
    }

    pub fn get_connector_url(&self, source_type: SourceType) -> Option<&String> {
        self.connector_urls.get(&source_type)
    }
}

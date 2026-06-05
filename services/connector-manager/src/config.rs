use shared::{DatabaseConfig, RedisConfig};
use std::env;
use std::process;

#[derive(Debug, Clone)]
pub struct ConnectorManagerConfig {
    pub database: DatabaseConfig,
    pub redis: RedisConfig,
    pub port: u16,
    pub max_concurrent_syncs: usize,
    pub max_concurrent_syncs_per_type: usize,
    pub scheduler_interval_seconds: u64,
    pub stale_sync_timeout_minutes: u64,
    pub extraction_concurrency: usize,
    pub extraction_retry_after_seconds: u64,
    pub sync_backoff_base_seconds: i64,
    pub sync_backoff_max_seconds: i64,
    pub sync_max_consecutive_failures: i32,
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
            .unwrap_or_else(|_| "60".to_string())
            .parse::<u64>()
            .unwrap_or(10);

        let extraction_concurrency = env::var("EXTRACTION_CONCURRENCY")
            .unwrap_or_else(|_| "2".to_string())
            .parse::<usize>()
            .unwrap_or(2)
            .max(1);

        let extraction_retry_after_seconds = env::var("EXTRACTION_RETRY_AFTER_SECONDS")
            .unwrap_or_else(|_| "30".to_string())
            .parse::<u64>()
            .unwrap_or(30);

        let sync_backoff_base_seconds = env::var("SYNC_BACKOFF_BASE_SECONDS")
            .unwrap_or_else(|_| "30".to_string())
            .parse::<i64>()
            .unwrap_or(30);

        let sync_backoff_max_seconds = env::var("SYNC_BACKOFF_MAX_SECONDS")
            .unwrap_or_else(|_| "3600".to_string())
            .parse::<i64>()
            .unwrap_or(3600);

        let sync_max_consecutive_failures = env::var("SYNC_MAX_CONSECUTIVE_FAILURES")
            .unwrap_or_else(|_| "10".to_string())
            .parse::<i32>()
            .unwrap_or(10);

        Self {
            database,
            redis,
            port,
            max_concurrent_syncs,
            max_concurrent_syncs_per_type,
            scheduler_interval_seconds,
            stale_sync_timeout_minutes,
            extraction_concurrency,
            extraction_retry_after_seconds,
            sync_backoff_base_seconds,
            sync_backoff_max_seconds,
            sync_max_consecutive_failures,
        }
    }
}

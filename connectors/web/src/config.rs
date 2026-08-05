use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use spider::configuration::{SpiderCloudConfig, SpiderCloudMode};
use spider::website::Website;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WebSourceConfig {
    pub root_url: String,
    #[serde(default = "default_max_depth")]
    pub max_depth: usize,
    #[serde(default = "default_max_pages")]
    pub max_pages: usize,
    #[serde(default = "default_respect_robots")]
    pub respect_robots_txt: bool,
    #[serde(default)]
    pub user_agent: Option<String>,
    #[serde(default)]
    pub blacklist_patterns: Vec<String>,
    #[serde(default)]
    pub include_subdomains: bool,
}

fn default_max_depth() -> usize {
    10
}

fn default_max_pages() -> usize {
    10_000
}

fn default_respect_robots() -> bool {
    true
}

/// Parse a `SPIDER_CLOUD_MODE` value.
///
/// Defaults to [`SpiderCloudMode::Smart`] for anything unrecognized so a typo
/// degrades to the most reliable mode instead of failing a sync. `Smart`
/// proxies every request and automatically escalates to the unblocker API when
/// bot protection is detected.
fn parse_spider_cloud_mode(mode: &str) -> SpiderCloudMode {
    match mode.trim().to_ascii_lowercase().as_str() {
        "proxy" => SpiderCloudMode::Proxy,
        "api" => SpiderCloudMode::Api,
        "unblocker" => SpiderCloudMode::Unblocker,
        "fallback" => SpiderCloudMode::Fallback,
        _ => SpiderCloudMode::Smart,
    }
}

/// Process-wide Spider Cloud config, resolved from the environment exactly once.
///
/// The environment is fixed at startup (`dotenv` runs in `main`), so this is
/// read on the first crawl and reused afterwards — no per-crawl `env::var`
/// lock and allocation.
static SPIDER_CLOUD: std::sync::OnceLock<Option<SpiderCloudConfig>> = std::sync::OnceLock::new();

/// Resolved Spider Cloud config, or `None` when the integration is not enabled.
fn spider_cloud() -> Option<&'static SpiderCloudConfig> {
    SPIDER_CLOUD.get_or_init(spider_cloud_from_env).as_ref()
}

/// Build a [`SpiderCloudConfig`] from the environment.
///
/// Returns `None` unless `SPIDER_CLOUD_API_KEY` is set to a non-empty value,
/// which keeps crawling on the existing direct-fetch path by default.
fn spider_cloud_from_env() -> Option<SpiderCloudConfig> {
    let api_key = std::env::var("SPIDER_CLOUD_API_KEY").ok()?;
    let api_key = api_key.trim();

    if api_key.is_empty() {
        return None;
    }

    let mode = std::env::var("SPIDER_CLOUD_MODE").unwrap_or_default();
    let mut cloud = SpiderCloudConfig::new(api_key).with_mode(parse_spider_cloud_mode(&mode));

    if let Ok(api_url) = std::env::var("SPIDER_CLOUD_API_URL") {
        if !api_url.trim().is_empty() {
            cloud = cloud.with_api_url(api_url.trim());
        }
    }

    Some(cloud)
}

impl WebSourceConfig {
    pub fn from_json(config: &serde_json::Value) -> Result<Self> {
        serde_json::from_value(config.clone()).context("Failed to parse web source configuration")
    }

    pub fn build_spider_website(&self) -> Result<Website> {
        let mut website = Website::new(&self.root_url);

        website
            .with_respect_robots_txt(self.respect_robots_txt)
            .with_subdomains(self.include_subdomains)
            .with_depth(self.max_depth)
            // Spider treats a limit of 0 as unlimited, matching `max_pages: 0`.
            .with_limit(self.max_pages.min(u32::MAX as usize) as u32)
            .with_delay(300);

        if let Some(user_agent) = &self.user_agent {
            website.with_user_agent(Some(user_agent.as_str()));
        }

        if !self.blacklist_patterns.is_empty() {
            for pattern in &self.blacklist_patterns {
                website.with_blacklist_url(Some(vec![pattern.as_str().into()]));
            }
        }

        // Opt-in only: without SPIDER_CLOUD_API_KEY this is a no-op and the
        // crawl uses the same direct-fetch path as before.
        if let Some(cloud) = spider_cloud() {
            tracing::info!(
                "Spider Cloud enabled ({:?}) for {}",
                cloud.mode,
                self.root_url
            );
            website.with_spider_cloud_config(cloud.clone());
        }

        Ok(website)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use spider::CaseInsensitiveString;

    #[test]
    fn test_parse_minimal_config() {
        let config = json!({
            "root_url": "https://example.com"
        });

        let web_config = WebSourceConfig::from_json(&config).unwrap();
        assert_eq!(web_config.root_url, "https://example.com");
        assert_eq!(web_config.max_depth, 10);
        assert_eq!(web_config.max_pages, 10_000);
        assert!(web_config.respect_robots_txt);
        assert!(!web_config.include_subdomains);
    }

    #[test]
    fn test_parse_full_config() {
        let config = json!({
            "root_url": "https://docs.example.com",
            "max_depth": 5,
            "max_pages": 1000,
            "respect_robots_txt": false,
            "user_agent": "MyBot/1.0",
            "blacklist_patterns": ["/admin", "/api"],
            "include_subdomains": true
        });

        let web_config = WebSourceConfig::from_json(&config).unwrap();
        assert_eq!(web_config.root_url, "https://docs.example.com");
        assert_eq!(web_config.max_depth, 5);
        assert_eq!(web_config.max_pages, 1000);
        assert!(!web_config.respect_robots_txt);
        assert_eq!(web_config.user_agent, Some("MyBot/1.0".to_string()));
        assert_eq!(web_config.blacklist_patterns.len(), 2);
        assert!(web_config.include_subdomains);
    }

    #[test]
    fn test_build_spider_website() {
        let config = WebSourceConfig {
            root_url: "https://example.com".to_string(),
            max_depth: 5,
            max_pages: 1000,
            respect_robots_txt: true,
            user_agent: Some("TestBot/1.0".to_string()),
            blacklist_patterns: vec!["/admin".to_string()],
            include_subdomains: false,
        };

        let website = config.build_spider_website();
        assert!(website.is_ok());
    }

    #[test]
    fn test_max_pages_is_applied_as_crawl_limit() {
        let config = WebSourceConfig {
            root_url: "https://example.com".to_string(),
            max_depth: 5,
            max_pages: 42,
            respect_robots_txt: true,
            user_agent: None,
            blacklist_patterns: vec![],
            include_subdomains: false,
        };

        let website = config.build_spider_website().unwrap();
        let budget = website
            .configuration
            .budget
            .as_ref()
            .expect("max_pages should configure a crawl budget");

        assert_eq!(budget.get(&CaseInsensitiveString::from("*")), Some(&42));
    }
  
    fn test_parse_spider_cloud_mode() {
        assert_eq!(parse_spider_cloud_mode("proxy"), SpiderCloudMode::Proxy);
        assert_eq!(parse_spider_cloud_mode("api"), SpiderCloudMode::Api);
        assert_eq!(
            parse_spider_cloud_mode("unblocker"),
            SpiderCloudMode::Unblocker
        );
        assert_eq!(
            parse_spider_cloud_mode("fallback"),
            SpiderCloudMode::Fallback
        );
        assert_eq!(parse_spider_cloud_mode("smart"), SpiderCloudMode::Smart);
        assert_eq!(parse_spider_cloud_mode(" SMART "), SpiderCloudMode::Smart);

        // Unrecognized values degrade to the most reliable mode.
        assert_eq!(parse_spider_cloud_mode(""), SpiderCloudMode::Smart);
        assert_eq!(parse_spider_cloud_mode("nonsense"), SpiderCloudMode::Smart);
    }
}

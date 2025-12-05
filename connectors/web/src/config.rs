use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
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
            .with_delay(300);

        if let Some(user_agent) = &self.user_agent {
            website.with_user_agent(Some(user_agent.as_str()));
        }

        if !self.blacklist_patterns.is_empty() {
            for pattern in &self.blacklist_patterns {
                website.with_blacklist_url(Some(vec![pattern.as_str().into()]));
            }
        }

        Ok(website)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

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
}

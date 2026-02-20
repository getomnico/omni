use shared::ConnectorConfig;
use std::env;
use tracing::info;

#[derive(Debug, Clone)]
pub struct AtlassianConnectorConfig {
    pub base: ConnectorConfig,
    pub webhook_url: Option<String>,
}

impl AtlassianConnectorConfig {
    pub fn from_env() -> Self {
        let base = ConnectorConfig::from_env();
        let webhook_url = Self::derive_webhook_url();

        if let Some(ref url) = webhook_url {
            info!("Atlassian webhook URL: {}", url);
        } else {
            info!("Webhooks disabled (OMNI_DOMAIN not set or is localhost)");
        }

        Self { base, webhook_url }
    }

    fn derive_webhook_url() -> Option<String> {
        let domain = env::var("OMNI_DOMAIN").ok()?;
        if domain.is_empty() || domain == "localhost" {
            return None;
        }
        Some(format!("https://{}/webhook/atlassian", domain))
    }
}

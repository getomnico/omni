use std::env;
use std::process;
use tracing::{error, info};

fn get_required_env(key: &str) -> String {
    env::var(key).unwrap_or_else(|_| {
        error!("Required environment variable '{}' is not set", key);
        process::exit(1);
    })
}

fn parse_port(port_str: &str, var_name: &str) -> u16 {
    port_str.parse::<u16>().unwrap_or_else(|_| {
        error!("Invalid port number in '{}': '{}'", var_name, port_str);
        process::exit(1)
    })
}

#[derive(Debug, Clone)]
pub struct GoogleConnectorConfig {
    pub port: u16,
    pub webhook_url: Option<String>,
    pub webhook_renewal_interval_seconds: u64,
}

impl GoogleConnectorConfig {
    pub fn from_env() -> Self {
        let port_str = get_required_env("PORT");
        let port = parse_port(&port_str, "PORT");

        let webhook_url = Self::derive_webhook_url();

        let webhook_renewal_interval_seconds = env::var("WEBHOOK_RENEWAL_CHECK_INTERVAL_SECONDS")
            .unwrap_or_else(|_| "3600".to_string())
            .parse::<u64>()
            .unwrap_or(3600);

        Self {
            port,
            webhook_url,
            webhook_renewal_interval_seconds,
        }
    }

    fn derive_webhook_url() -> Option<String> {
        let domain = env::var("OMNI_DOMAIN").ok()?;
        let domain = domain.trim().to_string();

        if domain.is_empty() || domain == "localhost" {
            info!("OMNI_DOMAIN is '{}', webhooks disabled", domain);
            return None;
        }

        let url = format!("https://{}/google-webhook", domain);
        info!("Derived webhook URL: {}", url);
        Some(url)
    }
}

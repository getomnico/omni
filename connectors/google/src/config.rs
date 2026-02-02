use shared::RedisConfig;
use std::env;
use std::process;
use tracing::error;

fn get_required_env(key: &str) -> String {
    env::var(key).unwrap_or_else(|_| {
        error!("Required environment variable '{}' is not set", key);
        process::exit(1);
    })
}

fn validate_url(url: &str, var_name: &str) -> String {
    if url.is_empty() {
        error!("Environment variable '{}' cannot be empty", var_name);
        process::exit(1);
    }

    if !url.starts_with("http://")
        && !url.starts_with("https://")
        && !url.starts_with("redis://")
        && !url.starts_with("postgresql://")
    {
        error!("Invalid URL format in '{}': '{}'", var_name, url);
        process::exit(1);
    }

    url.to_string()
}

fn parse_port(port_str: &str, var_name: &str) -> u16 {
    port_str.parse::<u16>().unwrap_or_else(|_| {
        error!("Invalid port number in '{}': '{}'", var_name, port_str);
        process::exit(1)
    })
}

#[derive(Debug, Clone)]
pub struct GoogleConnectorConfig {
    pub redis: RedisConfig,
    pub port: u16,
    pub webhook_url: Option<String>,
    pub ai_service_url: String,
}

impl GoogleConnectorConfig {
    pub fn from_env() -> Self {
        let redis = RedisConfig::from_env();

        let port_str = get_required_env("PORT");
        let port = parse_port(&port_str, "PORT");

        let webhook_url = env::var("GOOGLE_WEBHOOK_URL").ok();
        if let Some(ref url) = webhook_url {
            if !url.trim().is_empty() {
                validate_url(url, "GOOGLE_WEBHOOK_URL");
                if !url.starts_with("https://") {
                    error!("GOOGLE_WEBHOOK_URL must use HTTPS for Google webhooks");
                    process::exit(1);
                }
            }
        }

        let ai_service_url = get_required_env("AI_SERVICE_URL");
        let ai_service_url = validate_url(&ai_service_url, "AI_SERVICE_URL");

        Self {
            redis,
            port,
            webhook_url,
            ai_service_url,
        }
    }
}

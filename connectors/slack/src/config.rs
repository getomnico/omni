use shared::{DatabaseConfig, RedisConfig};
use std::env;
use std::process;
use tracing::error;

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
pub struct SlackConnectorConfig {
    pub redis: RedisConfig,
    pub port: u16,
    pub database: DatabaseConfig,
    pub bot_token: String,
}

impl SlackConnectorConfig {
    pub fn from_env() -> Self {
        let redis = RedisConfig::from_env();

        let port_str = get_required_env("PORT");
        let port = parse_port(&port_str, "PORT");

        let database = DatabaseConfig::from_env();

        let bot_token = get_required_env("SLACK_BOT_TOKEN");
        if bot_token.trim().is_empty() || !bot_token.starts_with("xoxb-") {
            error!("SLACK_BOT_TOKEN must be a valid Slack bot token starting with 'xoxb-'");
            process::exit(1);
        }

        Self {
            redis,
            port,
            database,
            bot_token,
        }
    }
}

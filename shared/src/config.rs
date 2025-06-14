use std::env;
use std::process;

#[derive(Debug, Clone)]
pub struct DatabaseConfig {
    pub database_url: String,
    pub max_connections: u32,
    pub acquire_timeout_seconds: u64,
}

#[derive(Debug, Clone)]
pub struct RedisConfig {
    pub redis_url: String,
}

#[derive(Debug, Clone)]
pub struct SearcherConfig {
    pub database: DatabaseConfig,
    pub redis: RedisConfig,
    pub port: u16,
    pub ai_service_url: String,
}

#[derive(Debug, Clone)]
pub struct IndexerConfig {
    pub database: DatabaseConfig,
    pub redis: RedisConfig,
    pub port: u16,
    pub ai_service_url: String,
}

#[derive(Debug, Clone)]
pub struct AIServiceConfig {
    pub database: DatabaseConfig,
    pub redis: RedisConfig,
    pub port: u16,
    pub model_path: String,
    pub embedding_model: String,
    pub embedding_dimensions: u32,
    pub vllm_url: String,
}

#[derive(Debug, Clone)]
pub struct ConnectorConfig {
    pub redis: RedisConfig,
    pub port: u16,
}

#[derive(Debug, Clone)]
pub struct GoogleConnectorConfig {
    pub base: ConnectorConfig,
    pub client_id: String,
    pub client_secret: String,
    pub redirect_uri: String,
}

#[derive(Debug, Clone)]
pub struct SlackConnectorConfig {
    pub base: ConnectorConfig,
    pub client_id: String,
    pub client_secret: String,
    pub bot_token: String,
}

#[derive(Debug, Clone)]
pub struct AtlassianConnectorConfig {
    pub base: ConnectorConfig,
    pub client_id: String,
    pub client_secret: String,
}

fn get_required_env(key: &str) -> String {
    env::var(key).unwrap_or_else(|_| {
        eprintln!("ERROR: Required environment variable '{}' is not set", key);
        eprintln!("Please set this variable in your .env file or environment");
        process::exit(1);
    })
}

fn get_optional_env(key: &str, default: &str) -> String {
    env::var(key).unwrap_or_else(|_| default.to_string())
}

fn parse_port(port_str: &str, var_name: &str) -> u16 {
    port_str.parse::<u16>().unwrap_or_else(|_| {
        eprintln!("ERROR: Invalid port number in '{}': '{}'", var_name, port_str);
        eprintln!("Port must be a number between 1 and 65535");
        process::exit(1);
    })
}

fn validate_url(url: &str, var_name: &str) -> String {
    if url.is_empty() {
        eprintln!("ERROR: Environment variable '{}' cannot be empty", var_name);
        process::exit(1);
    }
    
    if !url.starts_with("http://") && !url.starts_with("https://") && !url.starts_with("redis://") && !url.starts_with("postgresql://") {
        eprintln!("ERROR: Invalid URL format in '{}': '{}'", var_name, url);
        eprintln!("URL must start with http://, https://, redis://, or postgresql://");
        process::exit(1);
    }
    
    url.to_string()
}

impl DatabaseConfig {
    pub fn from_env() -> Self {
        let database_url = get_required_env("DATABASE_URL");
        let database_url = validate_url(&database_url, "DATABASE_URL");
        
        let max_connections_str = get_optional_env("DB_MAX_CONNECTIONS", "10");
        let max_connections = max_connections_str.parse::<u32>().unwrap_or_else(|_| {
            eprintln!("ERROR: Invalid max connections in 'DB_MAX_CONNECTIONS': '{}'", max_connections_str);
            eprintln!("Must be a positive number");
            process::exit(1);
        });
        
        let acquire_timeout_str = get_optional_env("DB_ACQUIRE_TIMEOUT_SECONDS", "3");
        let acquire_timeout_seconds = acquire_timeout_str.parse::<u64>().unwrap_or_else(|_| {
            eprintln!("ERROR: Invalid timeout in 'DB_ACQUIRE_TIMEOUT_SECONDS': '{}'", acquire_timeout_str);
            eprintln!("Must be a positive number");
            process::exit(1);
        });
        
        Self { 
            database_url,
            max_connections,
            acquire_timeout_seconds,
        }
    }
}

impl RedisConfig {
    pub fn from_env() -> Self {
        let redis_url = get_required_env("REDIS_URL");
        let redis_url = validate_url(&redis_url, "REDIS_URL");
        
        Self { redis_url }
    }
}

impl SearcherConfig {
    pub fn from_env() -> Self {
        let database = DatabaseConfig::from_env();
        let redis = RedisConfig::from_env();
        
        let port_str = get_required_env("PORT");
        let port = parse_port(&port_str, "PORT");
        
        let ai_service_url = get_required_env("AI_SERVICE_URL");
        let ai_service_url = validate_url(&ai_service_url, "AI_SERVICE_URL");
        
        Self {
            database,
            redis,
            port,
            ai_service_url,
        }
    }
}

impl IndexerConfig {
    pub fn from_env() -> Self {
        let database = DatabaseConfig::from_env();
        let redis = RedisConfig::from_env();
        
        let port_str = get_required_env("PORT");
        let port = parse_port(&port_str, "PORT");
        
        let ai_service_url = get_required_env("AI_SERVICE_URL");
        let ai_service_url = validate_url(&ai_service_url, "AI_SERVICE_URL");
        
        Self {
            database,
            redis,
            port,
            ai_service_url,
        }
    }
}

impl AIServiceConfig {
    pub fn from_env() -> Self {
        let database = DatabaseConfig::from_env();
        let redis = RedisConfig::from_env();
        
        let port_str = get_required_env("PORT");
        let port = parse_port(&port_str, "PORT");
        
        let model_path = get_required_env("MODEL_PATH");
        if model_path.is_empty() {
            eprintln!("ERROR: MODEL_PATH cannot be empty");
            process::exit(1);
        }
        
        let embedding_model = get_required_env("EMBEDDING_MODEL");
        if embedding_model.is_empty() {
            eprintln!("ERROR: EMBEDDING_MODEL cannot be empty");
            process::exit(1);
        }
        
        let embedding_dimensions_str = get_required_env("EMBEDDING_DIMENSIONS");
        let embedding_dimensions = embedding_dimensions_str.parse::<u32>().unwrap_or_else(|_| {
            eprintln!("ERROR: Invalid embedding dimensions in 'EMBEDDING_DIMENSIONS': '{}'", embedding_dimensions_str);
            eprintln!("Must be a positive number");
            process::exit(1);
        });
        
        let vllm_url = get_required_env("VLLM_URL");
        let vllm_url = validate_url(&vllm_url, "VLLM_URL");
        
        Self {
            database,
            redis,
            port,
            model_path,
            embedding_model,
            embedding_dimensions,
            vllm_url,
        }
    }
}

impl ConnectorConfig {
    pub fn from_env() -> Self {
        let redis = RedisConfig::from_env();
        
        let port_str = get_required_env("PORT");
        let port = parse_port(&port_str, "PORT");
        
        Self { redis, port }
    }
}

impl GoogleConnectorConfig {
    pub fn from_env() -> Self {
        let base = ConnectorConfig::from_env();
        
        let client_id = get_required_env("GOOGLE_CLIENT_ID");
        if client_id.trim().is_empty() || client_id == "your-google-client-id" {
            eprintln!("ERROR: GOOGLE_CLIENT_ID must be set to a valid Google OAuth client ID");
            eprintln!("Please configure your Google OAuth credentials");
            process::exit(1);
        }
        
        let client_secret = get_required_env("GOOGLE_CLIENT_SECRET");
        if client_secret.trim().is_empty() || client_secret == "your-google-client-secret" {
            eprintln!("ERROR: GOOGLE_CLIENT_SECRET must be set to a valid Google OAuth client secret");
            eprintln!("Please configure your Google OAuth credentials");
            process::exit(1);
        }
        
        let redirect_uri = get_required_env("GOOGLE_REDIRECT_URI");
        let redirect_uri = validate_url(&redirect_uri, "GOOGLE_REDIRECT_URI");
        
        Self {
            base,
            client_id,
            client_secret,
            redirect_uri,
        }
    }
}

impl SlackConnectorConfig {
    pub fn from_env() -> Self {
        let base = ConnectorConfig::from_env();
        
        let client_id = get_required_env("SLACK_CLIENT_ID");
        if client_id.trim().is_empty() || client_id == "your-slack-client-id" {
            eprintln!("ERROR: SLACK_CLIENT_ID must be set to a valid Slack OAuth client ID");
            eprintln!("Please configure your Slack OAuth credentials");
            process::exit(1);
        }
        
        let client_secret = get_required_env("SLACK_CLIENT_SECRET");
        if client_secret.trim().is_empty() || client_secret == "your-slack-client-secret" {
            eprintln!("ERROR: SLACK_CLIENT_SECRET must be set to a valid Slack OAuth client secret");
            eprintln!("Please configure your Slack OAuth credentials");
            process::exit(1);
        }
        
        let bot_token = get_required_env("SLACK_BOT_TOKEN");
        if bot_token.trim().is_empty() || bot_token == "your-slack-bot-token" {
            eprintln!("ERROR: SLACK_BOT_TOKEN must be set to a valid Slack bot token");
            eprintln!("Please configure your Slack bot token");
            process::exit(1);
        }
        
        Self {
            base,
            client_id,
            client_secret,
            bot_token,
        }
    }
}

impl AtlassianConnectorConfig {
    pub fn from_env() -> Self {
        let base = ConnectorConfig::from_env();
        
        let client_id = get_required_env("ATLASSIAN_CLIENT_ID");
        if client_id.trim().is_empty() || client_id == "your-atlassian-client-id" {
            eprintln!("ERROR: ATLASSIAN_CLIENT_ID must be set to a valid Atlassian OAuth client ID");
            eprintln!("Please configure your Atlassian OAuth credentials");
            process::exit(1);
        }
        
        let client_secret = get_required_env("ATLASSIAN_CLIENT_SECRET");
        if client_secret.trim().is_empty() || client_secret == "your-atlassian-client-secret" {
            eprintln!("ERROR: ATLASSIAN_CLIENT_SECRET must be set to a valid Atlassian OAuth client secret");
            eprintln!("Please configure your Atlassian OAuth credentials");
            process::exit(1);
        }
        
        Self {
            base,
            client_id,
            client_secret,
        }
    }
}
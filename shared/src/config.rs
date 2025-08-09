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
    pub typo_tolerance_enabled: bool,
    pub typo_tolerance_max_distance: i32,
    pub typo_tolerance_min_word_length: usize,
    pub hybrid_search_fts_weight: f32,
    pub hybrid_search_semantic_weight: f32,
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
    pub database: DatabaseConfig,
    pub webhook_url: Option<String>,
    pub ai_service_url: String,
}

#[derive(Debug, Clone)]
pub struct SlackConnectorConfig {
    pub base: ConnectorConfig,
    pub database: DatabaseConfig,
    pub bot_token: String,
}

#[derive(Debug, Clone)]
pub struct AtlassianConnectorConfig {
    pub base: ConnectorConfig,
    pub database: DatabaseConfig,
    pub base_url: String,
    pub user_email: String,
    pub api_token: String,
}

#[derive(Debug, Clone)]
pub struct FilesystemConnectorConfig {
    pub database: DatabaseConfig,
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
        eprintln!(
            "ERROR: Invalid port number in '{}': '{}'",
            var_name, port_str
        );
        eprintln!("Port must be a number between 1 and 65535");
        process::exit(1);
    })
}

fn validate_url(url: &str, var_name: &str) -> String {
    if url.is_empty() {
        eprintln!("ERROR: Environment variable '{}' cannot be empty", var_name);
        process::exit(1);
    }

    if !url.starts_with("http://")
        && !url.starts_with("https://")
        && !url.starts_with("redis://")
        && !url.starts_with("postgresql://")
    {
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
            eprintln!(
                "ERROR: Invalid max connections in 'DB_MAX_CONNECTIONS': '{}'",
                max_connections_str
            );
            eprintln!("Must be a positive number");
            process::exit(1);
        });

        let acquire_timeout_str = get_optional_env("DB_ACQUIRE_TIMEOUT_SECONDS", "3");
        let acquire_timeout_seconds = acquire_timeout_str.parse::<u64>().unwrap_or_else(|_| {
            eprintln!(
                "ERROR: Invalid timeout in 'DB_ACQUIRE_TIMEOUT_SECONDS': '{}'",
                acquire_timeout_str
            );
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

        let typo_tolerance_enabled = get_optional_env("TYPO_TOLERANCE_ENABLED", "true")
            .parse::<bool>()
            .unwrap_or(true);

        let typo_tolerance_max_distance = get_optional_env("TYPO_TOLERANCE_MAX_DISTANCE", "2")
            .parse::<i32>()
            .unwrap_or_else(|_| {
                eprintln!("ERROR: Invalid value for TYPO_TOLERANCE_MAX_DISTANCE");
                eprintln!("Must be a positive integer");
                process::exit(1);
            });

        let typo_tolerance_min_word_length =
            get_optional_env("TYPO_TOLERANCE_MIN_WORD_LENGTH", "4")
                .parse::<usize>()
                .unwrap_or_else(|_| {
                    eprintln!("ERROR: Invalid value for TYPO_TOLERANCE_MIN_WORD_LENGTH");
                    eprintln!("Must be a positive integer");
                    process::exit(1);
                });

        let hybrid_search_fts_weight = get_optional_env("HYBRID_SEARCH_FTS_WEIGHT", "0.3")
            .parse::<f32>()
            .unwrap_or_else(|_| {
                eprintln!("ERROR: Invalid value for HYBRID_SEARCH_FTS_WEIGHT");
                eprintln!("Must be a float between 0.0 and 1.0");
                process::exit(1);
            });

        let hybrid_search_semantic_weight =
            get_optional_env("HYBRID_SEARCH_SEMANTIC_WEIGHT", "1.0")
                .parse::<f32>()
                .unwrap_or_else(|_| {
                    eprintln!("ERROR: Invalid value for HYBRID_SEARCH_SEMANTIC_WEIGHT");
                    eprintln!("Must be a float between 0.0 and 1.0");
                    process::exit(1);
                });

        Self {
            database,
            redis,
            port,
            ai_service_url,
            typo_tolerance_enabled,
            typo_tolerance_max_distance,
            typo_tolerance_min_word_length,
            hybrid_search_fts_weight,
            hybrid_search_semantic_weight,
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
            eprintln!(
                "ERROR: Invalid embedding dimensions in 'EMBEDDING_DIMENSIONS': '{}'",
                embedding_dimensions_str
            );
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
        let database = DatabaseConfig::from_env();

        let webhook_url = env::var("GOOGLE_WEBHOOK_URL").ok();
        if let Some(ref url) = webhook_url {
            if !url.trim().is_empty() {
                validate_url(url, "GOOGLE_WEBHOOK_URL");
                if !url.starts_with("https://") {
                    eprintln!("ERROR: GOOGLE_WEBHOOK_URL must use HTTPS for Google webhooks");
                    eprintln!("Google requires HTTPS endpoints for webhook notifications");
                    process::exit(1);
                }
            }
        }

        let ai_service_url = get_required_env("AI_SERVICE_URL");
        let ai_service_url = validate_url(&ai_service_url, "AI_SERVICE_URL");

        Self {
            base,
            database,
            webhook_url,
            ai_service_url,
        }
    }
}

impl SlackConnectorConfig {
    pub fn from_env() -> Self {
        let base = ConnectorConfig::from_env();
        let database = DatabaseConfig::from_env();

        let bot_token = get_required_env("SLACK_BOT_TOKEN");
        if bot_token.trim().is_empty() || !bot_token.starts_with("xoxb-") {
            eprintln!("ERROR: SLACK_BOT_TOKEN must be set to a valid Slack bot token");
            eprintln!("Bot tokens should start with 'xoxb-'");
            eprintln!("Please install your Slack app and obtain the bot token");
            process::exit(1);
        }

        Self {
            base,
            database,
            bot_token,
        }
    }
}

impl AtlassianConnectorConfig {
    pub fn from_env() -> Self {
        let base = ConnectorConfig::from_env();
        let database = DatabaseConfig::from_env();

        let base_url = get_required_env("ATLASSIAN_BASE_URL");
        let base_url = validate_url(&base_url, "ATLASSIAN_BASE_URL");
        if !base_url.contains("atlassian.net") && !base_url.contains("atlassian.com") {
            eprintln!("ERROR: ATLASSIAN_BASE_URL should be your Atlassian instance URL");
            eprintln!("Example: https://your-company.atlassian.net");
            process::exit(1);
        }

        let user_email = get_required_env("ATLASSIAN_USER_EMAIL");
        if user_email.trim().is_empty() || !user_email.contains('@') {
            eprintln!("ERROR: ATLASSIAN_USER_EMAIL must be set to a valid email address");
            eprintln!("This should be the email of the service account user");
            process::exit(1);
        }

        let api_token = get_required_env("ATLASSIAN_API_TOKEN");
        if api_token.trim().is_empty() {
            eprintln!("ERROR: ATLASSIAN_API_TOKEN must be set to a valid Atlassian API token");
            eprintln!("Create an API token at https://id.atlassian.com/manage-profile/security/api-tokens");
            process::exit(1);
        }

        Self {
            base,
            database,
            base_url,
            user_email,
            api_token,
        }
    }
}

impl FilesystemConnectorConfig {
    pub fn from_env() -> Self {
        let database = DatabaseConfig::from_env();

        Self { database }
    }
}

use std::env;
use std::process;
use url::Url;

#[derive(Debug, Clone)]
pub struct DatabaseConfig {
    pub database_url: String,
    pub max_connections: u32,
    pub acquire_timeout_seconds: u64,
    pub require_ssl: bool,
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
    pub hybrid_search_fts_weight: f32,
    pub hybrid_search_semantic_weight: f32,
    pub semantic_search_timeout_ms: u64,
    pub rag_context_window: i32,
}

#[derive(Debug, Clone)]
pub struct IndexerConfig {
    pub database: DatabaseConfig,
    pub redis: RedisConfig,
    pub port: u16,
    pub ai_service_url: String,
}

#[derive(Debug, Clone)]
pub struct ConnectorConfig {
    pub redis: RedisConfig,
    pub port: u16,
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
        let database_host = get_required_env("DATABASE_HOST");
        let database_username = get_required_env("DATABASE_USERNAME");
        let database_name = get_required_env("DATABASE_NAME");
        let database_password = get_required_env("DATABASE_PASSWORD");
        let database_port = get_optional_env("DATABASE_PORT", "5432");

        let port = parse_port(&database_port, "DATABASE_PORT");

        // Check if SSL should be required
        let require_ssl = get_optional_env("DATABASE_SSL", "false")
            .parse::<bool>()
            .unwrap_or(false);

        // Construct base URL
        let base_url = format!(
            "postgresql://{}:{}@{}:{}/{}",
            database_username, database_password, database_host, port, database_name
        );

        // Parse URL and add SSL parameter if required
        let mut url = Url::parse(&base_url).unwrap_or_else(|e| {
            eprintln!("ERROR: Failed to parse database URL: {}", e);
            eprintln!("URL: {}", base_url);
            process::exit(1);
        });

        if require_ssl {
            url.query_pairs_mut().append_pair("sslmode", "require");
        }

        let database_url = url.to_string();

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
            require_ssl,
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
        let semantic_search_timeout_ms = get_optional_env("SEMANTIC_SEARCH_TIMEOUT_MS", "5000")
            .parse::<u64>()
            .unwrap_or_else(|_| {
                eprintln!("ERROR: Invalid value for SEMANTIC_SEARCH_TIMEOUT_MS");
                eprintln!("Must be a positive integer");
                process::exit(1);
            });

        let rag_context_window = get_optional_env("RAG_CONTEXT_WINDOW", "2")
            .parse::<i32>()
            .unwrap_or_else(|_| {
                eprintln!("ERROR: Invalid value for RAG_CONTEXT_WINDOW");
                eprintln!("Must be a positive integer");
                process::exit(1);
            });

        Self {
            database,
            redis,
            port,
            ai_service_url,
            hybrid_search_fts_weight,
            hybrid_search_semantic_weight,
            semantic_search_timeout_ms,
            rag_context_window,
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

impl ConnectorConfig {
    pub fn from_env() -> Self {
        let redis = RedisConfig::from_env();

        let port_str = get_required_env("PORT");
        let port = parse_port(&port_str, "PORT");

        Self { redis, port }
    }
}

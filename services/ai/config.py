import os
import sys
from urllib.parse import quote_plus


def get_required_env(key: str) -> str:
    """Get required environment variable with validation"""
    value = os.getenv(key)
    if not value:
        print(
            f"ERROR: Required environment variable '{key}' is not set", file=sys.stderr
        )
        print(
            "Please set this variable in your .env file or environment", file=sys.stderr
        )
        sys.exit(1)
    return value


def get_optional_env(key: str, default: str) -> str:
    """Get optional environment variable with default"""
    return os.getenv(key, default)


def validate_port(port_str: str) -> int:
    """Validate port number"""
    try:
        port = int(port_str)
        if port < 1 or port > 65535:
            raise ValueError("Port must be between 1 and 65535")
        return port
    except ValueError as e:
        print(f"ERROR: Invalid port number '{port_str}': {e}", file=sys.stderr)
        sys.exit(1)


def validate_embedding_dimensions(dims_str: str) -> int:
    """Validate embedding dimensions"""
    try:
        dims = int(dims_str)
        if dims < 1:
            raise ValueError("Embedding dimensions must be positive")
        return dims
    except ValueError as e:
        print(f"ERROR: Invalid embedding dimensions '{dims_str}': {e}", file=sys.stderr)
        sys.exit(1)


def construct_database_url() -> str:
    """Construct database URL from individual components"""
    database_host = get_required_env("DATABASE_HOST")
    database_username = get_required_env("DATABASE_USERNAME") 
    database_name = get_required_env("DATABASE_NAME")
    database_password = get_required_env("DATABASE_PASSWORD")
    database_port = get_optional_env("DATABASE_PORT", "5432")
    
    port = validate_port(database_port)
    
    return f"postgresql://{quote_plus(database_username)}:{quote_plus(database_password)}@{database_host}:{port}/{database_name}"


# Load and validate configuration
PORT = validate_port(get_required_env("PORT"))
MODEL_PATH = get_required_env("MODEL_PATH")
REDIS_URL = get_required_env("REDIS_URL")
DATABASE_URL = construct_database_url()

# Embedding server configuration
EMBEDDING_PROVIDER = get_required_env("EMBEDDING_PROVIDER").lower()
JINA_API_KEY = get_optional_env("JINA_API_KEY", "")
JINA_MODEL = get_optional_env("JINA_MODEL", "jina-embeddings-v3")
JINA_API_URL = get_optional_env("JINA_API_URL", "https://api.jina.ai/v1/embeddings")
BEDROCK_EMBEDDING_MODEL_ID = get_optional_env("BEDROCK_EMBEDDING_MODEL_ID", "amazon.titan-embed-text-v2:0")
EMBEDDING_MODEL = get_required_env("EMBEDDING_MODEL")
EMBEDDING_DIMENSIONS = validate_embedding_dimensions(
    get_required_env("EMBEDDING_DIMENSIONS")
)

# LLM configuration
LLM_PROVIDER = get_optional_env("LLM_PROVIDER", "vllm").lower()
VLLM_URL = get_optional_env("VLLM_URL", "http://vllm:8000")  # Make optional
ANTHROPIC_API_KEY = get_optional_env("ANTHROPIC_API_KEY", "")
ANTHROPIC_MODEL = get_optional_env("ANTHROPIC_MODEL", "claude-sonnet-4-20250514")
ANTHROPIC_MAX_TOKENS = int(get_optional_env("ANTHROPIC_MAX_TOKENS", "4096"))
DEFAULT_MAX_TOKENS = int(get_optional_env("DEFAULT_MAX_TOKENS", "8192"))
DEFAULT_TEMPERATURE = float(get_optional_env("DEFAULT_TEMPERATURE", "0.0"))
DEFAULT_TOP_P = float(get_optional_env("DEFAULT_TOP_P", "1.0"))

# AWS Bedrock configuration
BEDROCK_MODEL_ID = get_optional_env("BEDROCK_MODEL_ID", "us.anthropic.claude-sonnet-4-20250514-v1:0")
TITLE_GENERATION_MODEL_ID = get_optional_env("TITLE_GENERATION_MODEL_ID", "us.anthropic.claude-haiku-4-5-20251001-v1:0")
AWS_REGION = get_optional_env("AWS_REGION", "")  # Optional, auto-detected in ECS

# Embedding batch inference configuration
ENABLE_EMBEDDING_BATCH_INFERENCE = get_optional_env("ENABLE_EMBEDDING_BATCH_INFERENCE", "false").lower() == "true"
EMBEDDING_BATCH_S3_BUCKET = get_optional_env("EMBEDDING_BATCH_S3_BUCKET", "")
EMBEDDING_BATCH_BEDROCK_ROLE_ARN = get_optional_env("EMBEDDING_BATCH_BEDROCK_ROLE_ARN", "")

# Embedding batch accumulation thresholds
EMBEDDING_BATCH_MIN_DOCUMENTS = int(get_optional_env("EMBEDDING_BATCH_MIN_DOCUMENTS", "100"))
EMBEDDING_BATCH_MAX_DOCUMENTS = int(get_optional_env("EMBEDDING_BATCH_MAX_DOCUMENTS", "50000"))
EMBEDDING_BATCH_ACCUMULATION_TIMEOUT_SECONDS = int(get_optional_env("EMBEDDING_BATCH_ACCUMULATION_TIMEOUT_SECONDS", "300"))  # 5 minutes

# Embedding batch processing intervals
EMBEDDING_BATCH_ACCUMULATION_POLL_INTERVAL = int(get_optional_env("EMBEDDING_BATCH_ACCUMULATION_POLL_INTERVAL", "10"))  # 10 seconds
EMBEDDING_BATCH_MONITOR_POLL_INTERVAL = int(get_optional_env("EMBEDDING_BATCH_MONITOR_POLL_INTERVAL", "30"))  # 30 seconds


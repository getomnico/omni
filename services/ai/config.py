import os
import sys
from urllib.parse import quote_plus


def get_required_env(key: str) -> str:
    """Get required environment variable with validation. Empty strings are treated as absent."""
    value = os.getenv(key)
    if not value or value.strip() == "":
        print(
            f"ERROR: Required environment variable '{key}' is not set", file=sys.stderr
        )
        print(
            "Please set this variable in your .env file or environment", file=sys.stderr
        )
        sys.exit(1)
    return value


def get_optional_env(key: str, default: str) -> str:
    """Get optional environment variable with default. Empty strings are treated as absent."""
    value = os.getenv(key)
    if value is None or value.strip() == "":
        return default
    return value


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


# Load and validate configuration
PORT = validate_port(get_required_env("PORT"))
MODEL_PATH = get_required_env("MODEL_PATH")
REDIS_URL = get_required_env("REDIS_URL")

# Database connection settings (exported so consumers like the memory
# bootstrap don't have to re-read os.environ).
DATABASE_HOST = get_required_env("DATABASE_HOST")
DATABASE_USERNAME = get_required_env("DATABASE_USERNAME")
DATABASE_NAME = get_required_env("DATABASE_NAME")
DATABASE_PASSWORD = get_required_env("DATABASE_PASSWORD")
DATABASE_PORT = validate_port(get_optional_env("DATABASE_PORT", "5432"))
DATABASE_URL = (
    f"postgresql://{quote_plus(DATABASE_USERNAME)}:{quote_plus(DATABASE_PASSWORD)}"
    f"@{DATABASE_HOST}:{DATABASE_PORT}/{DATABASE_NAME}"
)

# Embedding configuration (provider config is in DB; only window size remains here)
EMBEDDING_MAX_MODEL_LEN = int(get_optional_env("EMBEDDING_MAX_MODEL_LEN", "8192"))

DEFAULT_MAX_TOKENS = int(get_optional_env("DEFAULT_MAX_TOKENS", "8192"))
DEFAULT_TEMPERATURE = float(get_optional_env("DEFAULT_TEMPERATURE", "0.0"))
DEFAULT_TOP_P = float(get_optional_env("DEFAULT_TOP_P", "1.0"))

# AWS configuration (used by online Bedrock embedding/LLM providers)
AWS_REGION = get_optional_env("AWS_REGION", "")  # Optional, auto-detected in ECS

# Conversation compaction
MAX_CONVERSATION_INPUT_TOKENS = int(
    get_optional_env("MAX_CONVERSATION_INPUT_TOKENS", "150000")
)
COMPACTION_RECENT_MESSAGES_COUNT = int(
    get_optional_env("COMPACTION_RECENT_MESSAGES_COUNT", "20")
)
COMPACTION_SUMMARY_MAX_TOKENS = int(
    get_optional_env("COMPACTION_SUMMARY_MAX_TOKENS", "2000")
)
ENABLE_CONVERSATION_COMPACTION = (
    get_optional_env("ENABLE_CONVERSATION_COMPACTION", "true").lower() == "true"
)
COMPACTION_CACHE_TTL_SECONDS = int(
    get_optional_env("COMPACTION_CACHE_TTL_SECONDS", "86400")
)  # 24 hours

# Agent configuration
AGENT_MAX_ITERATIONS = int(get_optional_env("AGENT_MAX_ITERATIONS", "15"))
CONNECTOR_MANAGER_URL = get_required_env("CONNECTOR_MANAGER_URL")
SANDBOX_URL: str | None = os.getenv("SANDBOX_URL") or None
MEMORY_ENABLED = get_optional_env("MEMORY_ENABLED", "false").lower() == "true"
MEMORY_PROVIDER: str = get_optional_env("MEMORY_PROVIDER", "mem0")
MEM0_HISTORY_DB_PATH: str = get_optional_env(
    "MEM0_HISTORY_DB_PATH", "/tmp/mem0_history.db"
)

# Background agent scheduler
AGENT_SCHEDULER_POLL_INTERVAL = int(
    get_optional_env("AGENT_SCHEDULER_POLL_INTERVAL", "30")
)  # seconds
AGENT_MAX_CONCURRENT_RUNS = int(get_optional_env("AGENT_MAX_CONCURRENT_RUNS", "3"))
AGENT_RUN_LEASE_SECONDS = int(get_optional_env("AGENT_RUN_LEASE_SECONDS", "300"))
AGENT_RUN_HEARTBEAT_INTERVAL_SECONDS = int(
    get_optional_env("AGENT_RUN_HEARTBEAT_INTERVAL_SECONDS", "30")
)
AGENT_RUN_CLAIM_POLL_INTERVAL_SECONDS = int(
    get_optional_env("AGENT_RUN_CLAIM_POLL_INTERVAL_SECONDS", "5")
)
AGENT_RUN_STALE_RECOVERY_INTERVAL_SECONDS = int(
    get_optional_env("AGENT_RUN_STALE_RECOVERY_INTERVAL_SECONDS", "30")
)
AGENT_RUN_MAX_ATTEMPTS = int(get_optional_env("AGENT_RUN_MAX_ATTEMPTS", "3"))
AGENT_RUN_BACKOFF_SECONDS = tuple(
    int(part.strip())
    for part in get_optional_env("AGENT_RUN_BACKOFF_SECONDS", "30,120,300").split(",")
    if part.strip()
)

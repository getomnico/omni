"""Configuration constants for Notion connector."""

ITEMS_PER_PAGE = 100
MAX_CONTENT_LENGTH = 100_000
CHECKPOINT_INTERVAL = 50
RATE_LIMIT_DELAY = 0.35

# Pin the Notion-Version header so behavior is stable when notion-client bumps
# its default. 2025-09-03 introduced data sources and is what this connector
# is written against.
NOTION_API_VERSION = "2025-09-03"

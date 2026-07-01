"""Configuration constants for ClickUp connector."""

CLICKUP_MCP_URL = "https://mcp.clickup.com/mcp"
CLICKUP_OAUTH_AUTH_ENDPOINT = "https://mcp.clickup.com/oauth/authorize"
CLICKUP_OAUTH_TOKEN_ENDPOINT = "https://mcp.clickup.com/oauth/token"
CLICKUP_OAUTH_REGISTRATION_ENDPOINT = "https://mcp.clickup.com/oauth/register"
CLICKUP_OAUTH_USERINFO_ENDPOINT = "https://api.clickup.com/api/v2/user"
CLICKUP_OAUTH_SCOPES = ["read", "write"]
CLICKUP_OAUTH_RESOURCE = CLICKUP_MCP_URL

TASKS_PER_PAGE = 100
MAX_COMMENT_COUNT = 50
MAX_CONTENT_LENGTH = 100_000
CHECKPOINT_INTERVAL = 50
MAX_RETRIES = 3
INITIAL_BACKOFF_SECONDS = 1.0

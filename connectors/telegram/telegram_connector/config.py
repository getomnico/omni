"""Telegram connector configuration constants."""

# Telegram Bot API limits
MESSAGES_PER_REQUEST = 100
MAX_CONTENT_LENGTH = 100_000
CHECKPOINT_INTERVAL = 50
RATE_LIMIT_DELAY = 0.05  # Telegram allows ~30 requests/second

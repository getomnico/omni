import logging
import sys
import os
from typing import Optional


def setup_logging(
    level: Optional[str] = None, format_string: Optional[str] = None
) -> None:
    """
    Configure logging for the AI service. Call this once at application startup.

    Args:
        level: Logging level (DEBUG, INFO, WARNING, ERROR, CRITICAL)
               If None, reads from LOG_LEVEL env var or defaults to INFO
        format_string: Optional custom format string
    """
    # Get log level from env var if not provided
    if level is None:
        level = os.getenv("LOG_LEVEL", "INFO")

    # Default format includes timestamp, level, module name, function name, and message
    if format_string is None:
        format_string = (
            "[%(asctime)s] [%(levelname)s] [%(name)s:%(funcName)s] %(message)s"
        )

    # Configure root logger - this only needs to be done once at startup
    logging.basicConfig(
        level=getattr(logging, level.upper()),
        format=format_string,
        handlers=[logging.StreamHandler(sys.stdout)],  # Log to stdout instead of stderr
        force=True,  # Override any existing configuration
    )

    # Suppress some noisy loggers
    logging.getLogger("httpx").setLevel(logging.WARNING)
    logging.getLogger("httpcore").setLevel(logging.WARNING)
    logging.getLogger("uvicorn.access").setLevel(logging.WARNING)

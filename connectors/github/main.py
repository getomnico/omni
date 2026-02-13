#!/usr/bin/env python3
"""GitHub Connector entry point for Omni."""

import logging
import os

from github_connector import GitHubConnector

logging.basicConfig(
    level=logging.INFO,
    format="%(asctime)s - %(name)s - %(levelname)s - %(message)s",
)

if __name__ == "__main__":
    port = int(os.environ.get("PORT", "8000"))
    GitHubConnector().serve(port=port)

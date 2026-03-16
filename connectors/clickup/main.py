#!/usr/bin/env python3
"""ClickUp Connector entry point for Omni."""

import logging
import os

from clickup_connector import ClickUpConnector

logging.basicConfig(
    level=logging.INFO,
    format="%(asctime)s - %(name)s - %(levelname)s - %(message)s",
)

if __name__ == "__main__":
    port = int(os.environ.get("PORT", "8000"))
    ClickUpConnector().serve(port=port)

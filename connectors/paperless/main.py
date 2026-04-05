#!/usr/bin/env python3
"""Paperless-ngx Connector entry point for Omni."""

import logging
import os

from paperless_connector import PaperlessConnector

logging.basicConfig(
    level=logging.INFO,
    format="%(asctime)s - %(name)s - %(levelname)s - %(message)s",
)

if __name__ == "__main__":
    port = int(os.environ.get("PORT", "8000"))
    PaperlessConnector().serve(port=port)

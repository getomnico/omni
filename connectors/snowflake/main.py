#!/usr/bin/env python3
"""Snowflake connector entry point."""

import logging
import os

from snowflake_connector import SnowflakeConnector

logging.basicConfig(level=logging.INFO)

if __name__ == "__main__":
    SnowflakeConnector().serve(port=int(os.environ.get("PORT", "8000")))

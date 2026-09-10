#!/usr/bin/env python3
"""Salesforce Connector entry point for Omni."""

import logging
import os

from salesforce_connector import SalesforceConnector

logging.basicConfig(
    level=logging.INFO,
    format="%(asctime)s - %(name)s - %(levelname)s - %(message)s",
)

if __name__ == "__main__":
    port_value = os.environ.get("PORT")
    if not port_value:
        raise SystemExit("PORT environment variable is required")
    try:
        port = int(port_value)
    except ValueError as exc:
        raise SystemExit("PORT environment variable must be an integer") from exc
    SalesforceConnector().serve(port=port)

#!/usr/bin/env python3
"""Microsoft 365 Connector entry point for Omni."""

import logging
import os

from ms_connector import MicrosoftConnector

logging.basicConfig(
    level=logging.INFO,
    format="%(asctime)s - %(name)s - %(levelname)s - %(message)s",
)

if __name__ == "__main__":
    port = int(os.environ.get("PORT", "8000"))
    MicrosoftConnector().serve(port=port)

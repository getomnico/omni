#!/usr/bin/env python3
"""Google Ads Connector entry point for Omni."""

import logging
import os

from google_ads_connector import GoogleAdsConnector

logging.basicConfig(
    level=logging.INFO,
    format="%(asctime)s - %(name)s - %(levelname)s - %(message)s",
)

if __name__ == "__main__":
    port = os.environ.get("PORT")
    if not port:
        raise SystemExit("PORT environment variable is required")
    GoogleAdsConnector().serve(port=int(port))

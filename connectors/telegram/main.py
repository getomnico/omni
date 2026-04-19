#!/usr/bin/env python3
"""Telegram Connector entry point for Omni."""

import logging
import os

from telegram_connector import TelegramConnector

logging.basicConfig(
    level=logging.INFO,
    format="%(asctime)s - %(name)s - %(levelname)s - %(message)s",
)

if __name__ == "__main__":
    port = int(os.environ.get("PORT", "8000"))
    TelegramConnector().serve(port=port)

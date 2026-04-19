#!/usr/bin/env python3
"""One-time interactive auth script to generate a Telethon StringSession.

Run this locally (not in Docker) to authenticate with your phone number.
The output session string is stored as a credential in Omni.

Usage:
    python scripts/auth.py --api-id YOUR_API_ID --api-hash YOUR_API_HASH

Get api_id and api_hash from https://my.telegram.org/apps
"""

import argparse
import asyncio

from telethon import TelegramClient
from telethon.sessions import StringSession


async def main(api_id: int, api_hash: str) -> None:
    client = TelegramClient(StringSession(), api_id, api_hash)
    await client.start()  # type: ignore[arg-type]

    me = await client.get_me()
    print(f"\nAuthenticated as: {me.first_name} {me.last_name or ''} (@{me.username})")
    print(f"Phone: {me.phone}")
    print()

    session_string = client.session.save()

    print("=" * 60)
    print("SESSION STRING (copy this into Omni credentials):")
    print("=" * 60)
    print(session_string)
    print("=" * 60)
    print()
    print("Store this as the 'session' field in your Telegram source credentials.")
    print("Also store 'api_id' and 'api_hash' in the same credentials.")

    await client.disconnect()


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description="Generate Telethon session string")
    parser.add_argument("--api-id", type=int, required=True, help="Telegram API ID")
    parser.add_argument("--api-hash", type=str, required=True, help="Telegram API hash")
    args = parser.parse_args()

    asyncio.run(main(args.api_id, args.api_hash))

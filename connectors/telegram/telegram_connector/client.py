"""Telegram Bot API client wrapper."""

import asyncio
import logging
from typing import Any

import httpx

from .config import RATE_LIMIT_DELAY

logger = logging.getLogger(__name__)

BASE_URL = "https://api.telegram.org"


class TelegramError(Exception):
    """Base exception for Telegram API errors."""

    pass


class AuthenticationError(TelegramError):
    """Invalid bot token (401)."""

    pass


class RateLimitError(TelegramError):
    """Rate limited (429)."""

    def __init__(self, message: str, retry_after: float = 1.0):
        super().__init__(message)
        self.retry_after = retry_after


class TelegramClient:
    """Async wrapper around the Telegram Bot API."""

    def __init__(self, token: str, rate_limit_delay: float = RATE_LIMIT_DELAY):
        self._token = token
        self._rate_limit_delay = rate_limit_delay
        self._client = httpx.AsyncClient(timeout=30.0)

    async def get_me(self) -> dict[str, Any]:
        """Validate token by fetching the bot user."""
        return await self._api_call("getMe")

    async def get_updates(
        self,
        offset: int | None = None,
        limit: int = 100,
        allowed_updates: list[str] | None = None,
    ) -> list[dict[str, Any]]:
        """Get recent updates (messages) via long polling."""
        params: dict[str, Any] = {"limit": limit}
        if offset is not None:
            params["offset"] = offset
        if allowed_updates:
            params["allowed_updates"] = allowed_updates
        result = await self._api_call("getUpdates", params)
        return result if isinstance(result, list) else []

    async def get_chat(self, chat_id: int | str) -> dict[str, Any]:
        """Get chat info."""
        return await self._api_call("getChat", {"chat_id": chat_id})

    async def get_chat_member_count(self, chat_id: int | str) -> int:
        """Get number of members in a chat."""
        return await self._api_call("getChatMemberCount", {"chat_id": chat_id})

    async def get_chat_administrators(
        self, chat_id: int | str
    ) -> list[dict[str, Any]]:
        """Get list of administrators in a chat."""
        return await self._api_call("getChatAdministrators", {"chat_id": chat_id})

    async def get_file(self, file_id: str) -> dict[str, Any]:
        """Get file info for downloading."""
        return await self._api_call("getFile", {"file_id": file_id})

    def _bot_url(self, path: str) -> str:
        """Build Bot API URL without storing token in an attribute."""
        return f"{BASE_URL}/bot{self._token}/{path}"

    async def download_file(self, file_path: str) -> bytes:
        """Download a file by path."""
        url = f"{BASE_URL}/file/bot{self._token}/{file_path}"
        response = await self._client.get(url)
        response.raise_for_status()
        return response.content

    async def _api_call(
        self, method: str, params: dict[str, Any] | None = None
    ) -> Any:
        """Execute an API call with retry logic for rate limits."""
        max_retries = 3
        for attempt in range(max_retries + 1):
            try:
                await asyncio.sleep(self._rate_limit_delay)
                response = await self._client.post(
                    self._bot_url(method),
                    json=params or {},
                )
                data = response.json()

                if not data.get("ok"):
                    error_code = data.get("error_code", 0)
                    description = data.get("description", "Unknown error")

                    if error_code == 401:
                        raise AuthenticationError(
                            "Invalid or expired bot token"
                        )

                    if error_code == 429:
                        retry_after = (
                            data.get("parameters", {}).get("retry_after", 1.0)
                        )
                        if attempt < max_retries:
                            logger.warning(
                                "Rate limited, retrying in %.1fs (attempt %d/%d)",
                                retry_after,
                                attempt + 1,
                                max_retries,
                            )
                            await asyncio.sleep(retry_after)
                            continue
                        raise RateLimitError(
                            f"Rate limited after {max_retries} retries",
                            retry_after,
                        )

                    raise TelegramError(
                        f"Telegram API error ({error_code}): {description}"
                    )

                return data.get("result")

            except (httpx.HTTPError, httpx.TimeoutException) as e:
                if attempt < max_retries:
                    logger.warning(
                        "HTTP error, retrying (attempt %d/%d): %s",
                        attempt + 1,
                        max_retries,
                        e,
                    )
                    await asyncio.sleep(1.0)
                    continue
                raise TelegramError(f"Telegram API call failed: {e}") from e

        raise TelegramError("Unexpected retry loop exit")

    async def close(self) -> None:
        """Close the underlying HTTP client."""
        await self._client.aclose()

"""Thin async wrapper over Microsoft Graph REST API with retry logic."""

import asyncio
import logging
from collections.abc import AsyncIterator
from functools import wraps
from typing import Any

import httpx

from .auth import MSGraphAuth

logger = logging.getLogger(__name__)

GRAPH_BASE_URL = "https://graph.microsoft.com/v1.0"


class GraphAPIError(Exception):
    """Base exception for Graph API errors."""

    def __init__(self, message: str, status_code: int | None = None):
        super().__init__(message)
        self.status_code = status_code


class AuthenticationError(GraphAPIError):
    """Invalid or expired credentials (401)."""

    pass


def with_retry(max_retries: int = 3, base_delay: float = 1.0):
    """Decorator for retrying Graph API calls with exponential backoff.

    Handles:
    - 429 Rate Limit: Wait for Retry-After header (unlimited retries)
    - 5xx Server Error: Exponential backoff (limited retries)
    - 401 Unauthorized: Re-authenticate once, then fail
    """

    def decorator(func):
        @wraps(func)
        async def wrapper(self, *args, **kwargs):
            last_exception = None
            error_retries = 0
            auth_retried = False

            while True:
                try:
                    return await func(self, *args, **kwargs)
                except httpx.HTTPStatusError as e:
                    last_exception = e
                    status = e.response.status_code

                    if status == 401:
                        if auth_retried:
                            raise AuthenticationError(
                                "Authentication failed after token refresh",
                                status_code=401,
                            ) from e
                        auth_retried = True
                        logger.warning("Got 401, refreshing token and retrying")
                        self._refresh_token()
                        continue

                    if status == 404:
                        raise GraphAPIError(
                            f"Resource not found: {e.response.text}",
                            status_code=404,
                        ) from e

                    if status == 429:
                        retry_after = int(e.response.headers.get("Retry-After", "10"))
                        logger.warning("Rate limited. Waiting %ds", retry_after)
                        await asyncio.sleep(retry_after)
                        continue

                    if status >= 500:
                        error_retries += 1
                        if error_retries > max_retries:
                            break
                        delay = base_delay * (2 ** (error_retries - 1))
                        logger.warning(
                            "Server error %d. Retrying in %.1fs (%d/%d)",
                            status,
                            delay,
                            error_retries,
                            max_retries,
                        )
                        await asyncio.sleep(delay)
                        continue

                    raise GraphAPIError(
                        f"API error {status}: {e.response.text}",
                        status_code=status,
                    ) from e

            raise GraphAPIError(
                f"Max retries exceeded: {last_exception}"
            ) from last_exception

        return wrapper

    return decorator


class GraphClient:
    """Async client for Microsoft Graph API v1.0."""

    def __init__(self, auth: MSGraphAuth, http_client: httpx.AsyncClient | None = None):
        self._auth = auth
        self._client = http_client or httpx.AsyncClient(
            base_url=GRAPH_BASE_URL,
            timeout=httpx.Timeout(30.0, connect=10.0),
        )
        self._refresh_token()

    def _refresh_token(self) -> None:
        token = self._auth.get_token()
        self._client.headers["Authorization"] = f"Bearer {token}"

    async def close(self) -> None:
        await self._client.aclose()

    @with_retry(max_retries=3)
    async def get(
        self, url: str, params: dict[str, Any] | None = None
    ) -> dict[str, Any]:
        """Execute a GET request against the Graph API."""
        response = await self._client.get(url, params=params)
        response.raise_for_status()
        return response.json()

    @with_retry(max_retries=3)
    async def get_binary(self, url: str) -> bytes:
        """Execute a GET request that returns binary content (file downloads)."""
        response = await self._client.get(url, follow_redirects=True)
        response.raise_for_status()
        return response.content

    async def get_paginated(
        self,
        url: str,
        params: dict[str, Any] | None = None,
    ) -> AsyncIterator[dict[str, Any]]:
        """Follow @odata.nextLink pagination, yielding individual items."""
        next_url: str | None = url
        next_params = params

        while next_url:
            data = await self.get(next_url, params=next_params)
            for item in data.get("value", []):
                yield item

            next_link = data.get("@odata.nextLink")
            if next_link:
                # nextLink is an absolute URL with params baked in
                next_url = next_link
                next_params = None
            else:
                next_url = None

    async def get_delta(
        self,
        url: str,
        delta_token: str | None = None,
        params: dict[str, Any] | None = None,
    ) -> tuple[list[dict[str, Any]], str | None]:
        """Execute a delta query, following all pages.

        Returns (items, new_delta_token). On first call, pass delta_token=None
        for a full snapshot. On subsequent calls, pass the previously returned
        delta_token for incremental changes.
        """
        if delta_token:
            # Delta link is an absolute URL — use it directly
            next_url: str | None = delta_token
            next_params = None
        else:
            next_url = url
            next_params = params

        items: list[dict[str, Any]] = []
        new_delta_token: str | None = None

        while next_url:
            data = await self.get(next_url, params=next_params)
            items.extend(data.get("value", []))

            next_link = data.get("@odata.nextLink")
            if next_link:
                next_url = next_link
                next_params = None
            else:
                new_delta_token = data.get("@odata.deltaLink")
                next_url = None

        return items, new_delta_token

    async def list_users(self) -> list[dict[str, Any]]:
        """Enumerate all users in the tenant."""
        users: list[dict[str, Any]] = []
        async for user in self.get_paginated(
            "/users",
            params={"$select": "id,displayName,mail,userPrincipalName"},
        ):
            users.append(user)
        return users

    async def test_connection(self) -> None:
        """Validate credentials by calling /organization."""
        await self.get("/organization", params={"$select": "id,displayName"})

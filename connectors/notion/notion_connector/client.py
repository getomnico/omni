"""Notion API client wrapper."""

import asyncio
import logging
from typing import Any

from notion_client import AsyncClient
from notion_client.errors import APIResponseError

from .config import ITEMS_PER_PAGE, NOTION_API_VERSION, RATE_LIMIT_DELAY

logger = logging.getLogger(__name__)


class NotionError(Exception):
    """Base exception for Notion API errors."""

    pass


class AuthenticationError(NotionError):
    """Invalid or expired token (401)."""

    pass


class ForbiddenError(NotionError):
    """Integration is missing the capability required to call this endpoint (403)."""

    pass


class RateLimitError(NotionError):
    """Rate limited (429)."""

    def __init__(self, message: str, retry_after: float = 1.0):
        super().__init__(message)
        self.retry_after = retry_after


class NotionClient:
    """Async wrapper around the official Notion Python SDK."""

    def __init__(
        self,
        token: str,
        base_url: str | None = None,
        rate_limit_delay: float = RATE_LIMIT_DELAY,
    ):
        kwargs: dict[str, Any] = {
            "auth": token,
            "notion_version": NOTION_API_VERSION,
        }
        if base_url:
            kwargs["base_url"] = base_url
        self._client = AsyncClient(**kwargs)
        self._rate_limit_delay = rate_limit_delay

    async def validate_token(self) -> dict[str, Any]:
        """Validate token by fetching the bot user. Returns bot user info."""
        try:
            response = await self._api_call(self._client.users.me)
            return response
        except AuthenticationError:
            raise
        except NotionError as e:
            raise NotionError(f"Token validation failed: {e}") from e

    async def search_pages(
        self,
        start_cursor: str | None = None,
        page_size: int = ITEMS_PER_PAGE,
    ) -> dict[str, Any]:
        """Search for all pages in the workspace, newest-edited first."""
        kwargs: dict[str, Any] = {
            "filter": {"value": "page", "property": "object"},
            "sort": {"timestamp": "last_edited_time", "direction": "descending"},
            "page_size": page_size,
        }
        if start_cursor:
            kwargs["start_cursor"] = start_cursor
        return await self._api_call(self._client.search, **kwargs)

    async def search_data_sources(
        self,
        start_cursor: str | None = None,
        page_size: int = ITEMS_PER_PAGE,
    ) -> dict[str, Any]:
        """Search for all data sources in the workspace, newest-edited first.

        Under Notion-Version 2025-09-03+, the search endpoint returns data
        sources (not databases). A database can contain multiple data sources;
        each data source is what holds rows and a property schema.
        """
        kwargs: dict[str, Any] = {
            "filter": {"value": "data_source", "property": "object"},
            "sort": {"timestamp": "last_edited_time", "direction": "descending"},
            "page_size": page_size,
        }
        if start_cursor:
            kwargs["start_cursor"] = start_cursor
        return await self._api_call(self._client.search, **kwargs)

    async def query_data_source(
        self,
        data_source_id: str,
        start_cursor: str | None = None,
        filter: dict[str, Any] | None = None,
        page_size: int = ITEMS_PER_PAGE,
    ) -> dict[str, Any]:
        """Query pages within a data source.

        Under Notion-Version 2025-09-03+, the legacy databases/{id}/query
        endpoint is deprecated in favor of data_sources/{id}/query.
        """
        body: dict[str, Any] = {"page_size": page_size}
        if start_cursor:
            body["start_cursor"] = start_cursor
        if filter:
            body["filter"] = filter

        async def _do_query() -> Any:
            return await self._client.request(
                path=f"data_sources/{data_source_id}/query",
                method="POST",
                body=body,
            )

        return await self._api_call(_do_query)

    async def retrieve_data_source(self, data_source_id: str) -> dict[str, Any]:
        """Retrieve one data source by ID."""

        async def _do_retrieve() -> Any:
            return await self._client.request(
                path=f"data_sources/{data_source_id}",
                method="GET",
            )

        return await self._api_call(_do_retrieve)

    async def get_block_children(
        self,
        block_id: str,
        start_cursor: str | None = None,
    ) -> dict[str, Any]:
        """Get child blocks of a block (one level)."""
        kwargs: dict[str, Any] = {"block_id": block_id, "page_size": ITEMS_PER_PAGE}
        if start_cursor:
            kwargs["start_cursor"] = start_cursor
        return await self._api_call(self._client.blocks.children.list, **kwargs)

    async def get_all_blocks(self, block_id: str) -> list[dict[str, Any]]:
        """Recursively fetch all blocks for a page, including nested children."""
        blocks: list[dict[str, Any]] = []
        cursor: str | None = None

        while True:
            response = await self.get_block_children(block_id, start_cursor=cursor)
            results = response.get("results", [])

            for block in results:
                blocks.append(block)
                if block.get("has_children"):
                    children = await self.get_all_blocks(block["id"])
                    block["_children"] = children

            if not response.get("has_more"):
                break
            cursor = response.get("next_cursor")

        return blocks

    async def list_users(self) -> list[dict[str, Any]]:
        """List all users in the workspace."""
        users: list[dict[str, Any]] = []
        cursor: str | None = None

        while True:
            kwargs: dict[str, Any] = {"page_size": ITEMS_PER_PAGE}
            if cursor:
                kwargs["start_cursor"] = cursor
            response = await self._api_call(self._client.users.list, **kwargs)
            users.extend(response.get("results", []))
            if not response.get("has_more"):
                break
            cursor = response.get("next_cursor")

        return users

    async def _api_call(self, method: Any, **kwargs: Any) -> dict[str, Any]:
        """Execute an API call with retry logic for rate limits."""
        max_retries = 3
        for attempt in range(max_retries + 1):
            try:
                await asyncio.sleep(self._rate_limit_delay)
                result: dict[str, Any] = await method(**kwargs)
                return result
            except APIResponseError as e:
                if e.status == 401:
                    raise AuthenticationError("Invalid or expired token") from e
                if e.status == 403:
                    raise ForbiddenError(
                        f"Integration lacks required capability ({e})"
                    ) from e
                if e.status == 429:
                    retry_after = (
                        float(e.headers.get("Retry-After", "1.0"))
                        if hasattr(e, "headers") and e.headers
                        else 1.0
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
                        f"Rate limited after {max_retries} retries", retry_after
                    ) from e
                raise NotionError(f"Notion API error ({e.status}): {e}") from e
            except asyncio.CancelledError:
                raise
            except Exception as e:
                raise NotionError(f"Notion API call failed: {e}") from e
        raise NotionError("Unexpected retry loop exit")

    async def close(self) -> None:
        """Close the underlying HTTP client."""
        await self._client.aclose()

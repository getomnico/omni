"""Async HTTP client for the ClickUp API."""

import asyncio
import logging
import time
from collections.abc import AsyncIterator
from typing import Any

import httpx

from .config import (
    INITIAL_BACKOFF_SECONDS,
    MAX_COMMENT_COUNT,
    MAX_RETRIES,
    TASKS_PER_PAGE,
)

logger = logging.getLogger(__name__)

BASE_URL = "https://api.clickup.com"


class ClickUpError(Exception):
    """Base exception for ClickUp API errors."""


class AuthenticationError(ClickUpError):
    """Invalid or expired token (401)."""


class RateLimitError(ClickUpError):
    """Rate limit exceeded (429)."""


class ClickUpClient:
    """Thin async wrapper around the ClickUp REST API."""

    def __init__(self, token: str, base_url: str | None = None):
        self._client = httpx.AsyncClient(
            base_url=base_url or BASE_URL,
            headers={"Authorization": token},
            timeout=30.0,
        )

    async def _request(self, method: str, path: str, **kwargs: Any) -> Any:
        """Make an HTTP request with rate-limit retry and error handling."""
        backoff = INITIAL_BACKOFF_SECONDS
        for attempt in range(MAX_RETRIES + 1):
            resp = await self._client.request(method, path, **kwargs)

            if resp.status_code == 429:
                reset = resp.headers.get("X-RateLimit-Reset")
                if reset:
                    wait = max(float(reset) - time.time(), 1.0)
                else:
                    wait = backoff
                logger.warning(
                    "Rate limited, waiting %.1fs (attempt %d)", wait, attempt + 1
                )
                await asyncio.sleep(wait)
                backoff *= 2
                continue

            if resp.status_code == 401:
                raise AuthenticationError("Invalid or expired ClickUp token")

            if resp.status_code >= 500:
                if attempt < MAX_RETRIES:
                    logger.warning(
                        "Server error %d, retrying in %.1fs", resp.status_code, backoff
                    )
                    await asyncio.sleep(backoff)
                    backoff *= 2
                    continue
                raise ClickUpError(f"Server error {resp.status_code}: {resp.text}")

            resp.raise_for_status()
            return resp.json()

        raise ClickUpError("Max retries exceeded")

    # ── Workspaces / Teams ──────────────────────────────────────────

    async def get_workspaces(self) -> list[dict[str, Any]]:
        """List authorized workspaces. Also validates the token."""
        data = await self._request("GET", "/api/v2/team")
        return data.get("teams", [])

    # ── Hierarchy (for name lookups) ────────────────────────────────

    async def list_spaces(self, team_id: str) -> list[dict[str, Any]]:
        data = await self._request("GET", f"/api/v2/team/{team_id}/space")
        return data.get("spaces", [])

    async def list_folders(self, space_id: str) -> list[dict[str, Any]]:
        data = await self._request("GET", f"/api/v2/space/{space_id}/folder")
        return data.get("folders", [])

    async def list_lists_in_folder(self, folder_id: str) -> list[dict[str, Any]]:
        data = await self._request("GET", f"/api/v2/folder/{folder_id}/list")
        return data.get("lists", [])

    async def list_folderless_lists(self, space_id: str) -> list[dict[str, Any]]:
        data = await self._request("GET", f"/api/v2/space/{space_id}/list")
        return data.get("lists", [])

    # ── Tasks ───────────────────────────────────────────────────────

    async def list_tasks(
        self,
        team_id: str,
        *,
        include_closed: bool = True,
        subtasks: bool = True,
        date_updated_gt: int | None = None,
    ) -> AsyncIterator[dict[str, Any]]:
        """Paginate through tasks in a workspace via the filtered team endpoint."""
        page = 0
        while True:
            params: dict[str, Any] = {
                "page": page,
                "subtasks": str(subtasks).lower(),
                "include_closed": str(include_closed).lower(),
            }
            if date_updated_gt is not None:
                params["date_updated_gt"] = str(date_updated_gt)

            data = await self._request(
                "GET", f"/api/v2/team/{team_id}/task", params=params
            )
            tasks = data.get("tasks", [])
            for task in tasks:
                yield task

            if len(tasks) < TASKS_PER_PAGE:
                break
            page += 1

    async def get_task(self, task_id: str) -> dict[str, Any]:
        return await self._request("GET", f"/api/v2/task/{task_id}")

    # ── Comments ────────────────────────────────────────────────────

    async def get_task_comments(self, task_id: str) -> list[dict[str, Any]]:
        """Fetch comments for a task, capped at MAX_COMMENT_COUNT."""
        comments: list[dict[str, Any]] = []
        start: int | None = None
        start_id: str | None = None

        while len(comments) < MAX_COMMENT_COUNT:
            params: dict[str, Any] = {}
            if start is not None and start_id is not None:
                params["start"] = str(start)
                params["start_id"] = start_id

            data = await self._request(
                "GET", f"/api/v2/task/{task_id}/comment", params=params
            )
            batch = data.get("comments", [])
            if not batch:
                break

            comments.extend(batch)

            # ClickUp comment pagination: use last comment's date + id
            last = batch[-1]
            start = int(last.get("date", 0))
            start_id = last.get("id")

            if len(batch) < 25:
                break

        return comments[:MAX_COMMENT_COUNT]

    # ── Docs (v3 API) ──────────────────────────────────────────────

    async def list_docs(self, workspace_id: str) -> AsyncIterator[dict[str, Any]]:
        """List docs in a workspace via the v3 API."""
        # The v3 docs search endpoint may support pagination; fetch until empty
        data = await self._request("GET", f"/api/v3/workspaces/{workspace_id}/docs")
        for doc in data.get("docs", []):
            yield doc

    async def get_doc_pages(
        self, workspace_id: str, doc_id: str
    ) -> list[dict[str, Any]]:
        data = await self._request(
            "GET", f"/api/v3/workspaces/{workspace_id}/docs/{doc_id}/pages"
        )
        return data.get("pages", [])

    # ── Lifecycle ───────────────────────────────────────────────────

    async def close(self) -> None:
        await self._client.aclose()

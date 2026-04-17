"""Thin async httpx wrapper around the mem0 REST API.

All methods are best-effort: failures are logged as warnings and never
propagate to the caller — memory is non-critical infrastructure.
Exception: `delete` and `delete_all` return a boolean so UI code can
reflect whether the operation actually succeeded.
"""

import logging
from typing import Any

import httpx

logger = logging.getLogger(__name__)


class MemoryClient:
    """Async client for the mem0 memory service."""

    def __init__(self, base_url: str, timeout: float = 30.0):
        self._base_url = base_url.rstrip("/")
        self._client = httpx.AsyncClient(timeout=timeout)

    async def search(self, query: str, user_id: str, limit: int = 5) -> list[str]:
        """Search for relevant memories for user_id given a query.

        Returns a list of memory strings, empty list on any failure.
        """
        try:
            resp = await self._client.post(
                f"{self._base_url}/search",
                json={"query": query, "user_id": user_id, "top_k": limit},
            )
            resp.raise_for_status()
            return [item["memory"] for item in resp.json().get("results", [])]
        except Exception as e:
            logger.warning(f"Memory search failed for user {user_id}: {e}")
            return []

    async def add(self, messages: list[dict[str, Any]], user_id: str) -> None:
        """Add a conversation turn to memory for user_id.

        Fire-and-forget: logs warnings on failure, never raises.
        """
        try:
            resp = await self._client.post(
                f"{self._base_url}/memories",
                json={"messages": messages, "user_id": user_id},
            )
            resp.raise_for_status()
        except Exception as e:
            logger.warning(f"Memory add failed for user {user_id}: {e}")

    async def list(self, user_id: str) -> list[dict[str, Any]]:
        """Return all memories for user_id.

        Each entry is a dict with at least `id` and `memory` keys (mem0's
        response shape). Returns an empty list on any failure.
        """
        try:
            resp = await self._client.get(
                f"{self._base_url}/memories", params={"user_id": user_id}
            )
            resp.raise_for_status()
            data = resp.json()
            # mem0 may return {"results": [...]} or a bare list depending on version
            if isinstance(data, dict):
                return list(data.get("results", []))
            if isinstance(data, list):
                return data
            return []
        except Exception as e:
            logger.warning(f"Memory list failed for user {user_id}: {e}")
            return []

    async def delete(self, memory_id: str) -> bool:
        """Delete a single memory by id. Returns True on success."""
        try:
            resp = await self._client.delete(f"{self._base_url}/memories/{memory_id}")
            resp.raise_for_status()
            return True
        except Exception as e:
            logger.warning(f"Memory delete failed for {memory_id}: {e}")
            return False

    async def delete_all(self, user_id: str) -> bool:
        """Delete every memory for user_id. Returns True on success."""
        try:
            resp = await self._client.delete(
                f"{self._base_url}/memories", params={"user_id": user_id}
            )
            resp.raise_for_status()
            return True
        except Exception as e:
            logger.warning(f"Memory delete_all failed for user {user_id}: {e}")
            return False

    async def aclose(self) -> None:
        """Close the underlying httpx connection pool. Idempotent."""
        if not self._client.is_closed:
            await self._client.aclose()

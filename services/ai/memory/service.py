"""In-process wrapper around mem0.Memory.

Mirrors the public surface of the old HTTP-based MemoryClient so every
caller in the AI service can be swapped with an import-only change.
Every mem0 call is dispatched on the threadpool because mem0 is
synchronous under the hood. All errors are logged and swallowed —
memory is non-critical infrastructure.
"""
import logging
import sqlite3
from typing import Any

import psycopg
from anthropic.types import MessageParam
from fastapi.concurrency import run_in_threadpool
from mem0 import Memory

logger = logging.getLogger(__name__)


def _sanitize_messages(messages: list[MessageParam]) -> list[dict[str, str]]:
    """Flatten list content to text-only strings; drop empty messages.

    mem0's parse_vision_messages calls get_image_description(msg, llm=None)
    for any list-typed content, not just actual images, and crashes when
    no vision LLM is configured.
    """
    out: list[dict[str, str]] = []
    for m in messages:
        content: Any = m.get("content", "")
        if isinstance(content, list):
            content = " ".join(
                b.get("text", "") for b in content
                if isinstance(b, dict) and b.get("type") == "text"
            )
        if content and isinstance(content, str):
            out.append({"role": m.get("role", "user"), "content": content})
    return out


class MemoryService:
    """Direct wrapper around mem0.Memory with the MemoryClient surface."""

    def __init__(self, memory: Memory, db_config: dict[str, Any]):
        self._mem = memory
        self._db_config = db_config

    async def add(self, messages: list[MessageParam], user_id: str) -> None:
        """Write a conversation turn to memory. Fire-and-forget."""
        sanitized = _sanitize_messages(messages)
        if not sanitized:
            return
        try:
            await run_in_threadpool(self._mem.add, sanitized, user_id=user_id)
        except Exception as e:
            logger.warning(f"Memory add failed for user {user_id}: {e}")

    async def search(self, query: str, user_id: str, limit: int = 5) -> list[str]:
        """Vector-search memory. Returns a list of fact strings."""
        try:
            results = await run_in_threadpool(
                self._mem.search, query, top_k=limit, filters={"user_id": user_id}
            )
        except Exception as e:
            logger.warning(f"Memory search failed for user {user_id}: {e}")
            return []
        items = results.get("results", []) if isinstance(results, dict) else results
        return [it.get("memory", "") for it in items if isinstance(it, dict)]

    async def list(self, user_id: str) -> list[dict[str, Any]]:
        """Return every memory entry for user_id."""
        try:
            results = await run_in_threadpool(
                self._mem.get_all, filters={"user_id": user_id}
            )
        except Exception as e:
            logger.warning(f"Memory list failed for user {user_id}: {e}")
            return []
        items = results.get("results", []) if isinstance(results, dict) else results
        return [it for it in items if isinstance(it, dict)]

    async def delete(self, memory_id: str) -> bool:
        """Delete a single memory by mem0 id. Returns True on success."""
        try:
            await run_in_threadpool(self._mem.delete, memory_id)
            return True
        except Exception as e:
            logger.warning(f"Memory delete failed for {memory_id}: {e}")
            return False

    def _purge_user_across_all_collections(self, user_id: str) -> int:
        """Delete user's rows from every mem0_memories* table in public.

        Embedder changes create a new fingerprinted collection; this
        scan guarantees "Delete all" is complete across old ones too.
        """
        try:
            with psycopg.connect(
                host=self._db_config["host"],
                port=self._db_config.get("port", 5432),
                dbname=self._db_config["dbname"],
                user=self._db_config["user"],
                password=self._db_config["password"],
            ) as conn:
                cur = conn.execute(
                    "SELECT tablename FROM pg_tables "
                    "WHERE schemaname = 'public' AND tablename LIKE 'mem0_memories%'",
                )
                tables = cur.fetchall()
                total = 0
                for (table,) in tables:
                    result = conn.execute(
                        f'DELETE FROM "{table}" WHERE payload->>\'user_id\' = %s',
                        (user_id,),
                    )
                    total += result.rowcount
                conn.commit()
            return total
        except Exception as e:
            logger.warning(f"Multi-collection purge failed for user {user_id}: {e}")
            return 0

    def _delete_all_sync(self, user_id: str) -> int:
        # mem0 clears the active collection.
        self._mem.delete_all(user_id=user_id)

        # Clear the SQLite messages ring-buffer so deleted facts do not
        # get re-extracted on the next add() — mem0 feeds the last 10
        # raw messages into the LLM extraction prompt.
        try:
            with sqlite3.connect(self._mem.db.db_path) as conn:
                conn.execute(
                    "DELETE FROM messages WHERE session_scope = ?",
                    (f"user_id={user_id}",),
                )
        except Exception as e:
            logger.warning(f"Memory ring-buffer clear failed for {user_id}: {e}")

        return self._purge_user_across_all_collections(user_id)

    async def delete_all(self, user_id: str) -> int:
        """Delete all memories for user_id. Returns total rows purged."""
        try:
            return await run_in_threadpool(self._delete_all_sync, user_id)
        except Exception as e:
            logger.warning(f"Memory delete_all failed for {user_id}: {e}")
            return 0

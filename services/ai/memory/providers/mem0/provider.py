"""mem0 implementation of MemoryProvider.

mem0 is treated as one swap-in implementation behind the abstraction —
nothing outside this module imports `mem0` or relies on its data shapes.

History DB note: mem0 keeps the last 10 raw conversation messages in a
SQLite file used to feed its LLM extraction prompt. We default the path
to /tmp/mem0_history.db (ephemeral, lost on container restart). The cost
of losing the buffer on restart is at most one less context window for
fact extraction on the next add() — acceptable for a swap-in legacy
provider.
"""

import logging
import sqlite3
from datetime import datetime
from typing import Any

from anthropic.types import MessageParam
from asyncpg import Pool
from fastapi.concurrency import run_in_threadpool

from config import (
    DATABASE_HOST,
    DATABASE_NAME,
    DATABASE_PASSWORD,
    DATABASE_PORT,
    DATABASE_USERNAME,
    MEM0_HISTORY_DB_PATH,
)
from db.connection import get_db_pool
from memory.provider import (
    DEFAULT_LIST_LIMIT,
    MemoryProvider,
    MemoryRecord,
    MemorySearchHit,
)

from .bootstrap import (
    DatabaseSettings,
    MemoryConfigError,
    build_mem0_config,
)

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
                b.get("text", "")
                for b in content
                if isinstance(b, dict) and b.get("type") == "text"
            )
        if content and isinstance(content, str):
            out.append({"role": m.get("role", "user"), "content": content})
    return out


def _parse_created_at(raw: Any) -> datetime | None:
    if not isinstance(raw, str):
        return None
    try:
        return datetime.fromisoformat(raw.replace("Z", "+00:00"))
    except ValueError:
        return None


def _to_record(item: dict[str, Any], key: str) -> MemoryRecord:
    """Convert a mem0 result dict into a provider-agnostic MemoryRecord."""
    return MemoryRecord(
        id=str(item.get("id", "")),
        text=item.get("memory", "") or "",
        key=key,
        created_at=_parse_created_at(item.get("created_at")),
        metadata={
            k: v for k, v in item.items() if k not in ("id", "memory", "created_at")
        },
    )


class Mem0Provider:
    """MemoryProvider implementation backed by an in-process mem0.Memory.

    `db_pool` is the same asyncpg pool the rest of omni-ai uses — we
    reuse it for the cross-collection purge query so we don't spin up a
    second pool. mem0 still manages its own psycopg pool internally for
    its pgvector tables; that's not avoidable without forking the lib.
    """

    def __init__(self, memory: Any, db_pool: Pool):
        self._mem = memory
        self._db_pool = db_pool

    async def add(self, messages: list[MessageParam], key: str) -> None:
        sanitized = _sanitize_messages(messages)
        if not sanitized:
            return
        try:
            await run_in_threadpool(self._mem.add, sanitized, user_id=key)
        except Exception as e:
            logger.warning(f"Memory add failed for {key}: {e}")

    async def search(
        self, query: str, key: str, limit: int = 5
    ) -> list[MemorySearchHit]:
        try:
            results = await run_in_threadpool(
                self._mem.search, query, top_k=limit, filters={"user_id": key}
            )
        except Exception as e:
            logger.warning(f"Memory search failed for {key}: {e}")
            return []
        items = results.get("results", []) if isinstance(results, dict) else results
        hits: list[MemorySearchHit] = []
        for it in items:
            if not isinstance(it, dict):
                continue
            score = it.get("score")
            hits.append(
                MemorySearchHit(
                    record=_to_record(it, key),
                    score=float(score) if isinstance(score, (int, float)) else 0.0,
                )
            )
        return hits

    async def list(
        self,
        key: str,
        limit: int = DEFAULT_LIST_LIMIT,
        offset: int = 0,
    ) -> list[MemoryRecord]:
        try:
            results = await run_in_threadpool(
                self._mem.get_all, filters={"user_id": key}
            )
        except Exception as e:
            logger.warning(f"Memory list failed for {key}: {e}")
            return []
        items = results.get("results", []) if isinstance(results, dict) else results
        records = [_to_record(it, key) for it in items if isinstance(it, dict)]
        # mem0's get_all has no native pagination, so slice in memory.
        return records[offset : offset + limit]

    async def delete(self, memory_id: str) -> bool:
        try:
            await run_in_threadpool(self._mem.delete, memory_id)
            return True
        except Exception as e:
            logger.warning(f"Memory delete failed for {memory_id}: {e}")
            return False

    async def _purge_user_across_all_collections(self, key: str) -> int:
        """Delete `key`'s rows from every mem0_memories* table in public.

        Embedder changes create a new fingerprinted collection; this
        scan guarantees "Delete all" is complete across old ones too.
        """
        try:
            async with self._db_pool.acquire() as conn:
                tables = await conn.fetch(
                    "SELECT tablename FROM pg_tables "
                    "WHERE schemaname = 'public' AND tablename LIKE 'mem0_memories%'"
                )
                total = 0
                for row in tables:
                    table = row["tablename"]
                    # Identifier comes from pg_tables (no user input), but we
                    # still quote to be explicit. The user_id is parameterised.
                    result = await conn.execute(
                        f"DELETE FROM \"{table}\" WHERE payload->>'user_id' = $1",
                        key,
                    )
                    # asyncpg returns "DELETE <n>".
                    if isinstance(result, str) and result.startswith("DELETE "):
                        total += int(result.split(" ", 1)[1])
                return total
        except Exception as e:
            logger.warning(f"Multi-collection purge failed for {key}: {e}")
            return 0

    def _clear_sqlite_history(self, key: str) -> None:
        """Drop key's rows from mem0's sqlite messages ring-buffer.

        mem0 feeds the last 10 raw messages into its LLM extraction prompt;
        leaving them around after delete_all would let deleted facts get
        re-extracted on the next add().
        """
        try:
            with sqlite3.connect(self._mem.db.db_path) as conn:
                conn.execute(
                    "DELETE FROM messages WHERE session_scope = ?",
                    (f"user_id={key}",),
                )
        except Exception as e:
            logger.warning(f"Memory ring-buffer clear failed for {key}: {e}")

    async def delete_all(self, key: str) -> int:
        try:
            await run_in_threadpool(self._mem.delete_all, user_id=key)
            await run_in_threadpool(self._clear_sqlite_history, key)
            return await self._purge_user_across_all_collections(key)
        except Exception as e:
            logger.warning(f"Memory delete_all failed for {key}: {e}")
            return 0


async def build_mem0_provider(app_state) -> MemoryProvider | None:
    """Construct a Mem0Provider from app state and the omni-ai DB settings.

    Returns None on failure — callers degrade to no-memory behaviour.
    Raised exceptions from mem0 init are caught here so a misconfigured
    embedder/LLM cannot prevent the AI service from booting.
    """
    # Imported lazily so MEMORY_PROVIDER=other doesn't pay the import cost.
    from mem0 import Memory

    db = DatabaseSettings(
        host=DATABASE_HOST,
        port=DATABASE_PORT,
        dbname=DATABASE_NAME,
        user=DATABASE_USERNAME,
        password=DATABASE_PASSWORD,
    )

    try:
        cfg = await build_mem0_config(
            app_state,
            db=db,
            history_db_path=MEM0_HISTORY_DB_PATH,
        )
        memory = await run_in_threadpool(Memory, cfg)
        db_pool = await get_db_pool()
    except (MemoryConfigError, Exception) as e:
        logger.warning(f"mem0 provider initialization failed: {e}")
        return None

    return Mem0Provider(memory, db_pool)

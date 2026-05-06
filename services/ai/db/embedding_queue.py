"""Repository for embedding queue database operations."""

import logging
from enum import StrEnum
from typing import Optional, List
from dataclasses import dataclass
from datetime import datetime
from asyncpg import Pool

from .connection import get_db_pool


class QueueStatus(StrEnum):
    PENDING = "pending"
    PROCESSING = "processing"
    COMPLETED = "completed"
    FAILED = "failed"


logger = logging.getLogger(__name__)


@dataclass
class EmbeddingQueueItem:
    """Represents an embedding_queue item"""

    id: str
    document_id: str
    status: str
    error_message: Optional[str]
    retry_count: int
    created_at: datetime


class EmbeddingQueueRepository:
    """Repository for embedding queue database operations."""

    def __init__(self, pool: Optional[Pool] = None):
        self.pool = pool

    async def _get_pool(self) -> Pool:
        """Get database pool"""
        if self.pool:
            return self.pool
        return await get_db_pool()

    async def get_by_id(self, item_id: str) -> Optional[EmbeddingQueueItem]:
        """Get a queue item by ID."""
        pool = await self._get_pool()

        row = await pool.fetchrow(
            """
            SELECT id, document_id, status, error_message, retry_count, created_at
            FROM embedding_queue
            WHERE id = $1
            """,
            item_id,
        )
        if row:
            return EmbeddingQueueItem(**dict(row))
        return None

    async def get_status_counts(self) -> dict[str, int]:
        """Get counts of queue items grouped by status."""
        pool = await self._get_pool()

        rows = await pool.fetch(
            "SELECT status, COUNT(*) as count FROM embedding_queue GROUP BY status"
        )
        return {row["status"]: int(row["count"]) for row in rows}

    async def get_pending_count(self, max_retries: int) -> int:
        """Get number of pending queue items."""
        pool = await self._get_pool()

        res = await pool.fetchval(
            """
            SELECT COUNT(*)
            FROM embedding_queue
            WHERE retry_count < $1
              AND status IN ('pending', 'failed')
            """,
            max_retries,
        )

        return int(res)

    async def get_pending_items(
        self, limit: int, max_retries: int
    ) -> List[EmbeddingQueueItem]:
        """Atomically fetch and claim pending items.

        Uses FOR UPDATE SKIP LOCKED so each item is only claimed by one worker.
        """
        pool = await self._get_pool()

        rows = await pool.fetch(
            """
            UPDATE embedding_queue
            SET status = 'processing'
            WHERE id IN (
                SELECT id
                FROM embedding_queue
                WHERE retry_count < $1
                  AND status IN ('pending', 'failed')
                ORDER BY created_at ASC
                LIMIT $2
                FOR UPDATE SKIP LOCKED
            )
            RETURNING id, document_id, status, error_message, retry_count, created_at
            """,
            max_retries,
            limit,
        )
        return [EmbeddingQueueItem(**dict(row)) for row in rows]

    async def mark_completed(self, item_ids: List[str]) -> None:
        """Mark queue items as completed"""
        if not item_ids:
            return

        pool = await self._get_pool()

        await pool.execute(
            """
            UPDATE embedding_queue
            SET status = 'completed', processed_at = CURRENT_TIMESTAMP
            WHERE id = ANY($1)
            """,
            item_ids,
        )
        logger.info(f"Marked {len(item_ids)} queue items as completed")

    async def mark_failed(self, item_ids: List[str], error: str) -> None:
        """Mark queue items as failed"""
        if not item_ids:
            return

        pool = await self._get_pool()

        await pool.execute(
            """
            UPDATE embedding_queue
            SET status = 'failed', error_message = $2, processed_at = CURRENT_TIMESTAMP,
                retry_count = retry_count + 1, updated_at = CURRENT_TIMESTAMP
            WHERE id = ANY($1)
            """,
            item_ids,
            error,
        )
        logger.error(f"Marked {len(item_ids)} queue items as failed: {error}")

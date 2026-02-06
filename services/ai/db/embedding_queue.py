"""Repository for embedding queue database operations."""

import logging
from typing import Optional, List
from dataclasses import dataclass
from datetime import datetime
from asyncpg import Pool

from .connection import get_db_pool

logger = logging.getLogger(__name__)


@dataclass
class EmbeddingQueueItem:
    """Represents an embedding_queue item"""

    id: str
    document_id: str
    status: str
    batch_job_id: Optional[str]
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

    async def get_pending_count(self) -> int:
        """Get number of pending queue items."""
        pool = await self._get_pool()

        res = await pool.fetchval(
            """
            SELECT COUNT(*)
            FROM embedding_queue
            WHERE status = 'pending' AND batch_job_id IS NULL AND retry_count < 5
            """
        )

        return int(res)

    async def get_pending_items(self, limit: int) -> List[EmbeddingQueueItem]:
        """Atomically fetch and claim pending items not assigned to any batch.

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
                WHERE status = 'pending' AND batch_job_id IS NULL AND retry_count < 5
                ORDER BY created_at ASC
                LIMIT $1
                FOR UPDATE SKIP LOCKED
            )
            RETURNING id, document_id, status, batch_job_id, created_at
            """,
            limit,
        )
        return [EmbeddingQueueItem(**dict(row)) for row in rows]

    async def get_items_for_batch(self, batch_id: str) -> List[EmbeddingQueueItem]:
        """Get all queue items for a batch"""
        pool = await self._get_pool()

        rows = await pool.fetch(
            """
            SELECT id, document_id, status, batch_job_id, created_at
            FROM embedding_queue
            WHERE batch_job_id = $1
            ORDER BY created_at ASC
            """,
            batch_id,
        )
        return [EmbeddingQueueItem(**dict(row)) for row in rows]

    async def assign_to_batch(self, batch_id: str, item_ids: List[str]) -> None:
        """Assign queue items to batch job"""
        if not item_ids:
            return

        pool = await self._get_pool()

        await pool.execute(
            """
            UPDATE embedding_queue
            SET batch_job_id = $1
            WHERE id = ANY($2)
            """,
            batch_id,
            item_ids,
        )
        logger.info(f"Assigned {len(item_ids)} items to batch {batch_id}")

    async def mark_processing(self, batch_id: str) -> None:
        """Mark all items in a batch as processing"""
        pool = await self._get_pool()

        await pool.execute(
            """
            UPDATE embedding_queue
            SET status = 'processing'
            WHERE batch_job_id = $1
            """,
            batch_id,
        )

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

    async def mark_pending(self, item_ids: List[str]) -> None:
        """Reset queue items back to pending (remove batch assignment)"""
        if not item_ids:
            return

        pool = await self._get_pool()

        await pool.execute(
            """
            UPDATE embedding_queue
            SET status = 'pending', batch_job_id = NULL
            WHERE id = ANY($1)
            """,
            item_ids,
        )
        logger.info(f"Reset {len(item_ids)} queue items to pending")

    async def mark_failed(self, item_ids: List[str], error: str) -> None:
        """Mark queue items as failed"""
        if not item_ids:
            return

        pool = await self._get_pool()

        await pool.execute(
            """
            UPDATE embedding_queue
            SET status = 'failed', error_message = $2, processed_at = CURRENT_TIMESTAMP
            WHERE id = ANY($1)
            """,
            item_ids,
            error,
        )
        logger.error(f"Marked {len(item_ids)} queue items as failed: {error}")

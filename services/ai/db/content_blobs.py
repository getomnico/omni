"""Repository for content blob database operations."""

import logging
from typing import Optional
from dataclasses import dataclass
from asyncpg import Pool

from .connection import get_db_pool

logger = logging.getLogger(__name__)


@dataclass
class ContentBlobRecord:
    """Content blob record from database"""

    id: str
    content: Optional[bytes]
    storage_key: Optional[str]
    storage_backend: str


class ContentBlobsRepository:
    """Repository for content blob database operations."""

    def __init__(self, pool: Optional[Pool] = None):
        self.pool = pool

    async def _get_pool(self) -> Pool:
        """Get database pool"""
        if self.pool:
            return self.pool
        return await get_db_pool()

    async def get_by_id(self, content_id: str) -> Optional[ContentBlobRecord]:
        """Get content blob by ID, including content and storage info."""
        pool = await self._get_pool()

        row = await pool.fetchrow(
            """
            SELECT id, content, storage_key, storage_backend
            FROM content_blobs
            WHERE id = $1
            """,
            content_id,
        )

        if row:
            return ContentBlobRecord(
                id=row["id"],
                content=row["content"],
                storage_key=row["storage_key"],
                storage_backend=row["storage_backend"],
            )
        return None

    async def insert_postgres(
        self, content_id: str, content: bytes, content_type: str
    ) -> None:
        """Insert a Postgres-backed content blob."""
        pool = await self._get_pool()
        await pool.execute(
            """
            INSERT INTO content_blobs (id, content, content_type, size_bytes, storage_backend)
            VALUES ($1, $2, $3, $4, 'postgres')
            """,
            content_id,
            content,
            content_type,
            len(content),
        )

    async def insert_s3(
        self,
        content_id: str,
        storage_key: str,
        content_type: str,
        size_bytes: int,
    ) -> None:
        """Insert an S3-backed content blob (bytes already uploaded to S3)."""
        pool = await self._get_pool()
        await pool.execute(
            """
            INSERT INTO content_blobs
                (id, storage_key, content_type, size_bytes, storage_backend)
            VALUES ($1, $2, $3, $4, 's3')
            """,
            content_id,
            storage_key,
            content_type,
            size_bytes,
        )

"""Repository for user-owned ad-hoc uploads."""

import logging
from dataclasses import dataclass
from datetime import datetime
from typing import Optional

from asyncpg import Pool

from .connection import get_db_pool

logger = logging.getLogger(__name__)


@dataclass
class Upload:
    id: str
    user_id: str
    content_id: str
    filename: str
    content_type: str
    size_bytes: int
    created_at: datetime


class UploadsRepository:
    def __init__(self, pool: Optional[Pool] = None):
        self.pool = pool

    async def _get_pool(self) -> Pool:
        if self.pool:
            return self.pool
        return await get_db_pool()

    async def create(
        self,
        upload_id: str,
        user_id: str,
        content_id: str,
        filename: str,
        content_type: str,
        size_bytes: int,
    ) -> Upload:
        pool = await self._get_pool()
        row = await pool.fetchrow(
            """
            INSERT INTO uploads (id, user_id, content_id, filename, content_type, size_bytes)
            VALUES ($1, $2, $3, $4, $5, $6)
            RETURNING id, user_id, content_id, filename, content_type, size_bytes, created_at
            """,
            upload_id,
            user_id,
            content_id,
            filename,
            content_type,
            size_bytes,
        )
        return Upload(**dict(row))

    async def get(self, upload_id: str) -> Optional[Upload]:
        pool = await self._get_pool()
        row = await pool.fetchrow(
            """
            SELECT id, user_id, content_id, filename, content_type, size_bytes, created_at
            FROM uploads WHERE id = $1
            """,
            upload_id,
        )
        return Upload(**dict(row)) if row else None

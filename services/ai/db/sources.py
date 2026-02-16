from typing import Optional

from asyncpg import Pool

from .connection import get_db_pool


class SourcesRepository:
    def __init__(self, pool: Optional[Pool] = None):
        self.pool = pool

    async def _get_pool(self) -> Pool:
        if self.pool:
            return self.pool
        return await get_db_pool()

    async def get_active_sources(self) -> list[dict]:
        """Return source_type and name for all active, non-deleted sources."""
        pool = await self._get_pool()
        async with pool.acquire() as conn:
            rows = await conn.fetch(
                "SELECT source_type, name FROM sources WHERE is_active = true AND is_deleted = false"
            )
        return [
            {"source_type": row["source_type"], "name": row["name"]} for row in rows
        ]

"""Repository for the configuration key-value table."""

import json
import logging

from asyncpg import Pool

from .connection import get_db_pool

logger = logging.getLogger(__name__)


class ConfigurationRepository:
    def __init__(self, pool: Pool | None = None):
        self.pool = pool

    async def _get_pool(self) -> Pool:
        if self.pool:
            return self.pool
        return await get_db_pool()

    async def get(self, key: str) -> dict | None:
        """Return the JSONB value for the given key, or None if not found."""
        pool = await self._get_pool()
        async with pool.acquire() as conn:
            row = await conn.fetchrow(
                "SELECT value FROM configuration WHERE key = $1", key
            )
        if row is None:
            return None
        value = row["value"]
        if isinstance(value, str):
            return json.loads(value)
        return dict(value)

    async def set(self, key: str, value: dict) -> None:
        """Upsert a configuration key with the given JSONB value."""
        pool = await self._get_pool()
        async with pool.acquire() as conn:
            await conn.execute(
                """
                INSERT INTO configuration (key, value)
                VALUES ($1, $2::jsonb)
                ON CONFLICT (key) DO UPDATE
                    SET value = EXCLUDED.value,
                        updated_at = NOW()
                """,
                key,
                json.dumps(value),
            )

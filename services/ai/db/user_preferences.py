import json
from typing import Any

from asyncpg import Pool

from .connection import get_db_pool


class UserPreferencesRepository:
    def __init__(self, pool: Pool | None = None):
        self.pool = pool

    async def _get_pool(self) -> Pool:
        if self.pool:
            return self.pool
        return await get_db_pool()

    async def get(self, user_id: str, key: str) -> Any:
        pool = await self._get_pool()
        row = await pool.fetchrow(
            "SELECT value FROM user_preferences WHERE user_id = $1 AND key = $2",
            user_id, key,
        )
        if row is None:
            return None
        value = row["value"]
        return json.loads(value) if isinstance(value, str) else value

    async def set(self, user_id: str, key: str, value: Any) -> None:
        pool = await self._get_pool()
        await pool.execute(
            """
            INSERT INTO user_preferences (user_id, key, value)
            VALUES ($1, $2, $3::jsonb)
            ON CONFLICT (user_id, key) DO UPDATE
                SET value = EXCLUDED.value, updated_at = NOW()
            """,
            user_id, key, json.dumps(value),
        )

    async def delete(self, user_id: str, key: str) -> None:
        pool = await self._get_pool()
        await pool.execute(
            "DELETE FROM user_preferences WHERE user_id = $1 AND key = $2",
            user_id, key,
        )

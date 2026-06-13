"""Read-only access to the unified `configuration` table.

omni-web is the writer for this table; omni-ai only reads. Two helpers
mirror the table's two scopes:

  - `get_global(key)` — admin-set, org-wide value
  - `get_user(user_id, key)` — per-user override
"""

import json
from typing import Any

from asyncpg import Pool

from .connection import get_db_pool
from .models import GlobalConfiguration


def _decode_value(value: Any) -> dict | str | None:
    """Decode a JSONB column. asyncpg returns dict/list/str depending on shape."""
    if isinstance(value, str):
        try:
            value = json.loads(value)
        except json.JSONDecodeError:
            return value
    if isinstance(value, (dict, str)):
        return value
    return None


class ConfigurationRepository:
    def __init__(self, pool: Pool | None = None):
        self.pool = pool

    async def _get_pool(self) -> Pool:
        if self.pool:
            return self.pool
        return await get_db_pool()

    async def get_global_configuration(self) -> GlobalConfiguration:
        """Return all global-scope configuration values as a typed object."""
        pool = await self._get_pool()
        rows = await pool.fetch(
            "SELECT key, value FROM configuration WHERE scope = 'global'",
        )
        return GlobalConfiguration.from_rows([dict(row) for row in rows])

    async def get_global(self, key: str) -> dict | None:
        """Return the JSONB value for the global-scope `key`, or None."""
        pool = await self._get_pool()
        row = await pool.fetchrow(
            "SELECT value FROM configuration WHERE scope = 'global' AND key = $1",
            key,
        )
        if row is None:
            return None
        decoded = _decode_value(row["value"])
        return decoded if isinstance(decoded, dict) else None

    async def get_user(self, user_id: str, key: str) -> dict | None:
        """Return the JSONB value for the per-user `key`, or None."""
        pool = await self._get_pool()
        row = await pool.fetchrow(
            "SELECT value FROM configuration "
            "WHERE scope = 'user' AND user_id = $1 AND key = $2",
            user_id,
            key,
        )
        if row is None:
            return None
        decoded = _decode_value(row["value"])
        return decoded if isinstance(decoded, dict) else None

"""Read-only access to the unified `configuration` table.

omni-web is the writer for this table; omni-ai only reads. Two helpers
mirror the table's two scopes:

  - `get_global(key)` — admin-set, org-wide value
  - `get_user(user_id, key)` — per-user override
  - `get_user_memory_mode(user_id)` — typed accessor for memory_mode
"""

import json
import logging
from typing import Any

from asyncpg import Pool

from memory import MemoryMode

from .connection import get_db_pool

logger = logging.getLogger(__name__)


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

    async def get_user_memory_mode(self, user_id: str) -> MemoryMode | None:
        """Return the user's stored memory_mode preference, or None if unset.

        Unrecognised values in the DB are logged and treated as None so
        that an out-of-band write cannot silently grant a higher mode
        than `MemoryMode` understands.
        """
        pool = await self._get_pool()
        row = await pool.fetchrow(
            "SELECT value FROM configuration "
            "WHERE scope = 'user' AND user_id = $1 AND key = $2",
            user_id,
            "memory_mode",
        )
        if row is None:
            return None
        decoded = _decode_value(row["value"])
        # Accept either {"value": "<mode>"} (admin UI), {"mode": "<mode>"}
        # (legacy migration seed), or a bare "<mode>" string.
        if isinstance(decoded, dict):
            raw = decoded.get("value") or decoded.get("mode")
        elif isinstance(decoded, str):
            raw = decoded
        else:
            logger.warning(
                f"Unexpected memory_mode value shape for user {user_id}: "
                f"{decoded!r}"
            )
            return None
        return MemoryMode.parse(raw)

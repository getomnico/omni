import json
import logging
from dataclasses import dataclass
from datetime import datetime
from typing import Optional

from asyncpg import Pool

from crypto import decrypt_config
from .connection import get_db_pool

logger = logging.getLogger(__name__)


@dataclass
class WebSearchProviderRecord:
    id: str
    name: str
    provider_type: str
    config: dict
    is_current: bool
    is_deleted: bool
    created_at: datetime
    updated_at: datetime

    @classmethod
    def from_row(cls, row: dict) -> "WebSearchProviderRecord":
        config = row["config"]
        if isinstance(config, str):
            config = json.loads(config)
        config = decrypt_config(config)
        return cls(
            id=row["id"].strip(),
            name=row["name"],
            provider_type=row["provider_type"],
            config=config,
            is_current=row["is_current"],
            is_deleted=row["is_deleted"],
            created_at=row["created_at"],
            updated_at=row["updated_at"],
        )


class WebSearchProvidersRepository:
    def __init__(self, pool: Optional[Pool] = None):
        self.pool = pool

    async def _get_pool(self) -> Pool:
        if self.pool:
            return self.pool
        return await get_db_pool()

    async def list_active(self) -> list[WebSearchProviderRecord]:
        pool = await self._get_pool()
        query = """
            SELECT id, name, provider_type, config, is_current, is_deleted, created_at, updated_at
            FROM web_search_providers
            WHERE is_deleted = FALSE
            ORDER BY created_at ASC
        """
        async with pool.acquire() as conn:
            rows = await conn.fetch(query)
        return [WebSearchProviderRecord.from_row(dict(row)) for row in rows]

    async def get_current(self) -> Optional[WebSearchProviderRecord]:
        pool = await self._get_pool()
        query = """
            SELECT id, name, provider_type, config, is_current, is_deleted, created_at, updated_at
            FROM web_search_providers
            WHERE is_current = TRUE AND is_deleted = FALSE
            LIMIT 1
        """
        async with pool.acquire() as conn:
            row = await conn.fetchrow(query)
        if row:
            return WebSearchProviderRecord.from_row(dict(row))
        return None

    async def get(self, provider_id: str) -> Optional[WebSearchProviderRecord]:
        pool = await self._get_pool()
        query = """
            SELECT id, name, provider_type, config, is_current, is_deleted, created_at, updated_at
            FROM web_search_providers
            WHERE id = $1
        """
        async with pool.acquire() as conn:
            row = await conn.fetchrow(query, provider_id)
        if row:
            return WebSearchProviderRecord.from_row(dict(row))
        return None

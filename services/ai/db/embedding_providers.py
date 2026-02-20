import json
import logging
from dataclasses import dataclass
from datetime import datetime
from typing import Optional

from asyncpg import Pool

from .connection import get_db_pool

logger = logging.getLogger(__name__)


@dataclass
class EmbeddingProviderRecord:
    id: str
    name: str
    provider_type: str
    config: dict
    is_current: bool
    is_deleted: bool
    created_at: datetime
    updated_at: datetime

    @classmethod
    def from_row(cls, row: dict) -> "EmbeddingProviderRecord":
        config = row["config"]
        if isinstance(config, str):
            config = json.loads(config)
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


class EmbeddingProvidersRepository:
    def __init__(self, pool: Optional[Pool] = None):
        self.pool = pool

    async def _get_pool(self) -> Pool:
        if self.pool:
            return self.pool
        return await get_db_pool()

    async def get_current(self) -> Optional[EmbeddingProviderRecord]:
        pool = await self._get_pool()
        query = """
            SELECT id, name, provider_type, config, is_current, is_deleted, created_at, updated_at
            FROM embedding_providers
            WHERE is_current = TRUE AND is_deleted = FALSE
            LIMIT 1
        """
        async with pool.acquire() as conn:
            row = await conn.fetchrow(query)
        if row:
            return EmbeddingProviderRecord.from_row(dict(row))
        return None

    async def get_current_fingerprint(self) -> Optional[tuple[str, datetime]]:
        """Return (id, updated_at) for the current provider, or None if no provider is set."""
        pool = await self._get_pool()
        query = """
            SELECT id, updated_at
            FROM embedding_providers
            WHERE is_current = TRUE AND is_deleted = FALSE
            LIMIT 1
        """
        async with pool.acquire() as conn:
            row = await conn.fetchrow(query)
        if row:
            return (row["id"].strip(), row["updated_at"])
        return None

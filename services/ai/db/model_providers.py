import json
import logging
from dataclasses import dataclass
from datetime import datetime
from typing import Optional

from asyncpg import Pool

from .connection import get_db_pool
from .models import ModelRecord

logger = logging.getLogger(__name__)


@dataclass
class ModelProviderRecord:
    id: str
    name: str
    provider_type: str
    config: dict
    is_deleted: bool
    created_at: datetime
    updated_at: datetime

    @classmethod
    def from_row(cls, row: dict) -> "ModelProviderRecord":
        config = row["config"]
        if isinstance(config, str):
            config = json.loads(config)
        return cls(
            id=row["id"].strip(),
            name=row["name"],
            provider_type=row["provider_type"],
            config=config,
            is_deleted=row["is_deleted"],
            created_at=row["created_at"],
            updated_at=row["updated_at"],
        )


class ModelProvidersRepository:
    def __init__(self, pool: Optional[Pool] = None):
        self.pool = pool

    async def _get_pool(self) -> Pool:
        if self.pool:
            return self.pool
        return await get_db_pool()

    async def list_active(self) -> list[ModelProviderRecord]:
        pool = await self._get_pool()
        query = """
            SELECT id, name, provider_type, config, is_deleted, created_at, updated_at
            FROM model_providers
            WHERE is_deleted = FALSE
            ORDER BY created_at ASC
        """
        async with pool.acquire() as conn:
            rows = await conn.fetch(query)
        return [ModelProviderRecord.from_row(dict(row)) for row in rows]

    async def get(self, provider_id: str) -> Optional[ModelProviderRecord]:
        pool = await self._get_pool()
        query = """
            SELECT id, name, provider_type, config, is_deleted, created_at, updated_at
            FROM model_providers
            WHERE id = $1
        """
        async with pool.acquire() as conn:
            row = await conn.fetchrow(query, provider_id)
        if row:
            return ModelProviderRecord.from_row(dict(row))
        return None


class ModelsRepository:
    def __init__(self, pool: Optional[Pool] = None):
        self.pool = pool

    async def _get_pool(self) -> Pool:
        if self.pool:
            return self.pool
        return await get_db_pool()

    async def list_active(self) -> list[ModelRecord]:
        pool = await self._get_pool()
        query = """
            SELECT m.id, m.model_provider_id, m.model_id, m.display_name,
                   m.is_default, m.is_deleted, m.created_at, m.updated_at,
                   mp.provider_type, mp.config
            FROM models m
            JOIN model_providers mp ON m.model_provider_id = mp.id
            WHERE m.is_deleted = FALSE AND mp.is_deleted = FALSE
            ORDER BY m.is_default DESC, m.created_at ASC
        """
        async with pool.acquire() as conn:
            rows = await conn.fetch(query)
        return [ModelRecord.from_row(dict(row)) for row in rows]

    async def get(self, model_id: str) -> Optional[ModelRecord]:
        pool = await self._get_pool()
        query = """
            SELECT m.id, m.model_provider_id, m.model_id, m.display_name,
                   m.is_default, m.is_deleted, m.created_at, m.updated_at,
                   mp.provider_type, mp.config
            FROM models m
            JOIN model_providers mp ON m.model_provider_id = mp.id
            WHERE m.id = $1
        """
        async with pool.acquire() as conn:
            row = await conn.fetchrow(query, model_id)
        if row:
            return ModelRecord.from_row(dict(row))
        return None

    async def get_default(self) -> Optional[ModelRecord]:
        pool = await self._get_pool()
        query = """
            SELECT m.id, m.model_provider_id, m.model_id, m.display_name,
                   m.is_default, m.is_deleted, m.created_at, m.updated_at,
                   mp.provider_type, mp.config
            FROM models m
            JOIN model_providers mp ON m.model_provider_id = mp.id
            WHERE m.is_default = TRUE AND m.is_deleted = FALSE AND mp.is_deleted = FALSE
            LIMIT 1
        """
        async with pool.acquire() as conn:
            row = await conn.fetchrow(query)
        if row:
            return ModelRecord.from_row(dict(row))
        return None

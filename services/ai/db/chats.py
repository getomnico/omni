from typing import Optional
from datetime import datetime
from ulid import ULID
import asyncpg
from asyncpg import Pool

from .models import Chat
from .connection import get_db_pool


class ChatsRepository:
    def __init__(self, pool: Optional[Pool] = None):
        self.pool = pool

    async def _get_pool(self) -> Pool:
        """Get database pool"""
        if self.pool:
            return self.pool
        return await get_db_pool()

    async def create(
        self,
        user_id: str,
        title: Optional[str] = None,
        model_id: Optional[str] = None,
    ) -> Chat:
        """Create a new chat"""
        pool = await self._get_pool()

        chat_id = str(ULID())

        query = """
            INSERT INTO chats (id, user_id, title, model_id, created_at, updated_at)
            VALUES ($1, $2, $3, $4, NOW(), NOW())
            RETURNING id, user_id, title, model_id, created_at, updated_at
        """

        async with pool.acquire() as conn:
            row = await conn.fetchrow(query, chat_id, user_id, title, model_id)

        return Chat.from_row(dict(row))

    async def get(self, chat_id: str) -> Optional[Chat]:
        """Get a chat by ID"""
        pool = await self._get_pool()

        query = """
            SELECT id, user_id, title, model_id, created_at, updated_at
            FROM chats
            WHERE id = $1
        """

        async with pool.acquire() as conn:
            row = await conn.fetchrow(query, chat_id)

        if row:
            return Chat.from_row(dict(row))
        return None

    async def update_title(self, chat_id: str, title: str) -> Optional[Chat]:
        """Update the title of a chat"""
        pool = await self._get_pool()

        query = """
            UPDATE chats
            SET title = $2, updated_at = NOW()
            WHERE id = $1
            RETURNING id, user_id, title, model_id, created_at, updated_at
        """

        async with pool.acquire() as conn:
            row = await conn.fetchrow(query, chat_id, title)

        if row:
            return Chat.from_row(dict(row))
        return None

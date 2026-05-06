from typing import Optional
from ulid import ULID
from asyncpg import Pool

from .models import Chat
from .connection import get_db_pool


_CHAT_COLUMNS = (
    "id, user_id, title, model_id, agent_id, loaded_toolsets, created_at, updated_at"
)


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
        agent_id: Optional[str] = None,
    ) -> Chat:
        """Create a new chat"""
        pool = await self._get_pool()

        chat_id = str(ULID())

        query = f"""
            INSERT INTO chats (id, user_id, title, model_id, agent_id, created_at, updated_at)
            VALUES ($1, $2, $3, $4, $5, NOW(), NOW())
            RETURNING {_CHAT_COLUMNS}
        """

        async with pool.acquire() as conn:
            row = await conn.fetchrow(
                query, chat_id, user_id, title, model_id, agent_id
            )

        return Chat.from_row(dict(row))

    async def get(self, chat_id: str) -> Optional[Chat]:
        """Get a chat by ID"""
        pool = await self._get_pool()

        query = f"""
            SELECT {_CHAT_COLUMNS}
            FROM chats
            WHERE id = $1 AND is_deleted = FALSE
        """

        async with pool.acquire() as conn:
            row = await conn.fetchrow(query, chat_id)

        if row:
            return Chat.from_row(dict(row))
        return None

    async def update_title(self, chat_id: str, title: str) -> Optional[Chat]:
        """Update the title of a chat"""
        pool = await self._get_pool()

        query = f"""
            UPDATE chats
            SET title = $2, updated_at = NOW()
            WHERE id = $1 AND is_deleted = FALSE
            RETURNING {_CHAT_COLUMNS}
        """

        async with pool.acquire() as conn:
            row = await conn.fetchrow(query, chat_id, title)

        if row:
            return Chat.from_row(dict(row))
        return None

    async def update_loaded_toolsets(self, chat_id: str, source_ids: list[str]) -> None:
        """Union-merge source_ids into chats.loaded_toolsets.

        Uses set-union semantics so concurrent streaming requests for the same
        chat (rare, but possible) cannot clobber each other's loads.
        """
        if not source_ids:
            return

        pool = await self._get_pool()

        query = """
            UPDATE chats
            SET loaded_toolsets = COALESCE(
                (SELECT array_agg(DISTINCT x) FROM unnest(loaded_toolsets || $2::text[]) x),
                '{}'::text[]
            ),
            updated_at = NOW()
            WHERE id = $1
        """

        async with pool.acquire() as conn:
            await conn.execute(query, chat_id, list(source_ids))

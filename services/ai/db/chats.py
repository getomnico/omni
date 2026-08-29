from typing import Optional
from ulid import ULID
from asyncpg import Pool

from .models import Chat, ChatSearchHit
from .connection import get_db_pool


_CHAT_COLUMNS = "id, user_id, title, model_id, agent_id, created_at, updated_at"


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

    async def search(
        self,
        user_id: str,
        query: str,
        limit: int,
        exclude_chat_id: str | None = None,
    ) -> list[ChatSearchHit]:
        """Search a user's non-deleted chats by title or message content."""
        pool = await self._get_pool()

        search_query = """
            WITH title_matches AS (
                SELECT
                    c.id,
                    c.title,
                    c.updated_at,
                    NULL::varchar AS matched_message_id,
                    NULL::text AS snippet,
                    (
                        SELECT COUNT(*)::integer
                        FROM chat_messages count_cm
                        WHERE count_cm.chat_id = c.id
                    ) AS message_count,
                    pdb.score(c.id) AS score,
                    'title'::text AS source
                FROM chats c
                WHERE c.title IS NOT NULL
                  AND c.title ||| $2
                  AND c.user_id = $1
                  AND c.is_deleted = FALSE
                  AND ($3::varchar IS NULL OR c.id <> $3::varchar)
            ),
            message_matches AS (
                SELECT DISTINCT ON (c.id)
                    c.id,
                    c.title,
                    c.updated_at,
                    cm.id AS matched_message_id,
                    cm.content_text AS snippet,
                    (
                        SELECT COUNT(*)::integer
                        FROM chat_messages count_cm
                        WHERE count_cm.chat_id = c.id
                    ) AS message_count,
                    pdb.score(cm.id) AS score,
                    'message'::text AS source
                FROM chat_messages cm
                JOIN chats c ON c.id = cm.chat_id
                WHERE cm.content_text IS NOT NULL
                  AND cm.content_text ||| $2
                  AND c.user_id = $1
                  AND c.is_deleted = FALSE
                  AND ($3::varchar IS NULL OR c.id <> $3::varchar)
                ORDER BY c.id, score DESC, cm.id
            ),
            ranked_matches AS (
                SELECT DISTINCT ON (id)
                    id,
                    title,
                    updated_at,
                    matched_message_id,
                    snippet,
                    message_count,
                    score,
                    source
                FROM (
                    SELECT * FROM title_matches
                    UNION ALL
                    SELECT * FROM message_matches
                ) AS all_matches
                ORDER BY id, score DESC
            )
            SELECT
                ranked.id,
                ranked.title,
                ranked.updated_at,
                COALESCE(ranked.matched_message_id, message.matched_message_id)
                    AS matched_message_id,
                left(
                    CASE
                        WHEN message.matched_message_id IS NOT NULL
                            THEN message.snippet
                        ELSE COALESCE(preview.preview_snippet, ranked.snippet)
                    END,
                    500
                ) AS snippet,
                ranked.message_count,
                CASE
                    WHEN message.matched_message_id IS NOT NULL THEN 'message'
                    ELSE ranked.source
                END AS source
            FROM ranked_matches ranked
            LEFT JOIN message_matches message ON message.id = ranked.id
            LEFT JOIN LATERAL (
                SELECT cm.content_text AS preview_snippet
                FROM chat_messages cm
                WHERE ranked.source = 'title'
                  AND cm.chat_id = ranked.id
                  AND cm.content_text IS NOT NULL
                  AND btrim(cm.content_text) <> ''
                ORDER BY cm.message_seq_num ASC
                LIMIT 1
            ) preview ON TRUE
            ORDER BY ranked.score DESC, ranked.updated_at DESC, ranked.id
            LIMIT $4
        """

        async with pool.acquire() as conn:
            rows = await conn.fetch(
                search_query,
                user_id,
                query,
                exclude_chat_id,
                limit,
            )

        return [ChatSearchHit.from_row(dict(row)) for row in rows]

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

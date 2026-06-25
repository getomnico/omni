from typing import Dict, Any, Optional, List
from datetime import datetime
from ulid import ULID
import asyncpg
from asyncpg import Pool
import json

from .models import ChatMessage
from .connection import get_db_pool


def _sanitize_jsonb_value(value: Any) -> Any:
    if isinstance(value, str):
        return value.replace("\x00", "\\x00")
    if isinstance(value, list):
        return [_sanitize_jsonb_value(item) for item in value]
    if isinstance(value, dict):
        return {key: _sanitize_jsonb_value(item) for key, item in value.items()}
    return value


def _extract_content_text(message: Dict[str, Any]) -> Optional[str]:
    """Plain-text projection of a message for search/title. Mirrors omni-web's
    extractContentText so rows written here match those written by the web."""
    if message.get("role") not in ("user", "assistant"):
        return None
    content = message.get("content")
    if isinstance(content, str):
        return content
    if isinstance(content, list):
        parts = [
            b.get("text", "")
            for b in content
            if isinstance(b, dict) and b.get("type") == "text"
        ]
        joined = "\n".join(p for p in parts if p)
        return joined or None
    return None


class MessagesRepository:
    def __init__(self, pool: Optional[Pool] = None):
        self.pool = pool

    async def _get_pool(self) -> Pool:
        """Get database pool"""
        if self.pool:
            return self.pool
        return await get_db_pool()

    async def update_message_content(
        self, message_id: str, message: Dict[str, Any]
    ) -> None:
        """Replace the JSONB `message` payload for a chat_messages row."""
        pool = await self._get_pool()
        async with pool.acquire() as conn:
            await conn.execute(
                "UPDATE chat_messages SET message = $1 WHERE id = $2",
                json.dumps(_sanitize_jsonb_value(message)),
                message_id,
            )

    async def create(
        self, chat_id: str, message: Dict[str, Any], parent_id: Optional[str] = None
    ) -> ChatMessage:
        """Create a new message in a chat"""
        pool = await self._get_pool()

        message_id = str(ULID())

        # Get the next sequence number for this chat
        seq_query = """
            SELECT COALESCE(MAX(message_seq_num), 0) + 1 as next_seq
            FROM chat_messages
            WHERE chat_id = $1
        """

        message = _sanitize_jsonb_value(message)
        content_text = _extract_content_text(message)

        async with pool.acquire() as conn:
            next_seq = await conn.fetchval(seq_query, chat_id)

            query = """
                INSERT INTO chat_messages (id, chat_id, message_seq_num, message, parent_id, content_text, created_at)
                VALUES ($1, $2, $3, $4, $5, $6, NOW())
                RETURNING id, chat_id, message_seq_num, message, parent_id, created_at
            """

            row = await conn.fetchrow(
                query,
                message_id,
                chat_id,
                next_seq,
                json.dumps(message),
                parent_id,
                content_text,
            )

        return ChatMessage.from_row(dict(row))

    async def get_by_chat(self, chat_id: str) -> List[ChatMessage]:
        """Get all messages for a chat"""
        pool = await self._get_pool()

        query = """
            SELECT id, chat_id, message_seq_num, message, parent_id, created_at
            FROM chat_messages
            WHERE chat_id = $1
            ORDER BY message_seq_num
        """

        async with pool.acquire() as conn:
            rows = await conn.fetch(query, chat_id)

        return [ChatMessage.from_row(dict(row)) for row in rows]

    async def get_active_path(self, chat_id: str) -> List[ChatMessage]:
        """Get the active branch path (path from root to the leaf with the highest message_seq_num).

        Finds the latest leaf (message with no children and highest seq num),
        then walks up via parent_id to root, and returns in root-to-leaf order.
        """
        pool = await self._get_pool()

        query = """
            WITH RECURSIVE walk_up AS (
                -- Start from the latest leaf (no children, highest seq num)
                SELECT cm.id, cm.chat_id, cm.message_seq_num, cm.message, cm.parent_id, cm.created_at
                FROM (
                    SELECT *
                    FROM chat_messages
                    WHERE chat_id = $1
                    AND id NOT IN (
                        SELECT DISTINCT parent_id FROM chat_messages
                        WHERE chat_id = $1 AND parent_id IS NOT NULL
                    )
                    ORDER BY message_seq_num DESC
                    LIMIT 1
                ) cm

                UNION ALL

                -- Walk up to root via parent_id
                SELECT cm.id, cm.chat_id, cm.message_seq_num, cm.message, cm.parent_id, cm.created_at
                FROM chat_messages cm
                JOIN walk_up wu ON cm.id = wu.parent_id
            )
            SELECT * FROM walk_up ORDER BY message_seq_num
        """

        async with pool.acquire() as conn:
            rows = await conn.fetch(query, chat_id)

        return [ChatMessage.from_row(dict(row)) for row in rows]

"""Telethon MTProto client for full Telegram history access."""

import logging
from typing import Any

from telethon import TelegramClient
from telethon.sessions import StringSession
from telethon.tl.types import (
    Channel,
    Chat,
    User,
)

logger = logging.getLogger(__name__)


class TelethonClient:
    """Async Telethon client wrapper for full history access."""

    def __init__(self, api_id: int, api_hash: str, session_string: str):
        self._client = TelegramClient(
            StringSession(session_string),
            api_id,
            api_hash,
        )

    async def connect(self) -> None:
        """Connect and verify the session is still valid."""
        await self._client.connect()
        if not await self._client.is_user_authorized():
            raise RuntimeError(
                "Session expired or invalid. Re-run the auth script to generate a new session string."
            )
        me = await self._client.get_me()
        logger.info("Telethon connected as: %s (id=%s)", me.first_name, me.id)

    async def list_dialogs(self) -> list[dict[str, Any]]:
        """List all chats/groups/channels the user has."""
        dialogs: list[dict[str, Any]] = []
        async for dialog in self._client.iter_dialogs():
            entity = dialog.entity
            chat_type = _get_entity_type(entity)
            dialogs.append(
                {
                    "id": dialog.id,
                    "title": dialog.title or dialog.name or str(dialog.id),
                    "type": chat_type,
                    "unread_count": dialog.unread_count,
                    "message_count": getattr(entity, "participants_count", None),
                }
            )
        return dialogs

    async def get_messages(
        self,
        chat_id: int,
        limit: int | None = None,
        min_id: int = 0,
    ) -> list[dict[str, Any]]:
        """Fetch messages from a chat, oldest first.

        Args:
            chat_id: The chat to fetch from.
            limit: Max messages to fetch (None = all).
            min_id: Only fetch messages with ID > min_id (for incremental sync).
        """
        messages: list[dict[str, Any]] = []
        async for msg in self._client.iter_messages(
            chat_id,
            limit=limit,
            min_id=min_id,
            reverse=True,
        ):
            messages.append(_serialize_message(msg))
        return messages

    async def get_chat_info(self, chat_id: int) -> dict[str, Any]:
        """Get chat metadata."""
        entity = await self._client.get_entity(chat_id)
        return {
            "id": chat_id,
            "title": getattr(entity, "title", None)
            or f"{getattr(entity, 'first_name', '')} {getattr(entity, 'last_name', '')}".strip()
            or str(chat_id),
            "type": _get_entity_type(entity),
            "username": getattr(entity, "username", None),
            "participants_count": getattr(entity, "participants_count", None),
        }

    async def get_session_string(self) -> str:
        """Export the current session as a string."""
        return self._client.session.save()  # type: ignore[return-value]

    async def close(self) -> None:
        """Disconnect the client."""
        await self._client.disconnect()


def _get_entity_type(entity: Any) -> str:
    if isinstance(entity, Channel):
        return "channel" if entity.broadcast else "supergroup"
    if isinstance(entity, Chat):
        return "group"
    if isinstance(entity, User):
        return "private"
    return "unknown"


def _serialize_message(msg: Any) -> dict[str, Any]:
    """Convert a Telethon Message to a plain dict."""
    sender = msg.sender
    sender_name = ""
    if sender:
        if hasattr(sender, "first_name"):
            sender_name = f"{sender.first_name or ''} {sender.last_name or ''}".strip()
        elif hasattr(sender, "title"):
            sender_name = sender.title or ""

    result: dict[str, Any] = {
        "id": msg.id,
        "date": msg.date.isoformat() if msg.date else None,
        "text": msg.text or "",
        "sender_name": sender_name,
        "sender_id": msg.sender_id,
        "reply_to_msg_id": msg.reply_to.reply_to_msg_id if msg.reply_to else None,
        "forward": bool(msg.forward),
    }

    # Media info
    if msg.photo:
        result["media_type"] = "photo"
    elif msg.document:
        result["media_type"] = "document"
        attrs = {type(a).__name__: a for a in (msg.document.attributes or [])}
        if "DocumentAttributeFilename" in attrs:
            result["file_name"] = attrs["DocumentAttributeFilename"].file_name
        if "DocumentAttributeVideo" in attrs:
            result["media_type"] = "video"
        if "DocumentAttributeAudio" in attrs:
            audio = attrs["DocumentAttributeAudio"]
            result["media_type"] = "voice" if audio.voice else "audio"
        if "DocumentAttributeSticker" in attrs:
            result["media_type"] = "sticker"
            result["sticker_emoji"] = attrs["DocumentAttributeSticker"].alt
    elif msg.poll:
        result["media_type"] = "poll"
        result["poll_question"] = msg.poll.poll.question.text if msg.poll.poll.question else ""

    return result

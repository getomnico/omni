"""Map Telegram API objects to Omni Document format."""

from datetime import datetime, timezone
from typing import Any

from omni_connector.models import Document, DocumentMetadata, DocumentPermissions


def _parse_date(value: Any) -> datetime | None:
    """Parse a date from ISO string, unix timestamp, or datetime."""
    if value is None:
        return None
    if isinstance(value, datetime):
        return value
    if isinstance(value, str):
        return datetime.fromisoformat(value)
    return datetime.fromtimestamp(value, tz=timezone.utc)


def _build_permissions(
    chat_info: dict[str, Any],
    allowed_users: list[str] | None,
) -> DocumentPermissions:
    """Build document permissions.

    If allowed_users is non-empty, restrict visibility to those users.
    Otherwise fall back to public for groups/channels, private for DMs.
    """
    if allowed_users:
        return DocumentPermissions(public=False, users=allowed_users)
    return DocumentPermissions(
        public=chat_info.get("type") in ("group", "supergroup", "channel"),
    )


def map_message_to_document(
    message: dict[str, Any],
    content_id: str,
    chat_info: dict[str, Any],
    bot_username: str,
    allowed_users: list[str] | None = None,
) -> Document:
    """Convert a Telegram message to an Omni Document."""
    chat_id = message["chat"]["id"]
    message_id = message["message_id"]
    chat_title = _get_chat_title(chat_info)

    # Build metadata
    date = message.get("date")
    created_at = _parse_date(date)
    edit_date = message.get("edit_date")
    updated_at = _parse_date(edit_date) or created_at

    sender = message.get("from", {})
    author = _format_user(sender)

    return Document(
        external_id=f"telegram:message:{chat_id}:{message_id}",
        title=f"{chat_title} — {author}" if author else chat_title,
        content_id=content_id,
        metadata=DocumentMetadata(
            title=chat_title,
            author=author,
            created_at=created_at,
            updated_at=updated_at,
            content_type="message",
            mime_type="text/plain",
            url=_build_message_url(chat_info, message_id),
            extra={
                "chat_id": chat_id,
                "message_id": message_id,
                "chat_type": chat_info.get("type", "unknown"),
            },
        ),
        permissions=_build_permissions(chat_info, allowed_users),
        attributes={
            "source_type": "telegram",
            "chat_id": str(chat_id),
            "chat_title": chat_title,
            "chat_type": chat_info.get("type", "unknown"),
            "sender": author,
        },
    )


def map_chat_to_document(
    chat_info: dict[str, Any],
    content_id: str,
    member_count: int | None = None,
    allowed_users: list[str] | None = None,
) -> Document:
    """Convert a Telegram chat/channel to an Omni Document."""
    chat_id = chat_info["id"]
    chat_title = _get_chat_title(chat_info)

    return Document(
        external_id=f"telegram:chat:{chat_id}",
        title=chat_title,
        content_id=content_id,
        metadata=DocumentMetadata(
            title=chat_title,
            content_type="chat",
            mime_type="text/plain",
            extra={
                "chat_id": chat_id,
                "chat_type": chat_info.get("type", "unknown"),
                "member_count": member_count,
            },
        ),
        permissions=_build_permissions(chat_info, allowed_users),
        attributes={
            "source_type": "telegram",
            "chat_id": str(chat_id),
            "chat_type": chat_info.get("type", "unknown"),
        },
    )


def generate_message_content(message: dict[str, Any]) -> str:
    """Extract searchable text content from a message."""
    parts: list[str] = []

    # Text content
    text = message.get("text") or message.get("caption") or ""
    if text:
        parts.append(text)

    # Document/file info
    doc = message.get("document")
    if doc:
        parts.append(f"[File: {doc.get('file_name', 'unknown')}]")

    # Photo
    if message.get("photo"):
        caption = message.get("caption", "")
        parts.append(f"[Photo]{f': {caption}' if caption else ''}")

    # Video
    video = message.get("video")
    if video:
        parts.append(f"[Video: {video.get('file_name', 'video')}]")

    # Audio / Voice
    audio = message.get("audio") or message.get("voice")
    if audio:
        parts.append(f"[Audio: {audio.get('title', audio.get('file_name', 'audio'))}]")

    # Sticker
    sticker = message.get("sticker")
    if sticker:
        parts.append(f"[Sticker: {sticker.get('emoji', '')}]")

    # Poll
    poll = message.get("poll")
    if poll:
        question = poll.get("question", "")
        options = [o.get("text", "") for o in poll.get("options", [])]
        parts.append(f"[Poll: {question}]")
        parts.extend(f"  - {opt}" for opt in options)

    # Location
    location = message.get("location")
    if location:
        parts.append(
            f"[Location: {location.get('latitude')}, {location.get('longitude')}]"
        )

    # Contact
    contact = message.get("contact")
    if contact:
        name = f"{contact.get('first_name', '')} {contact.get('last_name', '')}".strip()
        parts.append(f"[Contact: {name} {contact.get('phone_number', '')}]")

    # Forwarded from
    forward_from = message.get("forward_from") or message.get(
        "forward_from_chat"
    )
    if forward_from:
        forwarded_name = _format_user(forward_from) if "first_name" in forward_from else _get_chat_title(forward_from)
        parts.insert(0, f"[Forwarded from {forwarded_name}]")

    # Reply to
    reply = message.get("reply_to_message")
    if reply:
        reply_text = (reply.get("text") or reply.get("caption") or "")[:100]
        if reply_text:
            parts.insert(0, f"[Reply to: {reply_text}]")

    return "\n".join(parts) if parts else "[Empty message]"


def generate_chat_content(
    chat_info: dict[str, Any], member_count: int | None = None
) -> str:
    """Generate descriptive text for a chat."""
    parts: list[str] = []

    title = _get_chat_title(chat_info)
    parts.append(f"Chat: {title}")
    parts.append(f"Type: {chat_info.get('type', 'unknown')}")

    if member_count is not None:
        parts.append(f"Members: {member_count}")

    desc = chat_info.get("description") or chat_info.get("bio")
    if desc:
        parts.append(f"Description: {desc}")

    username = chat_info.get("username")
    if username:
        parts.append(f"Username: @{username}")

    return "\n".join(parts)


def _get_chat_title(chat: dict[str, Any]) -> str:
    """Extract display title from chat info."""
    if chat.get("title"):
        return chat["title"]
    # Private chat — use user's name
    first = chat.get("first_name", "")
    last = chat.get("last_name", "")
    return f"{first} {last}".strip() or f"Chat {chat.get('id', 'unknown')}"


def _format_user(user: dict[str, Any]) -> str:
    """Format user display name."""
    if not user:
        return ""
    first = user.get("first_name", "")
    last = user.get("last_name", "")
    username = user.get("username", "")
    name = f"{first} {last}".strip()
    if username:
        return f"{name} (@{username})" if name else f"@{username}"
    return name


def _build_message_url(
    chat_info: dict[str, Any], message_id: int
) -> str | None:
    """Build a t.me link for the message if possible."""
    username = chat_info.get("username")
    if username:
        return f"https://t.me/{username}/{message_id}"
    # Supergroups can use chat_id-based links
    chat_id = chat_info.get("id", 0)
    if chat_id and str(chat_id).startswith("-100"):
        # Remove the -100 prefix for the link
        clean_id = str(chat_id)[4:]
        return f"https://t.me/c/{clean_id}/{message_id}"
    return None

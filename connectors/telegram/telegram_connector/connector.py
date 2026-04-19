"""Telegram connector for Omni.

Supports two backends:
- **Telethon (MTProto)**: Full history access via user session. Requires api_id,
  api_hash, and session string from scripts/auth.py.
- **Bot API**: Forward-only indexing via getUpdates. Requires bot token only.

Backend is auto-detected from credentials:
- Has 'session' + 'api_id' + 'api_hash' → Telethon
- Has 'token' → Bot API
"""

import logging
from datetime import datetime, timezone
from typing import Any

from omni_connector import Connector, SyncContext
from omni_connector.models import ActionDefinition, ActionResponse

from .client import AuthenticationError, TelegramClient, TelegramError
from .config import CHECKPOINT_INTERVAL, MAX_CONTENT_LENGTH
from .mappers import (
    generate_chat_content,
    generate_message_content,
    map_chat_to_document,
    map_message_to_document,
)

logger = logging.getLogger(__name__)


class TelegramConnector(Connector):
    """Telegram connector for Omni."""

    @property
    def name(self) -> str:
        return "telegram"

    @property
    def version(self) -> str:
        return "1.1.0"

    @property
    def source_types(self) -> list[str]:
        return ["telegram"]

    @property
    def description(self) -> str:
        return "Connect to Telegram chats, groups, and channels"

    @property
    def sync_modes(self) -> list[str]:
        return ["full", "incremental"]

    @property
    def actions(self) -> list[ActionDefinition]:
        return [
            ActionDefinition(
                name="list_chats",
                description=(
                    "List all available Telegram chats with titles, types, "
                    "and IDs. Use to select which chats to sync."
                ),
                input_schema={"type": "object", "properties": {}},
                mode="read",
            ),
            ActionDefinition(
                name="auth_send_code",
                description=(
                    "Start the Telethon interactive auth flow: request an SMS "
                    "login code from Telegram. Returns a partial StringSession "
                    "(the DC auth key only — no user is logged in yet) plus the "
                    "phone_code_hash that must be passed to auth_verify_code."
                ),
                input_schema={
                    "type": "object",
                    "properties": {
                        "api_id": {"type": "integer"},
                        "api_hash": {"type": "string"},
                        "phone": {"type": "string"},
                    },
                    "required": ["api_id", "api_hash", "phone"],
                },
                mode="read",
                authenticated=False,
            ),
            ActionDefinition(
                name="auth_verify_code",
                description=(
                    "Verify the SMS code (and, if the account has 2FA enabled, "
                    "the 2FA password). On success returns a full StringSession "
                    "that can be written into the source credentials. If the "
                    "account requires 2FA but no password was supplied, returns "
                    "needs_2fa=true so the UI can prompt for it and call this "
                    "action again."
                ),
                input_schema={
                    "type": "object",
                    "properties": {
                        "api_id": {"type": "integer"},
                        "api_hash": {"type": "string"},
                        "partial_session": {"type": "string"},
                        "phone": {"type": "string"},
                        "phone_code_hash": {"type": "string"},
                        "code": {"type": "string"},
                        "password": {"type": "string"},
                    },
                    "required": [
                        "api_id",
                        "api_hash",
                        "partial_session",
                        "phone",
                        "phone_code_hash",
                        "code",
                    ],
                },
                mode="read",
                authenticated=False,
            ),
        ]

    async def execute_action(
        self,
        action: str,
        params: dict[str, Any],
        credentials: dict[str, Any],
    ) -> ActionResponse:
        # Action dispatch wraps credentials in an envelope:
        # {"credentials": {...}, "config": {...}, "principal_email": "..."}
        # Sync dispatch passes them directly. Normalize to the sync shape.
        inner = credentials.get("credentials")
        if isinstance(inner, dict) and (
            "api_id" in inner or "session" in inner or "token" in inner
        ):
            credentials = inner

        if action == "list_chats":
            return await self._action_list_chats(credentials)
        if action == "auth_send_code":
            return await self._action_auth_send_code(params)
        if action == "auth_verify_code":
            return await self._action_auth_verify_code(params)
        return ActionResponse.not_supported(action)

    async def _action_list_chats(self, credentials: dict[str, Any]) -> ActionResponse:
        """List all chats the user has access to (Telethon only)."""
        if not _is_telethon(credentials):
            return ActionResponse.failure(
                "list_chats requires Telethon credentials (api_id, api_hash, session). "
                "Bot API tokens cannot list chats."
            )

        from .telethon_client import TelethonClient

        client = TelethonClient(
            api_id=int(credentials["api_id"]),
            api_hash=credentials["api_hash"],
            session_string=credentials["session"],
        )
        try:
            await client.connect()
            dialogs = await client.list_dialogs()
            return ActionResponse.success({"chats": dialogs, "count": len(dialogs)})
        except Exception as e:
            return ActionResponse.failure(f"Failed to list chats: {e}")
        finally:
            await client.close()

    async def _action_auth_send_code(
        self, params: dict[str, Any]
    ) -> ActionResponse:
        """Start the interactive Telethon auth flow: request an SMS code.

        Stateless: returns a serialized `partial_session` (the DC auth key
        only — no user is logged in yet) plus the `phone_code_hash` that the
        caller must pass back to auth_verify_code. The frontend holds this
        state between steps.
        """
        from telethon import TelegramClient
        from telethon.sessions import StringSession

        api_id = params.get("api_id")
        api_hash = params.get("api_hash")
        phone = params.get("phone")
        if not (api_id and api_hash and phone):
            return ActionResponse.failure(
                "auth_send_code requires api_id, api_hash, and phone"
            )

        try:
            api_id_int = int(api_id)
        except (TypeError, ValueError):
            return ActionResponse.failure("api_id must be an integer")

        client = TelegramClient(StringSession(), api_id_int, str(api_hash))
        try:
            await client.connect()
            sent = await client.send_code_request(str(phone))
            partial = client.session.save()
            return ActionResponse.success(
                {
                    "partial_session": partial,
                    "phone_code_hash": sent.phone_code_hash,
                }
            )
        except Exception as e:
            logger.exception("auth_send_code failed")
            return ActionResponse.failure(f"Failed to send code: {e}")
        finally:
            await client.disconnect()  # type: ignore[func-returns-value]

    async def _action_auth_verify_code(
        self, params: dict[str, Any]
    ) -> ActionResponse:
        """Verify the SMS code (and optional 2FA password).

        On success returns a full StringSession that the caller writes into
        the source credentials. If the account requires 2FA but no password
        was supplied, returns `needs_2fa=true` so the UI can prompt and call
        this action again with `password` set.
        """
        from telethon import TelegramClient
        from telethon.errors import SessionPasswordNeededError
        from telethon.sessions import StringSession

        api_id = params.get("api_id")
        api_hash = params.get("api_hash")
        partial_session = params.get("partial_session")
        phone = params.get("phone")
        phone_code_hash = params.get("phone_code_hash")
        code = params.get("code")
        password = params.get("password")  # optional

        if not all([api_id, api_hash, partial_session, phone, phone_code_hash, code]):
            return ActionResponse.failure(
                "auth_verify_code requires api_id, api_hash, partial_session, "
                "phone, phone_code_hash, and code"
            )

        try:
            api_id_int = int(api_id)
        except (TypeError, ValueError):
            return ActionResponse.failure("api_id must be an integer")

        client = TelegramClient(
            StringSession(str(partial_session)),
            api_id_int,
            str(api_hash),
        )
        try:
            await client.connect()
            try:
                await client.sign_in(
                    phone=str(phone),
                    code=str(code),
                    phone_code_hash=str(phone_code_hash),
                )
            except SessionPasswordNeededError:
                if not password:
                    # Pass the in-flight session back so the frontend can
                    # re-use it (avoids re-sending a new SMS code).
                    return ActionResponse.success(
                        {
                            "needs_2fa": True,
                            "partial_session": client.session.save(),
                        }
                    )
                await client.sign_in(password=str(password))

            me = await client.get_me()
            session = client.session.save()
            username = getattr(me, "username", None)
            first = getattr(me, "first_name", "") or ""
            last = getattr(me, "last_name", "") or ""
            display = f"{first} {last}".strip() or username or str(getattr(me, "id", ""))
            return ActionResponse.success(
                {
                    "session": session,
                    "user": {
                        "id": getattr(me, "id", None),
                        "username": username,
                        "display_name": display,
                        "phone": getattr(me, "phone", None),
                    },
                }
            )
        except Exception as e:
            logger.exception("auth_verify_code failed")
            return ActionResponse.failure(f"Failed to verify code: {e}")
        finally:
            await client.disconnect()  # type: ignore[func-returns-value]

    async def sync(
        self,
        source_config: dict[str, Any],
        credentials: dict[str, Any],
        state: dict[str, Any] | None,
        ctx: SyncContext,
    ) -> None:
        if _is_telethon(credentials):
            await self._sync_telethon(source_config, credentials, state, ctx)
        elif credentials.get("token"):
            await self._sync_bot_api(source_config, credentials, state, ctx)
        else:
            await ctx.fail(
                "Invalid credentials. Provide either "
                "(api_id + api_hash + session) for Telethon, or (token) for Bot API."
            )

    # ── Telethon sync (full history) ─────────────────────────────

    async def _sync_telethon(
        self,
        source_config: dict[str, Any],
        credentials: dict[str, Any],
        state: dict[str, Any] | None,
        ctx: SyncContext,
    ) -> None:
        from .telethon_client import TelethonClient

        client = TelethonClient(
            api_id=int(credentials["api_id"]),
            api_hash=credentials["api_hash"],
            session_string=credentials["session"],
        )

        try:
            await client.connect()
        except Exception as e:
            await ctx.fail(f"Telethon connection failed: {e}")
            return

        state = state or {}
        chat_names: list[str] = source_config.get("chats", [])
        allowed_users: list[str] = source_config.get("allowed_users", [])

        try:
            target_chats = await self._resolve_chats(client, chat_names, source_config)

            if not target_chats:
                logger.warning("No chats configured or found. Nothing to sync.")
                await ctx.complete(state)
                return

            docs_emitted = 0
            chat_states: dict[str, int] = state.get("chat_last_message_ids", {})

            for chat in target_chats:
                if ctx.is_cancelled():
                    await ctx.fail("Cancelled by user")
                    return

                chat_id = int(chat["id"])
                chat_id_str = str(chat_id)
                min_id = int(chat_states.get(chat_id_str, 0))

                logger.info(
                    "Syncing chat: %s (id=%s, min_id=%s)",
                    chat["title"], chat_id, min_id,
                )

                # Emit chat metadata document
                content = generate_chat_content(chat, chat.get("participants_count"))
                content_id = await ctx.content_storage.save(
                    content[:MAX_CONTENT_LENGTH], "text/plain"
                )
                doc = map_chat_to_document(
                    chat, content_id, chat.get("participants_count"), allowed_users or None
                )
                await ctx.emit(doc)
                docs_emitted += 1

                # Fetch and emit messages
                max_msg_id = min_id
                messages = await client.get_messages(chat_id, min_id=min_id)

                for msg in messages:
                    if ctx.is_cancelled():
                        await ctx.fail("Cancelled by user")
                        return

                    await ctx.increment_scanned()

                    msg_text = _build_message_text(msg)
                    if not msg_text:
                        continue

                    content_id = await ctx.content_storage.save(
                        msg_text[:MAX_CONTENT_LENGTH], "text/plain"
                    )

                    bot_fmt = _to_bot_api_format(msg, chat)
                    doc = map_message_to_document(
                        bot_fmt, content_id, chat, "telethon", allowed_users or None
                    )
                    await ctx.emit(doc)
                    docs_emitted += 1
                    max_msg_id = max(max_msg_id, int(msg["id"]))

                    if docs_emitted % CHECKPOINT_INTERVAL == 0:
                        chat_states[chat_id_str] = max_msg_id
                        await ctx.save_state({
                            "chat_last_message_ids": chat_states,
                            "last_sync_at": datetime.now(timezone.utc).isoformat(),
                        })

                chat_states[chat_id_str] = max_msg_id
                logger.info(
                    "Chat %s done: %d messages (max_id=%s)",
                    chat["title"], len(messages), max_msg_id,
                )

            await ctx.complete({
                "chat_last_message_ids": chat_states,
                "last_sync_at": datetime.now(timezone.utc).isoformat(),
            })

        except Exception as e:
            logger.error("Telethon sync failed: %s", e, exc_info=True)
            await ctx.fail(f"Sync failed: {e}")
        finally:
            await client.close()

    async def _resolve_chats(
        self,
        client: Any,
        chat_names: list[str],
        source_config: dict[str, Any],
    ) -> list[dict[str, Any]]:
        """Resolve chat names/IDs from config to chat info dicts.

        Config options (in source_config):
        - "chats": ["Chat Title 1", "Chat Title 2"] — match by title
        - "chat_ids": [123456, -100789] — match by ID (backward compat)
        - Neither → sync ALL dialogs
        """
        chat_ids: list[int | str] = source_config.get("chat_ids", [])

        if not chat_names and not chat_ids:
            logger.info("No chats configured — syncing all dialogs")
            return await client.list_dialogs()

        resolved: list[dict[str, Any]] = []

        for cid in chat_ids:
            try:
                info = await client.get_chat_info(int(cid))
                resolved.append(info)
            except Exception as e:
                logger.warning("Failed to resolve chat_id %s: %s", cid, e)

        if chat_names:
            all_dialogs = await client.list_dialogs()
            name_set = {n.lower().strip() for n in chat_names}
            for dialog in all_dialogs:
                if dialog["title"].lower().strip() in name_set:
                    resolved.append(dialog)
                    name_set.discard(dialog["title"].lower().strip())
            if name_set:
                logger.warning("Could not find chats by name: %s", name_set)

        return resolved

    # ── Bot API sync (forward-only) ──────────────────────────────

    async def _sync_bot_api(
        self,
        source_config: dict[str, Any],
        credentials: dict[str, Any],
        state: dict[str, Any] | None,
        ctx: SyncContext,
    ) -> None:
        token = credentials["token"]
        allowed_users: list[str] = source_config.get("allowed_users", [])
        client = TelegramClient(token=token)

        try:
            bot_user = await client.get_me()
            bot_username = bot_user.get("username", "bot")
            logger.info("Bot API: authenticated as @%s", bot_username)
        except AuthenticationError as e:
            await ctx.fail(f"Authentication failed: {e}")
            return
        except TelegramError as e:
            await ctx.fail(f"Failed to connect: {e}")
            return

        state = state or {}
        last_update_id = state.get("last_update_id")
        offset = (last_update_id + 1) if last_update_id else None
        docs_emitted = 0
        seen_chats: dict[int, dict[str, Any]] = {}
        max_update_id = last_update_id or 0

        try:
            while True:
                if ctx.is_cancelled():
                    await ctx.fail("Cancelled by user")
                    return

                updates = await client.get_updates(offset=offset, limit=100)
                if not updates:
                    break

                for update in updates:
                    update_id = update.get("update_id", 0)
                    max_update_id = max(max_update_id, update_id)

                    message = (
                        update.get("message")
                        or update.get("channel_post")
                        or update.get("edited_message")
                        or update.get("edited_channel_post")
                    )
                    if not message:
                        continue

                    await ctx.increment_scanned()
                    chat = message.get("chat", {})
                    chat_id = chat.get("id")

                    if chat_id and chat_id not in seen_chats:
                        try:
                            chat_info = await client.get_chat(chat_id)
                            seen_chats[chat_id] = chat_info
                            content = generate_chat_content(chat_info)
                            cid = await ctx.content_storage.save(
                                content[:MAX_CONTENT_LENGTH], "text/plain"
                            )
                            await ctx.emit(
                                map_chat_to_document(
                                    chat_info, cid, None, allowed_users or None
                                )
                            )
                            docs_emitted += 1
                        except TelegramError as e:
                            logger.warning("Failed to get chat %s: %s", chat_id, e)
                            seen_chats[chat_id] = chat

                    chat_info = seen_chats.get(chat_id, chat)
                    msg_content = generate_message_content(message)
                    if msg_content and msg_content != "[Empty message]":
                        cid = await ctx.content_storage.save(
                            msg_content[:MAX_CONTENT_LENGTH], "text/plain"
                        )
                        await ctx.emit(
                            map_message_to_document(
                                message, cid, chat_info, bot_username, allowed_users or None
                            )
                        )
                        docs_emitted += 1

                    if docs_emitted % CHECKPOINT_INTERVAL == 0 and docs_emitted > 0:
                        await ctx.save_state({
                            "last_update_id": max_update_id,
                            "last_sync_at": datetime.now(timezone.utc).isoformat(),
                        })

                offset = max_update_id + 1

            await ctx.complete({
                "last_update_id": max_update_id,
                "last_sync_at": datetime.now(timezone.utc).isoformat(),
            })

        except AuthenticationError as e:
            await ctx.fail(f"Authentication failed: {e}")
        except TelegramError as e:
            await ctx.fail(f"Sync failed: {e}")
        finally:
            await client.close()


def _is_telethon(creds: dict[str, Any]) -> bool:
    return bool(creds.get("session") and creds.get("api_id") and creds.get("api_hash"))


def _build_message_text(msg: dict[str, Any]) -> str:
    """Build searchable text from a Telethon message dict."""
    parts: list[str] = []

    if msg.get("text"):
        parts.append(msg["text"])

    media_type = msg.get("media_type")
    if media_type == "photo":
        parts.append("[Photo]")
    elif media_type == "document":
        parts.append(f"[File: {msg.get('file_name', 'document')}]")
    elif media_type == "video":
        parts.append(f"[Video: {msg.get('file_name', 'video')}]")
    elif media_type in ("audio", "voice"):
        parts.append(f"[Audio: {msg.get('file_name', 'audio')}]")
    elif media_type == "sticker":
        parts.append(f"[Sticker: {msg.get('sticker_emoji', '')}]")
    elif media_type == "poll":
        parts.append(f"[Poll: {msg.get('poll_question', '')}]")

    if msg.get("forward"):
        parts.insert(0, "[Forwarded]")

    return "\n".join(parts) if parts else ""


def _to_bot_api_format(msg: dict[str, Any], chat: dict[str, Any]) -> dict[str, Any]:
    """Convert Telethon message dict to Bot API-like format for the mapper."""
    return {
        "message_id": msg["id"],
        "date": msg.get("date"),
        "text": msg.get("text", ""),
        "from": {"first_name": msg.get("sender_name", ""), "id": msg.get("sender_id")},
        "chat": {
            "id": chat["id"],
            "title": chat.get("title"),
            "type": chat.get("type", "unknown"),
            "username": chat.get("username"),
        },
        "reply_to_message": {"text": ""} if msg.get("reply_to_msg_id") else None,
    }

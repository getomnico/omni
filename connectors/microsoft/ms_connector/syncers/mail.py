"""Outlook Mail syncer using delta queries."""

import base64
import logging
from datetime import datetime, timedelta, timezone
from typing import Any

from omni_connector import SyncContext

from ..graph_client import GraphClient, GraphAPIError
from ..mappers import (
    map_message_to_document,
    map_attachment_to_document,
    generate_message_content,
    strip_html,
)
from .base import BaseSyncer, DEFAULT_MAX_AGE_DAYS
from .onedrive import _is_indexable, _get_extension

logger = logging.getLogger(__name__)

MAX_ATTACHMENT_SIZE = 10 * 1024 * 1024  # 10 MB
MAIL_FOLDERS = ["inbox", "sentitems", "archive"]


class MailSyncer(BaseSyncer):
    @property
    def name(self) -> str:
        return "mail"

    async def sync(
        self,
        client: GraphClient,
        ctx: SyncContext,
        state: dict[str, Any],
        source_config: dict[str, Any] | None = None,
        user_cache: dict[str, str] | None = None,
        group_cache: dict[str, str] | None = None,
    ) -> dict[str, Any]:
        """Run mail sync across all users and mail folders."""
        source_config = source_config or {}
        delta_tokens: dict[str, str] = state.get("delta_tokens", {})
        new_tokens: dict[str, str] = {}

        users = await client.list_users()
        logger.info("[mail] Syncing across %d users", len(users))

        users = [
            u
            for u in users
            if ctx.should_index_user(u.get("mail") or u.get("userPrincipalName") or "")
        ]
        logger.info("[mail] %d users after filtering", len(users))

        for user in users:
            if ctx.is_cancelled():
                logger.info("[mail] Cancelled")
                return state

            user_id = user["id"]

            for folder in MAIL_FOLDERS:
                if ctx.is_cancelled():
                    break

                token_key = f"{user_id}:{folder}"
                token = delta_tokens.get(token_key)

                new_token = await self._sync_folder_for_user(
                    client, user, ctx, folder, token
                )
                if new_token:
                    new_tokens[token_key] = new_token

        return {"delta_tokens": new_tokens}

    async def sync_for_user(
        self,
        client: GraphClient,
        user: dict[str, Any],
        ctx: SyncContext,
        delta_token: str | None,
        user_cache: dict[str, str] | None = None,
        group_cache: dict[str, str] | None = None,
    ) -> str | None:
        # Not used — sync() is overridden to handle multi-folder logic.
        # Kept to satisfy the abstract base class.
        return await self._sync_folder_for_user(client, user, ctx, "inbox", delta_token)

    async def _sync_folder_for_user(
        self,
        client: GraphClient,
        user: dict[str, Any],
        ctx: SyncContext,
        folder: str,
        delta_token: str | None,
    ) -> str | None:
        user_id = user["id"]
        display_name = user.get("displayName", user_id)
        logger.info("[mail] Syncing %s for user %s", folder, display_name)

        try:
            params: dict[str, str] = {
                "$select": "id,subject,bodyPreview,body,from,toRecipients,"
                "ccRecipients,receivedDateTime,sentDateTime,webLink,"
                "hasAttachments,internetMessageId"
            }
            if delta_token is None:
                cutoff = (
                    datetime.now(timezone.utc) - timedelta(days=DEFAULT_MAX_AGE_DAYS)
                ).strftime("%Y-%m-%dT%H:%M:%SZ")
                params["$filter"] = f"receivedDateTime ge {cutoff}"
            items, new_token = await client.get_delta(
                f"/users/{user_id}/mailFolders/{folder}/messages/delta",
                delta_token=delta_token,
                params=params,
            )
        except GraphAPIError as e:
            logger.warning(
                "[mail] Failed to fetch delta for %s/%s: %s",
                display_name,
                folder,
                e,
            )
            return delta_token

        skipped_deleted = 0

        for item in items:
            if ctx.is_cancelled():
                return delta_token

            if item.get("deleted") or item.get("@removed"):
                skipped_deleted += 1
                continue

            await ctx.increment_scanned()

            try:
                body_content = item.get("body", {}).get("content", "")
                body_type = item.get("body", {}).get("contentType", "text")
                if body_type.lower() == "html":
                    body_content = strip_html(body_content)

                content = generate_message_content(item, body_content)
                content_id = await ctx.content_storage.save(content, "text/plain")
                doc = map_message_to_document(
                    message=item,
                    content_id=content_id,
                )
                await ctx.emit(doc)
            except Exception as e:
                internet_msg_id = item.get("internetMessageId") or item.get(
                    "id", "unknown"
                )
                logger.warning("[mail] Error processing %s: %s", internet_msg_id, e)
                await ctx.emit_error(internet_msg_id, str(e))

            if item.get("hasAttachments"):
                await self._process_attachments(client, user_id, item, ctx)

        if skipped_deleted:
            logger.info(
                "[mail] %s/%s: %d items total, %d deleted skipped",
                display_name,
                folder,
                len(items),
                skipped_deleted,
            )

        return new_token

    async def _process_attachments(
        self,
        client: GraphClient,
        user_id: str,
        message: dict[str, Any],
        ctx: SyncContext,
    ) -> None:
        msg_id = message["id"]
        try:
            attachments = await client.list_message_attachments(user_id, msg_id)
        except Exception as e:
            logger.warning("[mail] Failed to fetch attachments for %s: %s", msg_id, e)
            return

        for att in attachments:
            att_id = att.get("id", "unknown")
            filename = att.get("name", "")
            size = att.get("size", 0)

            if size > MAX_ATTACHMENT_SIZE:
                logger.debug(
                    "[mail] Skipping large attachment %s (%d bytes)", filename, size
                )
                continue

            content_bytes_b64 = att.get("contentBytes")
            if not content_bytes_b64:
                continue

            try:
                raw_bytes = base64.b64decode(content_bytes_b64)
                mime_type = att.get("contentType", "application/octet-stream")
                extension = _get_extension(filename)

                if _is_indexable(mime_type, extension):
                    content_id = await ctx.content_storage.extract_and_store_content(
                        raw_bytes, mime_type, filename
                    )
                else:
                    content = (
                        f"Attachment: {filename}\nType: {mime_type}\nSize: {size} bytes"
                    )
                    content_id = await ctx.content_storage.save(content, "text/plain")

                doc = map_attachment_to_document(
                    attachment=att,
                    message=message,
                    content_id=content_id,
                )
                await ctx.emit(doc)
            except Exception as e:
                logger.warning(
                    "[mail] Error processing attachment %s on %s: %s",
                    att_id,
                    msg_id,
                    e,
                )
                internet_msg_id = message.get("internetMessageId") or msg_id
                await ctx.emit_error(f"mail:{internet_msg_id}:att:{att_id}", str(e))

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


class MailSyncer(BaseSyncer):
    @property
    def name(self) -> str:
        return "mail"

    async def sync_for_user(
        self,
        client: GraphClient,
        user: dict[str, Any],
        ctx: SyncContext,
        delta_token: str | None,
        user_cache: dict[str, str] | None = None,
        group_cache: dict[str, str] | None = None,
    ) -> str | None:
        user_id = user["id"]
        display_name = user.get("displayName", user_id)
        logger.info("[mail] Syncing inbox for user %s", display_name)

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
                f"/users/{user_id}/mailFolders/inbox/messages/delta",
                delta_token=delta_token,
                params=params,
            )
        except GraphAPIError as e:
            logger.warning(
                "[mail] Failed to fetch delta for user %s: %s", display_name, e
            )
            return delta_token

        for item in items:
            if ctx.is_cancelled():
                return delta_token

            await ctx.increment_scanned()

            # Skip deletions: a message deleted from one user's inbox
            # shouldn't disappear from search for all participants.
            if item.get("deleted") or item.get("@removed"):
                continue

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

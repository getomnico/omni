"""Outlook Mail syncer using delta queries."""

import logging
from datetime import datetime, timedelta, timezone
from typing import Any

from omni_connector import SyncContext

from ..graph_client import GraphClient, GraphAPIError
from ..mappers import map_message_to_document, generate_message_content, strip_html
from .base import BaseSyncer, DEFAULT_MAX_AGE_DAYS

logger = logging.getLogger(__name__)


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

        return new_token

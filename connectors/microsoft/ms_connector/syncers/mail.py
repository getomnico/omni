"""Outlook Mail syncer using delta queries."""

import logging
import re
from typing import Any

from omni_connector import SyncContext

from ..graph_client import GraphClient, GraphAPIError
from ..mappers import map_message_to_document, generate_message_content
from .base import BaseSyncer

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
    ) -> str | None:
        user_id = user["id"]
        display_name = user.get("displayName", user_id)
        logger.info("[mail] Syncing inbox for user %s", display_name)

        try:
            items, new_token = await client.get_delta(
                f"/users/{user_id}/mailFolders/inbox/messages/delta",
                delta_token=delta_token,
                params={
                    "$select": "id,subject,bodyPreview,body,from,toRecipients,"
                    "ccRecipients,receivedDateTime,sentDateTime,webLink,hasAttachments"
                },
            )
        except GraphAPIError as e:
            logger.warning(
                "[mail] Failed to fetch delta for user %s: %s", display_name, e
            )
            return delta_token

        user_email = user.get("mail") or user.get("userPrincipalName")

        for item in items:
            if ctx.is_cancelled():
                return delta_token

            await ctx.increment_scanned()

            if item.get("deleted") or item.get("@removed"):
                external_id = f"mail:{user_id}:{item['id']}"
                await ctx.emit_deleted(external_id)
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
                    user_id=user_id,
                    user_email=user_email,
                    content_id=content_id,
                )
                await ctx.emit(doc)
            except Exception as e:
                external_id = f"mail:{user_id}:{item.get('id', 'unknown')}"
                logger.warning("[mail] Error processing %s: %s", external_id, e)
                await ctx.emit_error(external_id, str(e))

        return new_token


_HTML_TAG_RE = re.compile(r"<[^>]+>")
_WHITESPACE_RE = re.compile(r"\s+")


def strip_html(html: str) -> str:
    """Naive HTML tag stripping for email bodies."""
    text = _HTML_TAG_RE.sub(" ", html)
    text = _WHITESPACE_RE.sub(" ", text)
    return text.strip()

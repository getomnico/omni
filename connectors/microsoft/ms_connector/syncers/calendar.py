"""Outlook Calendar syncer using delta queries on calendarView."""

import logging
from datetime import datetime, timedelta, timezone
from typing import Any

from omni_connector import SyncContext

from ..graph_client import GraphClient, GraphAPIError
from ..mappers import map_event_to_document, generate_event_content
from .base import BaseSyncer, DEFAULT_MAX_AGE_DAYS

logger = logging.getLogger(__name__)

DEFAULT_FUTURE_MONTHS = 6


class CalendarSyncer(BaseSyncer):

    @property
    def name(self) -> str:
        return "calendar"

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
        owner_email = (
            user.get("mail") or user.get("userPrincipalName") or ""
        ).lower() or None
        logger.info("[calendar] Syncing events for user %s", display_name)

        now = datetime.now(timezone.utc)
        start = (now - timedelta(days=DEFAULT_MAX_AGE_DAYS)).isoformat()
        end = (now + timedelta(days=30 * DEFAULT_FUTURE_MONTHS)).isoformat()

        try:
            items, new_token = await client.get_delta(
                f"/users/{user_id}/calendarView/delta",
                delta_token=delta_token,
                params={
                    "startDateTime": start,
                    "endDateTime": end,
                    "$select": "id,subject,body,start,end,location,organizer,"
                    "attendees,webLink,isAllDay,isCancelled",
                },
            )
        except GraphAPIError as e:
            logger.warning(
                "[calendar] Failed to fetch delta for user %s: %s", display_name, e
            )
            return delta_token

        skipped_deleted = 0

        for item in items:
            if ctx.is_cancelled():
                return delta_token

            if item.get("deleted") or item.get("@removed"):
                skipped_deleted += 1
                external_id = f"calendar:{user_id}:{item['id']}"
                await ctx.emit_deleted(external_id)
                continue

            await ctx.increment_scanned()

            try:
                content = generate_event_content(item)
                content_id = await ctx.content_storage.save(content, "text/plain")
                doc = map_event_to_document(
                    event=item,
                    user_id=user_id,
                    content_id=content_id,
                    owner_email=owner_email,
                )
                await ctx.emit(doc)
            except Exception as e:
                external_id = f"calendar:{user_id}:{item.get('id', 'unknown')}"
                logger.warning("[calendar] Error processing %s: %s", external_id, e)
                await ctx.emit_error(external_id, str(e))

        if skipped_deleted:
            logger.info(
                "[calendar] User %s: %d items total, %d deleted skipped",
                display_name,
                len(items),
                skipped_deleted,
            )

        return new_token

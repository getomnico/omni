"""Outlook Calendar syncer using delta queries on calendarView."""

import logging
from datetime import datetime, timedelta, timezone
from typing import Any

from omni_connector import SyncContext

from ..graph_client import GraphClient, GraphAPIError
from ..mappers import map_event_to_document, generate_event_content
from .base import BaseSyncer

logger = logging.getLogger(__name__)

DEFAULT_PAST_MONTHS = 6
DEFAULT_FUTURE_MONTHS = 6


class CalendarSyncer(BaseSyncer):
    def __init__(self, source_config: dict[str, Any] | None = None):
        config = source_config or {}
        self._past_months = config.get("calendar_past_months", DEFAULT_PAST_MONTHS)
        self._future_months = config.get(
            "calendar_future_months", DEFAULT_FUTURE_MONTHS
        )

    @property
    def name(self) -> str:
        return "calendar"

    async def sync_for_user(
        self,
        client: GraphClient,
        user: dict[str, Any],
        ctx: SyncContext,
        delta_token: str | None,
    ) -> str | None:
        user_id = user["id"]
        display_name = user.get("displayName", user_id)
        logger.info("[calendar] Syncing events for user %s", display_name)

        now = datetime.now(timezone.utc)
        start = (now - timedelta(days=30 * self._past_months)).isoformat()
        end = (now + timedelta(days=30 * self._future_months)).isoformat()

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

        for item in items:
            if ctx.is_cancelled():
                return delta_token

            await ctx.increment_scanned()

            if item.get("deleted") or item.get("@removed"):
                external_id = f"calendar:{user_id}:{item['id']}"
                await ctx.emit_deleted(external_id)
                continue

            try:
                content = generate_event_content(item)
                content_id = await ctx.content_storage.save(content, "text/plain")
                doc = map_event_to_document(
                    event=item,
                    user_id=user_id,
                    content_id=content_id,
                )
                await ctx.emit(doc)
            except Exception as e:
                external_id = f"calendar:{user_id}:{item.get('id', 'unknown')}"
                logger.warning("[calendar] Error processing %s: %s", external_id, e)
                await ctx.emit_error(external_id, str(e))

        return new_token

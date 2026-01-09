#!/usr/bin/env python3
"""
Example RSS Feed Connector for Omni.

This connector demonstrates how to use the omni-connector SDK to build
a custom connector that syncs RSS feed articles into Omni.

Usage:
    pip install omni-connector feedparser
    CONNECTOR_MANAGER_URL=http://localhost:8080 python rss_connector.py

The connector expects source config with:
    {
        "feed_url": "https://example.com/feed.xml"
    }
"""

import hashlib
import logging
import os
from datetime import datetime, timezone
from typing import Any

try:
    import feedparser
except ImportError:
    print("Please install feedparser: pip install feedparser")
    raise

from omni_connector import (
    ActionDefinition,
    ActionParameter,
    ActionResponse,
    Connector,
    Document,
    DocumentMetadata,
    DocumentPermissions,
    SyncContext,
)

logging.basicConfig(level=logging.INFO)
logger = logging.getLogger(__name__)


class RSSConnector(Connector):
    """RSS Feed connector that syncs articles from RSS/Atom feeds."""

    @property
    def name(self) -> str:
        return "rss"

    @property
    def version(self) -> str:
        return "1.0.0"

    @property
    def sync_modes(self) -> list[str]:
        return ["full", "incremental"]

    @property
    def actions(self) -> list[ActionDefinition]:
        return [
            ActionDefinition(
                name="validate_feed",
                description="Validate that an RSS feed URL is accessible and parseable",
                parameters={
                    "feed_url": ActionParameter(
                        type="string",
                        required=True,
                        description="The RSS feed URL to validate",
                    ),
                },
            ),
        ]

    async def sync(
        self,
        source_config: dict[str, Any],
        credentials: dict[str, Any],
        state: dict[str, Any] | None,
        ctx: SyncContext,
    ) -> None:
        """Sync articles from an RSS feed."""
        feed_url = source_config.get("feed_url")
        if not feed_url:
            await ctx.fail("Missing feed_url in source config")
            return

        last_sync_time = None
        if state:
            last_sync_str = state.get("last_sync_time")
            if last_sync_str:
                last_sync_time = datetime.fromisoformat(last_sync_str)

        logger.info("Fetching RSS feed: %s", feed_url)

        try:
            feed = feedparser.parse(feed_url)
        except Exception as e:
            await ctx.fail(f"Failed to parse feed: {e}")
            return

        if feed.bozo and feed.bozo_exception:
            logger.warning("Feed parsing warning: %s", feed.bozo_exception)

        docs_since_checkpoint = 0
        current_time = datetime.now(timezone.utc)

        for entry in feed.entries:
            if ctx.is_cancelled():
                await ctx.fail("Cancelled by user")
                return

            await ctx.increment_scanned()

            published = self._parse_entry_date(entry)
            if last_sync_time and published and published <= last_sync_time:
                continue

            entry_id = entry.get("id") or entry.get("link") or self._hash_entry(entry)
            if not entry_id:
                await ctx.emit_error("unknown", "Entry has no id or link")
                continue

            content = self._extract_content(entry)
            if not content:
                content = entry.get("summary", entry.get("title", ""))

            try:
                content_id = await ctx.content_storage.save(content, "text/html")
            except Exception as e:
                await ctx.emit_error(entry_id, f"Failed to store content: {e}")
                continue

            doc = Document(
                external_id=entry_id,
                title=entry.get("title", "Untitled"),
                content_id=content_id,
                metadata=DocumentMetadata(
                    author=self._get_author(entry),
                    created_at=published,
                    updated_at=published,
                    url=entry.get("link"),
                    mime_type="text/html",
                    extra={
                        "feed_title": feed.feed.get("title"),
                        "feed_url": feed_url,
                    },
                ),
                permissions=DocumentPermissions(public=True),
                attributes={
                    "source_type": "rss",
                    "feed_url": feed_url,
                },
            )

            await ctx.emit(doc)
            docs_since_checkpoint += 1

            if docs_since_checkpoint >= 50:
                await ctx.save_state({"last_sync_time": current_time.isoformat()})
                docs_since_checkpoint = 0

        await ctx.complete(new_state={"last_sync_time": current_time.isoformat()})
        logger.info(
            "Sync completed: %d scanned, %d emitted",
            ctx.documents_scanned,
            ctx.documents_emitted,
        )

    async def execute_action(
        self,
        action: str,
        params: dict[str, Any],
        credentials: dict[str, Any],
    ) -> ActionResponse:
        if action == "validate_feed":
            feed_url = params.get("feed_url")
            if not feed_url:
                return ActionResponse.failure("Missing feed_url parameter")

            try:
                feed = feedparser.parse(feed_url)
                if feed.bozo and feed.bozo_exception:
                    return ActionResponse.failure(
                        f"Feed parsing error: {feed.bozo_exception}"
                    )

                return ActionResponse.success(
                    {
                        "valid": True,
                        "title": feed.feed.get("title", "Unknown"),
                        "entry_count": len(feed.entries),
                    }
                )
            except Exception as e:
                return ActionResponse.failure(f"Failed to fetch feed: {e}")

        return ActionResponse.not_supported(action)

    def _parse_entry_date(self, entry: Any) -> datetime | None:
        """Parse the published date from an RSS entry."""
        for field in ["published_parsed", "updated_parsed", "created_parsed"]:
            parsed = entry.get(field)
            if parsed:
                try:
                    return datetime(*parsed[:6], tzinfo=timezone.utc)
                except (TypeError, ValueError):
                    continue
        return None

    def _extract_content(self, entry: Any) -> str | None:
        """Extract the main content from an RSS entry."""
        content = entry.get("content")
        if content and isinstance(content, list) and len(content) > 0:
            return content[0].get("value")
        return entry.get("summary")

    def _get_author(self, entry: Any) -> str | None:
        """Get the author from an RSS entry."""
        author = entry.get("author")
        if author:
            return author
        author_detail = entry.get("author_detail")
        if author_detail:
            return author_detail.get("name")
        return None

    def _hash_entry(self, entry: Any) -> str:
        """Generate a hash ID for an entry without an explicit ID."""
        content = f"{entry.get('title', '')}{entry.get('summary', '')}"
        return hashlib.sha256(content.encode()).hexdigest()[:16]


if __name__ == "__main__":
    port = int(os.environ.get("PORT", "8000"))
    RSSConnector().serve(port=port)

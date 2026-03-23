"""Main NotionConnector class."""

import logging
from datetime import datetime, timezone
from typing import Any

from omni_connector import Connector, SyncContext

from .client import AuthenticationError, NotionClient, NotionError
from .config import CHECKPOINT_INTERVAL, RATE_LIMIT_DELAY
from .mappers import (
    generate_database_content,
    generate_page_content,
    map_database_to_document,
    map_page_to_document,
)

logger = logging.getLogger(__name__)


class NotionConnector(Connector):
    """Notion connector for Omni."""

    @property
    def name(self) -> str:
        return "notion"

    @property
    def version(self) -> str:
        return "1.0.0"

    @property
    def source_types(self) -> list[str]:
        return ["notion"]

    @property
    def description(self) -> str:
        return "Connect to Notion pages and databases"

    @property
    def sync_modes(self) -> list[str]:
        return ["full", "incremental"]

    async def sync(
        self,
        source_config: dict[str, Any],
        credentials: dict[str, Any],
        state: dict[str, Any] | None,
        ctx: SyncContext,
    ) -> None:
        token = credentials.get("token")
        if not token:
            await ctx.fail("Missing 'token' in credentials")
            return

        api_url = source_config.get("api_url")
        client = NotionClient(
            token=token,
            base_url=api_url,
            rate_limit_delay=0 if api_url else RATE_LIMIT_DELAY,
        )

        try:
            bot_user = await client.validate_token()
        except AuthenticationError as e:
            await ctx.fail(f"Authentication failed: {e}")
            return
        except NotionError as e:
            await ctx.fail(f"Connection test failed: {e}")
            return

        bot_name = bot_user.get("name", "Unknown")
        logger.info("Starting Notion sync as bot '%s'", bot_name)

        workspace_name = bot_user.get("bot", {}).get(
            "workspace_name", "Notion Workspace"
        )
        permission_group = f"notion:workspace:{ctx.source_id}"

        await self._sync_group_memberships(
            client, permission_group, workspace_name, ctx
        )

        state = state or {}

        try:
            if state.get("last_sync_at"):
                await self._incremental_sync(client, state, permission_group, ctx)
            else:
                await self._full_sync(client, permission_group, ctx)
        except AuthenticationError as e:
            logger.error("Authentication error during sync: %s", e)
            await ctx.fail(f"Authentication failed: {e}")
        except Exception as e:
            logger.exception("Sync failed with unexpected error")
            await ctx.fail(str(e))
        finally:
            await client.close()

    async def _full_sync(
        self,
        client: NotionClient,
        permission_group: str,
        ctx: SyncContext,
    ) -> None:
        """Full sync: index all accessible databases and pages."""
        docs_emitted = 0
        database_page_ids: set[str] = set()

        # Phase 1: discover and index all databases + their entries
        cursor: str | None = None
        while True:
            if ctx.is_cancelled():
                await ctx.fail("Cancelled by user")
                return

            response = await client.search_databases(start_cursor=cursor)
            databases = response.get("results", [])

            for db in databases:
                if ctx.is_cancelled():
                    await ctx.fail("Cancelled by user")
                    return

                await ctx.increment_scanned()
                db_id = db["id"]

                try:
                    docs_emitted = await self._sync_database(
                        client, db, permission_group, ctx, docs_emitted
                    )
                    entry_ids = await self._sync_database_entries(
                        client, db_id, permission_group, ctx, docs_emitted
                    )
                    database_page_ids.update(entry_ids[0])
                    docs_emitted = entry_ids[1]
                except NotionError as e:
                    logger.error("Error syncing database %s: %s", db_id, e)
                    await ctx.emit_error(f"notion:database:{db_id}", str(e))

            if not response.get("has_more"):
                break
            cursor = response.get("next_cursor")

        # Phase 2: index standalone pages (not in any database)
        cursor = None
        while True:
            if ctx.is_cancelled():
                await ctx.fail("Cancelled by user")
                return

            response = await client.search_pages(start_cursor=cursor)
            pages = response.get("results", [])

            for page in pages:
                if ctx.is_cancelled():
                    await ctx.fail("Cancelled by user")
                    return

                page_id = page["id"]
                if page_id in database_page_ids:
                    continue

                await ctx.increment_scanned()
                try:
                    docs_emitted = await self._sync_page(
                        client,
                        page,
                        permission_group,
                        ctx,
                        docs_emitted,
                        is_database_entry=False,
                    )
                except Exception as e:
                    eid = f"notion:page:{page_id}"
                    logger.warning("Error processing %s: %s", eid, e)
                    await ctx.emit_error(eid, str(e))

                if docs_emitted >= CHECKPOINT_INTERVAL:
                    await ctx.save_state({})
                    docs_emitted = 0

            if not response.get("has_more"):
                break
            cursor = response.get("next_cursor")

        now = datetime.now(timezone.utc).isoformat()
        await ctx.complete(new_state={"last_sync_at": now})
        logger.info(
            "Full sync completed: %d scanned, %d emitted",
            ctx.documents_scanned,
            ctx.documents_emitted,
        )

    async def _incremental_sync(
        self,
        client: NotionClient,
        state: dict[str, Any],
        permission_group: str,
        ctx: SyncContext,
    ) -> None:
        """Incremental sync: re-index pages/databases modified since last sync."""
        last_sync_at = state["last_sync_at"]
        docs_emitted = 0

        # Search pages sorted by last_edited_time (most recent first)
        cursor: str | None = None
        while True:
            if ctx.is_cancelled():
                await ctx.fail("Cancelled by user")
                return

            response = await client.search_pages(start_cursor=cursor)
            pages = response.get("results", [])

            found_old = False
            for page in pages:
                if ctx.is_cancelled():
                    await ctx.fail("Cancelled by user")
                    return

                edited_time = page.get("last_edited_time", "")
                if edited_time and edited_time < last_sync_at:
                    found_old = True
                    break

                await ctx.increment_scanned()
                page_id = page["id"]
                is_db_entry = page.get("parent", {}).get("type") == "database_id"

                try:
                    docs_emitted = await self._sync_page(
                        client,
                        page,
                        permission_group,
                        ctx,
                        docs_emitted,
                        is_database_entry=is_db_entry,
                    )
                except Exception as e:
                    eid = f"notion:page:{page_id}"
                    logger.warning("Error processing %s: %s", eid, e)
                    await ctx.emit_error(eid, str(e))

                if docs_emitted >= CHECKPOINT_INTERVAL:
                    await ctx.save_state({"last_sync_at": last_sync_at})
                    docs_emitted = 0

            if found_old or not response.get("has_more"):
                break
            cursor = response.get("next_cursor")

        # Also check for modified databases
        cursor = None
        while True:
            if ctx.is_cancelled():
                await ctx.fail("Cancelled by user")
                return

            response = await client.search_databases(start_cursor=cursor)
            databases = response.get("results", [])

            found_old = False
            for db in databases:
                edited_time = db.get("last_edited_time", "")
                if edited_time and edited_time < last_sync_at:
                    found_old = True
                    break

                await ctx.increment_scanned()
                try:
                    docs_emitted = await self._sync_database(
                        client, db, permission_group, ctx, docs_emitted
                    )
                except Exception as e:
                    db_id = db["id"]
                    logger.warning("Error processing database %s: %s", db_id, e)
                    await ctx.emit_error(f"notion:database:{db_id}", str(e))

            if found_old or not response.get("has_more"):
                break
            cursor = response.get("next_cursor")

        now = datetime.now(timezone.utc).isoformat()
        await ctx.complete(new_state={"last_sync_at": now})
        logger.info(
            "Incremental sync completed: %d scanned, %d emitted",
            ctx.documents_scanned,
            ctx.documents_emitted,
        )

    async def _sync_group_memberships(
        self,
        client: NotionClient,
        permission_group: str,
        workspace_name: str,
        ctx: SyncContext,
    ) -> None:
        """Emit a workspace-level group membership event with all workspace members."""
        users = await client.list_users()
        member_emails: list[str] = []

        for user in users:
            if user.get("type") != "person":
                continue
            person = user.get("person", {})
            email = person.get("email")
            if not email:
                logger.warning(
                    "Workspace member %s (id=%s) has no email, skipping",
                    user.get("name", "unknown"),
                    user.get("id"),
                )
                continue
            member_emails.append(email.lower())

        if member_emails:
            await ctx.emit_group_membership(
                group_email=permission_group,
                member_emails=member_emails,
                group_name=workspace_name,
            )

        logger.info("Emitted workspace group with %d members", len(member_emails))

    async def _sync_database(
        self,
        client: NotionClient,
        database: dict[str, Any],
        permission_group: str,
        ctx: SyncContext,
        docs_emitted: int,
    ) -> int:
        """Emit a document for the database itself. Returns updated docs_emitted."""
        content = generate_database_content(database)
        content_id = await ctx.content_storage.save(content, "text/plain")
        doc = map_database_to_document(database, content_id, permission_group)
        await ctx.emit(doc)
        docs_emitted += 1
        return docs_emitted

    async def _sync_database_entries(
        self,
        client: NotionClient,
        database_id: str,
        permission_group: str,
        ctx: SyncContext,
        docs_emitted: int,
    ) -> tuple[set[str], int]:
        """Sync all pages within a database. Returns (page_ids, docs_emitted)."""
        page_ids: set[str] = set()
        cursor: str | None = None

        while True:
            if ctx.is_cancelled():
                break

            response = await client.query_database(database_id, start_cursor=cursor)
            pages = response.get("results", [])

            for page in pages:
                if ctx.is_cancelled():
                    break

                page_id = page["id"]
                page_ids.add(page_id)
                await ctx.increment_scanned()

                try:
                    docs_emitted = await self._sync_page(
                        client,
                        page,
                        permission_group,
                        ctx,
                        docs_emitted,
                        is_database_entry=True,
                    )
                except Exception as e:
                    eid = f"notion:page:{page_id}"
                    logger.warning("Error processing %s: %s", eid, e)
                    await ctx.emit_error(eid, str(e))

                if docs_emitted >= CHECKPOINT_INTERVAL:
                    await ctx.save_state({})
                    docs_emitted = 0

            if not response.get("has_more"):
                break
            cursor = response.get("next_cursor")

        return page_ids, docs_emitted

    async def _sync_page(
        self,
        client: NotionClient,
        page: dict[str, Any],
        permission_group: str,
        ctx: SyncContext,
        docs_emitted: int,
        is_database_entry: bool,
    ) -> int:
        """Fetch blocks for a page, generate content, and emit document."""
        page_id = page["id"]
        blocks = await client.get_all_blocks(page_id)
        properties = page.get("properties") if is_database_entry else None
        content = generate_page_content(page, blocks, properties)
        content_id = await ctx.content_storage.save(content, "text/plain")
        doc = map_page_to_document(
            page,
            content_id,
            permission_group,
            is_database_entry=is_database_entry,
        )
        await ctx.emit(doc)
        docs_emitted += 1
        return docs_emitted

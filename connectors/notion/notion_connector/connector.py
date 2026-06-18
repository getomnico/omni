"""Main NotionConnector class."""

import csv
import io
import logging
from datetime import UTC, datetime
from typing import Any

from fastapi.responses import JSONResponse, Response
from omni_connector import ActionDefinition, ActionResponse, Connector, SyncContext, SyncMode

from .client import AuthenticationError, ForbiddenError, NotionClient, NotionError
from .config import CHECKPOINT_INTERVAL, RATE_LIMIT_DELAY
from .mappers import (
    extract_property_display_value,
    generate_data_source_content,
    generate_page_content,
    map_data_source_to_document,
    map_page_to_document,
    render_blocks_to_text,
)

logger = logging.getLogger(__name__)


def _extract_token(credentials: dict[str, Any]) -> str | None:
    """Accept both raw connector creds and connector-manager ServiceCredential envelopes."""
    token = credentials.get("token")
    if isinstance(token, str) and token:
        return token

    payload = credentials.get("credentials")
    if isinstance(payload, dict):
        token = payload.get("token")
        if isinstance(token, str) and token:
            return token

    return None


class NotionConnector(Connector):
    """Notion connector for Omni."""

    @property
    def name(self) -> str:
        return "notion"

    @property
    def display_name(self) -> str:
        return "Notion"

    @property
    def version(self) -> str:
        return "1.0.0"

    @property
    def source_types(self) -> list[str]:
        return ["notion"]

    @property
    def description(self) -> str:
        return "Index pages and databases from a Notion workspace"

    @property
    def sync_modes(self) -> list[str]:
        return ["full", "incremental"]

    @property
    def actions(self) -> list[ActionDefinition]:
        return [
            ActionDefinition(
                name="export_data_source_csv",
                description=(
                    "Export a Notion database/data source as a CSV file. "
                    "Each row includes omni_external_id so agents can fetch "
                    "the indexed Omni document for that Notion row."
                ),
                input_schema={
                    "type": "object",
                    "properties": {
                        "data_source_id": {
                            "type": "string",
                            "description": "The Notion data source ID to export.",
                        },
                        "database_id": {
                            "type": "string",
                            "description": (
                                "Alias for data_source_id when the caller has "
                                "a database/data-source ID."
                            ),
                        },
                        "include_content": {
                            "type": "boolean",
                            "description": (
                                "When true, include rendered row page body text "
                                "in the notion_content column."
                            ),
                            "default": False,
                        },
                    },
                },
                mode="read",
                source_types=["notion"],
            )
        ]

    async def sync(
        self,
        source_config: dict[str, Any],
        credentials: dict[str, Any],
        checkpoint: dict[str, Any] | None,
        ctx: SyncContext,
    ) -> None:
        token = _extract_token(credentials)
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

        bot_name = bot_user.get("name") or "Unknown"
        logger.info("Starting Notion sync as bot '%s'", bot_name)

        # Notion can return workspace_name as an explicit null, so dict.get's
        # default doesn't fire — normalize via `or`.
        workspace_name = (
            bot_user.get("bot", {}).get("workspace_name") or "Notion Workspace"
        )
        permission_group = f"notion:workspace:{ctx.source_id}"

        await self._sync_group_memberships(
            client, permission_group, workspace_name, ctx
        )

        checkpoint = checkpoint or {}

        try:
            if ctx.sync_mode == SyncMode.INCREMENTAL and checkpoint.get("last_sync_at"):
                await self._incremental_sync(client, checkpoint, permission_group, ctx)
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

    async def execute_action(
        self,
        action: str,
        params: dict[str, Any],
        credentials: dict[str, Any],
    ) -> JSONResponse | Response:
        if action != "export_data_source_csv":
            return ActionResponse.not_supported(action).to_response(status_code=404)

        token = _extract_token(credentials)
        if not token:
            return ActionResponse.failure("Missing 'token' in credentials").to_response(
                status_code=400
            )

        data_source_id = params.get("data_source_id") or params.get("database_id")
        if not data_source_id:
            return ActionResponse.failure(
                "Missing data_source_id or database_id parameter"
            ).to_response(status_code=400)

        api_url = params.get("api_url")
        client = NotionClient(
            token=token,
            base_url=api_url,
            rate_limit_delay=0 if api_url else RATE_LIMIT_DELAY,
        )

        try:
            csv_text = await self._export_data_source_csv(
                client,
                data_source_id=data_source_id,
                include_content=bool(params.get("include_content", False)),
            )
        except AuthenticationError as e:
            return ActionResponse.failure(f"Authentication failed: {e}").to_response(
                status_code=401
            )
        except NotionError as e:
            return ActionResponse.failure(str(e)).to_response(status_code=500)
        finally:
            await client.close()

        filename = f"notion-data-source-{data_source_id[:8]}.csv"
        return Response(
            content=csv_text,
            media_type="text/csv; charset=utf-8",
            headers={
                "content-disposition": f'attachment; filename="{filename}"',
                "x-file-name": filename,
            },
        )

    async def _full_sync(
        self,
        client: NotionClient,
        permission_group: str,
        ctx: SyncContext,
    ) -> None:
        """Full sync: index all accessible data sources and pages."""
        sync_started_at = datetime.now(UTC).isoformat()
        docs_emitted = 0
        data_source_entry_ids: set[str] = set()

        # Phase 1: discover and index all data sources + their entries
        cursor: str | None = None
        while True:
            if ctx.is_cancelled():
                await ctx.fail("Cancelled by user")
                return

            response = await client.search_data_sources(start_cursor=cursor)
            data_sources = response.get("results", [])

            for ds in data_sources:
                if ctx.is_cancelled():
                    await ctx.fail("Cancelled by user")
                    return

                await ctx.increment_scanned()
                ds_id = ds["id"]

                try:
                    docs_emitted = await self._sync_data_source(
                        client, ds, permission_group, ctx, docs_emitted
                    )
                    entries, docs_emitted = await self._sync_data_source_entries(
                        client, ds_id, permission_group, ctx, docs_emitted
                    )
                    data_source_entry_ids.update(entries)

                    if docs_emitted >= CHECKPOINT_INTERVAL:
                        await ctx.save_checkpoint(ctx.checkpoint)
                        docs_emitted = 0
                except NotionError as e:
                    logger.error("Error syncing data source %s: %s", ds_id, e)
                    await ctx.emit_error(f"notion:data_source:{ds_id}", str(e))

            if not response.get("has_more"):
                break
            cursor = response.get("next_cursor")

        # Phase 2: index standalone pages (not part of any data source)
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
                if page_id in data_source_entry_ids:
                    continue

                await ctx.increment_scanned()
                try:
                    docs_emitted = await self._sync_page(
                        client,
                        page,
                        permission_group,
                        ctx,
                        docs_emitted,
                        is_data_source_entry=False,
                    )
                except Exception as e:
                    eid = f"notion:page:{page_id}"
                    logger.warning("Error processing %s: %s", eid, e)
                    await ctx.emit_error(eid, str(e))

                if docs_emitted >= CHECKPOINT_INTERVAL:
                    await ctx.save_checkpoint(ctx.checkpoint)
                    docs_emitted = 0

            if not response.get("has_more"):
                break
            cursor = response.get("next_cursor")

        await ctx.complete(checkpoint={"last_sync_at": sync_started_at})
        logger.info(
            "Full sync completed: %d scanned, %d emitted",
            ctx.documents_scanned,
            ctx.documents_emitted,
        )

    async def _incremental_sync(
        self,
        client: NotionClient,
        checkpoint: dict[str, Any],
        permission_group: str,
        ctx: SyncContext,
    ) -> None:
        """Incremental sync: re-index pages/data sources modified since last sync."""
        last_sync_at = checkpoint["last_sync_at"]
        sync_started_at = datetime.now(UTC).isoformat()
        docs_emitted = 0

        # Pages: search returns most-recently-edited first; stop once we cross
        # the cutoff. Database entries are also pages and surface here too.
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
                parent_type = page.get("parent", {}).get("type")
                is_ds_entry = parent_type in ("data_source_id", "database_id")

                try:
                    docs_emitted = await self._sync_page(
                        client,
                        page,
                        permission_group,
                        ctx,
                        docs_emitted,
                        is_data_source_entry=is_ds_entry,
                    )
                except Exception as e:
                    eid = f"notion:page:{page_id}"
                    logger.warning("Error processing %s: %s", eid, e)
                    await ctx.emit_error(eid, str(e))

                if docs_emitted >= CHECKPOINT_INTERVAL:
                    await ctx.save_checkpoint({"last_sync_at": last_sync_at})
                    docs_emitted = 0

            if found_old or not response.get("has_more"):
                break
            cursor = response.get("next_cursor")

        # Data sources: same early-break logic, sorted desc.
        cursor = None
        while True:
            if ctx.is_cancelled():
                await ctx.fail("Cancelled by user")
                return

            response = await client.search_data_sources(start_cursor=cursor)
            data_sources = response.get("results", [])

            found_old = False
            for ds in data_sources:
                edited_time = ds.get("last_edited_time", "")
                if edited_time and edited_time < last_sync_at:
                    found_old = True
                    break

                await ctx.increment_scanned()
                try:
                    docs_emitted = await self._sync_data_source(
                        client, ds, permission_group, ctx, docs_emitted
                    )
                except Exception as e:
                    ds_id = ds["id"]
                    logger.warning("Error processing data source %s: %s", ds_id, e)
                    await ctx.emit_error(f"notion:data_source:{ds_id}", str(e))

                if docs_emitted >= CHECKPOINT_INTERVAL:
                    await ctx.save_checkpoint({"last_sync_at": last_sync_at})
                    docs_emitted = 0

            if found_old or not response.get("has_more"):
                break
            cursor = response.get("next_cursor")

        await ctx.complete(checkpoint={"last_sync_at": sync_started_at})
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
        """Emit a workspace-level group membership event with all workspace members.

        If the integration is missing the "Read user information" capability,
        Notion returns 403 from /v1/users. We log a warning and skip emitting
        memberships rather than failing the entire sync — documents can still
        be indexed without group resolution, and the admin can grant the
        capability and re-sync later.
        """
        try:
            users = await client.list_users()
        except ForbiddenError as e:
            logger.warning(
                "Skipping workspace membership sync: %s. "
                "Under the integration's User capabilities, select "
                "'User information with email addresses' and re-sync to "
                "populate the workspace group.",
                e,
            )
            return

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

    async def _sync_data_source(
        self,
        client: NotionClient,
        data_source: dict[str, Any],
        permission_group: str,
        ctx: SyncContext,
        docs_emitted: int,
    ) -> int:
        """Emit a document for the data source itself. Returns updated docs_emitted."""
        content = generate_data_source_content(data_source)
        content_id = await ctx.content_storage.save(content, "text/markdown")
        doc = map_data_source_to_document(data_source, content_id, permission_group)
        await ctx.emit(doc)
        docs_emitted += 1
        return docs_emitted

    async def _sync_data_source_entries(
        self,
        client: NotionClient,
        data_source_id: str,
        permission_group: str,
        ctx: SyncContext,
        docs_emitted: int,
    ) -> tuple[set[str], int]:
        """Sync all pages within a data source. Returns (page_ids, docs_emitted)."""
        page_ids: set[str] = set()
        cursor: str | None = None

        while True:
            if ctx.is_cancelled():
                break

            response = await client.query_data_source(
                data_source_id, start_cursor=cursor
            )
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
                        is_data_source_entry=True,
                    )
                except Exception as e:
                    eid = f"notion:page:{page_id}"
                    logger.warning("Error processing %s: %s", eid, e)
                    await ctx.emit_error(eid, str(e))

                if docs_emitted >= CHECKPOINT_INTERVAL:
                    await ctx.save_checkpoint(ctx.checkpoint)
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
        is_data_source_entry: bool,
    ) -> int:
        """Fetch blocks for a page, generate content, and emit document."""
        page_id = page["id"]
        blocks = await client.get_all_blocks(page_id)
        properties = page.get("properties") if is_data_source_entry else None
        content = generate_page_content(page, blocks, properties)
        content_id = await ctx.content_storage.save(content, "text/markdown")
        doc = map_page_to_document(
            page,
            content_id,
            permission_group,
            is_data_source_entry=is_data_source_entry,
        )
        await ctx.emit(doc)
        docs_emitted += 1
        return docs_emitted

    async def _export_data_source_csv(
        self,
        client: NotionClient,
        data_source_id: str,
        include_content: bool = False,
    ) -> str:
        """Export all pages in a data source to CSV."""
        try:
            data_source = await client.retrieve_data_source(data_source_id)
            schema_property_names = list((data_source.get("properties") or {}).keys())
        except NotionError:
            logger.warning(
                "Could not retrieve data source schema for %s; "
                "falling back to row property discovery",
                data_source_id,
            )
            schema_property_names = []

        rows: list[dict[str, Any]] = []
        discovered_property_names: list[str] = []
        cursor: str | None = None

        while True:
            response = await client.query_data_source(
                data_source_id,
                start_cursor=cursor,
            )
            pages = response.get("results", [])
            for page in pages:
                properties = page.get("properties") or {}
                for name in properties:
                    if (
                        name not in schema_property_names
                        and name not in discovered_property_names
                    ):
                        discovered_property_names.append(name)

                row = self._data_source_page_to_csv_row(page, properties)
                if include_content:
                    blocks = await client.get_all_blocks(page["id"])
                    row["notion_content"] = render_blocks_to_text(blocks)
                rows.append(row)

            if not response.get("has_more"):
                break
            cursor = response.get("next_cursor")

        property_names = schema_property_names + discovered_property_names
        fieldnames = [
            "omni_external_id",
            "notion_page_id",
            "notion_url",
            "notion_created_time",
            "notion_last_edited_time",
            *property_names,
        ]
        if include_content:
            fieldnames.append("notion_content")

        output = io.StringIO()
        writer = csv.DictWriter(output, fieldnames=fieldnames, extrasaction="ignore")
        writer.writeheader()
        for row in rows:
            writer.writerow(row)
        return output.getvalue()

    def _data_source_page_to_csv_row(
        self,
        page: dict[str, Any],
        properties: dict[str, Any],
    ) -> dict[str, Any]:
        page_id = page["id"]
        row: dict[str, Any] = {
            "omni_external_id": f"notion:page:{page_id}",
            "notion_page_id": page_id,
            "notion_url": page.get("url", ""),
            "notion_created_time": page.get("created_time", ""),
            "notion_last_edited_time": page.get("last_edited_time", ""),
        }
        for name, prop in properties.items():
            row[name] = extract_property_display_value(prop) or ""
        return row

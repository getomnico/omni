"""SharePoint document library syncer using delta queries."""

import logging
from datetime import datetime, timedelta, timezone
from typing import Any

from omni_connector import SyncContext

from ..graph_client import GraphClient, GraphAPIError
from ..mappers import (
    map_drive_item_to_document,
    generate_drive_item_content,
    _parse_iso,
)
from .base import DEFAULT_MAX_AGE_DAYS
from .onedrive import _is_indexable, _get_extension

logger = logging.getLogger(__name__)


class SharePointSyncer:
    """Syncs files from SharePoint site document libraries.

    Iterates over all sites in the tenant, then uses per-drive delta queries
    (same driveItem API as OneDrive) for each site's default document library.
    """

    @property
    def name(self) -> str:
        return "sharepoint"

    async def sync(
        self,
        client: GraphClient,
        ctx: SyncContext,
        state: dict[str, Any],
        source_config: dict[str, Any] | None = None,
        user_cache: dict[str, str] | None = None,
        group_cache: dict[str, str] | None = None,
    ) -> dict[str, Any]:
        self._user_cache = user_cache or {}
        self._group_cache = group_cache or {}

        delta_tokens: dict[str, str] = state.get("delta_tokens", {})
        new_tokens: dict[str, str] = {}

        sites = await self._list_sites(client)
        logger.info("[sharepoint] Syncing across %d sites", len(sites))

        for site in sites:
            if ctx.is_cancelled():
                return state

            site_id = site["id"]
            site_name = site.get("displayName", site_id)
            token = delta_tokens.get(site_id)

            new_token = await self._sync_site(client, site, ctx, token)
            if new_token:
                new_tokens[site_id] = new_token

            await ctx.save_state({"delta_tokens": new_tokens})
            logger.info("[sharepoint] Finished site %s", site_name)

        return {"delta_tokens": new_tokens}

    async def _list_sites(self, client: GraphClient) -> list[dict[str, Any]]:
        sites: list[dict[str, Any]] = []
        async for site in client.get_paginated(
            "/sites",
            params={"search": "*", "$select": "id,displayName,webUrl"},
        ):
            sites.append(site)
        return sites

    async def _sync_site(
        self,
        client: GraphClient,
        site: dict[str, Any],
        ctx: SyncContext,
        delta_token: str | None,
    ) -> str | None:
        site_id = site["id"]
        site_name = site.get("displayName", site_id)
        logger.info("[sharepoint] Syncing site %s", site_name)

        try:
            items, new_token = await client.get_delta(
                f"/sites/{site_id}/drive/root/delta",
                delta_token=delta_token,
                params={
                    "$select": "id,name,file,folder,size,webUrl,lastModifiedDateTime,"
                    "createdDateTime,parentReference,content.downloadUrl"
                },
            )
        except GraphAPIError as e:
            logger.warning(
                "[sharepoint] Failed to fetch delta for site %s: %s", site_name, e
            )
            return delta_token

        cutoff = (
            datetime.now(timezone.utc) - timedelta(days=DEFAULT_MAX_AGE_DAYS)
            if delta_token is None
            else None
        )

        skipped_folders = 0
        skipped_cutoff = 0
        skipped_deleted = 0

        for item in items:
            if ctx.is_cancelled():
                return delta_token

            if item.get("deleted"):
                skipped_deleted += 1
                drive_id = item.get("parentReference", {}).get("driveId", "unknown")
                external_id = f"sharepoint:{site_id}:{item['id']}"
                await ctx.emit_deleted(external_id)
                continue

            if "folder" in item:
                skipped_folders += 1
                continue

            if cutoff:
                modified = _parse_iso(item.get("lastModifiedDateTime"))
                if modified and modified < cutoff:
                    skipped_cutoff += 1
                    continue

            await ctx.increment_scanned()

            try:
                await self._process_item(client, site, item, ctx)
            except Exception as e:
                external_id = f"sharepoint:{site_id}:{item['id']}"
                logger.warning("[sharepoint] Error processing %s: %s", external_id, e)
                await ctx.emit_error(external_id, str(e))

        total = len(items)
        skipped = skipped_folders + skipped_cutoff + skipped_deleted
        if skipped:
            logger.info(
                "[sharepoint] Site %s: %d items total, %d skipped "
                "(folders=%d, cutoff=%d, deleted=%d)",
                site_name,
                total,
                skipped,
                skipped_folders,
                skipped_cutoff,
                skipped_deleted,
            )

        return new_token

    async def _process_item(
        self,
        client: GraphClient,
        site: dict[str, Any],
        item: dict[str, Any],
        ctx: SyncContext,
    ) -> None:
        file_info = item.get("file", {})
        mime_type = file_info.get("mimeType", "")
        file_name = item.get("name", "")
        extension = _get_extension(file_name)

        drive_id = item.get("parentReference", {}).get("driveId", "unknown")
        item_id = item["id"]

        if _is_indexable(mime_type, extension):
            content_id = await self._extract_file_content(
                client, item, mime_type, file_name, ctx
            )
        else:
            content = generate_drive_item_content(item, {})
            content_id = await ctx.content_storage.save(content, "text/plain")

        try:
            graph_permissions = await client.list_item_permissions(drive_id, item_id)
        except Exception as e:
            logger.warning(
                "[sharepoint] Failed to fetch permissions for %s: %s", item_id, e
            )
            graph_permissions = []
        doc = map_drive_item_to_document(
            item=item,
            content_id=content_id,
            source_type="share_point",
            graph_permissions=graph_permissions,
            user_cache=self._user_cache,
            group_cache=self._group_cache,
            site_id=site["id"],
        )
        await ctx.emit(doc)

    async def _extract_file_content(
        self,
        client: GraphClient,
        item: dict[str, Any],
        mime_type: str,
        file_name: str,
        ctx: SyncContext,
    ) -> str:
        """Download file and extract text via connector manager. Returns content_id."""
        drive_id = item.get("parentReference", {}).get("driveId")
        item_id = item["id"]

        if not drive_id:
            content = generate_drive_item_content(item, {})
            return await ctx.content_storage.save(content, "text/plain")

        try:
            data = await client.get_binary(
                f"/drives/{drive_id}/items/{item_id}/content"
            )
            return await ctx.content_storage.extract_and_store_content(
                data, mime_type, file_name
            )
        except Exception as e:
            logger.warning(
                "[sharepoint] Failed to extract content for %s: %s", item_id, e
            )
            content = generate_drive_item_content(item, {})
            return await ctx.content_storage.save(content, "text/plain")

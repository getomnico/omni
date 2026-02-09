"""SharePoint document library syncer using delta queries."""

import logging
from typing import Any

from omni_connector import SyncContext

from ..graph_client import GraphClient, GraphAPIError
from ..mappers import map_drive_item_to_document, generate_drive_item_content
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
    ) -> dict[str, Any]:
        delta_tokens: dict[str, str] = state.get("sharepoint_delta_tokens", {})
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

            logger.info("[sharepoint] Finished site %s", site_name)

        return {"sharepoint_delta_tokens": new_tokens}

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

        for item in items:
            if ctx.is_cancelled():
                return delta_token

            await ctx.increment_scanned()

            if item.get("deleted"):
                drive_id = item.get("parentReference", {}).get("driveId", "unknown")
                external_id = f"sharepoint:{site_id}:{item['id']}"
                await ctx.emit_deleted(external_id)
                continue

            if "folder" in item:
                continue

            try:
                await self._process_item(client, site, item, ctx)
            except Exception as e:
                external_id = f"sharepoint:{site_id}:{item['id']}"
                logger.warning("[sharepoint] Error processing %s: %s", external_id, e)
                await ctx.emit_error(external_id, str(e))

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

        if _is_indexable(mime_type, extension):
            content = await self._download_content(client, item)
        else:
            content = generate_drive_item_content(item, {})

        content_id = await ctx.content_storage.save(content, "text/plain")
        doc = map_drive_item_to_document(
            item=item,
            content_id=content_id,
            source_type="sharepoint",
            site_id=site["id"],
        )
        await ctx.emit(doc)

    async def _download_content(
        self,
        client: GraphClient,
        item: dict[str, Any],
    ) -> str:
        drive_id = item.get("parentReference", {}).get("driveId")
        item_id = item["id"]

        if not drive_id:
            return generate_drive_item_content(item, {})

        try:
            data = await client.get_binary(
                f"/drives/{drive_id}/items/{item_id}/content"
            )
            return data.decode("utf-8", errors="replace")
        except Exception:
            return generate_drive_item_content(item, {})

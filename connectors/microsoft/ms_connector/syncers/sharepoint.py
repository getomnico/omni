"""SharePoint document library syncer using delta queries.

Iterates every site in the tenant and every document library (drive) on
each site, then runs a per-drive delta query. Folders are emitted as
metadata-only documents (unlike OneDrive, which skips them) since
SharePoint folder structure is itself searchable context.
"""

import logging
import re
from collections import Counter
from dataclasses import dataclass
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


@dataclass(frozen=True)
class Site:
    id: str
    display_name: str
    web_url: str

    @classmethod
    def from_graph(cls, raw: dict[str, Any]) -> "Site":
        site_id = raw["id"]
        return cls(
            id=site_id,
            display_name=raw.get("displayName") or site_id,
            web_url=raw.get("webUrl") or "",
        )


@dataclass(frozen=True)
class Drive:
    id: str
    name: str

    @classmethod
    def from_graph(cls, raw: dict[str, Any]) -> "Drive":
        drive_id = raw["id"]
        return cls(id=drive_id, name=raw.get("name") or drive_id)


@dataclass(frozen=True)
class SiteDiagnostic:
    """Subset of /sites/{id} fields we use for 403 root-cause classification."""

    is_personal_site: bool
    archive_status: str | None

    @classmethod
    def from_graph(cls, raw: dict[str, Any]) -> "SiteDiagnostic":
        site_collection = raw.get("siteCollection") or {}
        archival = site_collection.get("archivalDetails") or {}
        return cls(
            is_personal_site=bool(raw.get("isPersonalSite")),
            archive_status=archival.get("archiveStatus"),
        )


# State key for per-drive delta tokens. Renamed from the old per-site
# `delta_tokens` shape — old tokens are intentionally discarded; the next
# sync re-snapshots within DEFAULT_MAX_AGE_DAYS.
DRIVE_DELTA_TOKENS_KEY = "drive_delta_tokens"

# Heuristic: SharePoint provisions one site per private/shared Teams channel,
# typically with a webUrl path of `/sites/<team>-<channel>`. Used only to
# bucket 403s — not authoritative.
_PRIVATE_CHANNEL_URL_RE = re.compile(r"/sites/[^/]+-[^/]+/?$")


class SharePointSyncer:
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

        delta_tokens: dict[str, str] = dict(state.get(DRIVE_DELTA_TOKENS_KEY, {}))
        skip_classifications: Counter[str] = Counter()

        sites = await self._list_sites(client)
        logger.info("[sharepoint] Syncing across %d sites", len(sites))

        for site in sites:
            if ctx.is_cancelled():
                return {DRIVE_DELTA_TOKENS_KEY: delta_tokens}

            try:
                raw_drives = await client.list_site_drives(site.id)
            except GraphAPIError as e:
                await self._classify_site_error(
                    client, site, e, skip_classifications, op="list_drives"
                )
                continue

            for raw_drive in raw_drives:
                if ctx.is_cancelled():
                    return {DRIVE_DELTA_TOKENS_KEY: delta_tokens}

                drive = Drive.from_graph(raw_drive)
                token = delta_tokens.get(drive.id)
                new_token = await self._sync_drive(
                    client, site, drive, ctx, token, skip_classifications
                )
                if new_token:
                    delta_tokens[drive.id] = new_token
                    await ctx.save_state({DRIVE_DELTA_TOKENS_KEY: delta_tokens})

            logger.info("[sharepoint] Finished site %s", site.display_name)

        if skip_classifications:
            logger.warning(
                "[sharepoint] Skipped sites/drives by classification: %s",
                dict(skip_classifications),
            )

        return {DRIVE_DELTA_TOKENS_KEY: delta_tokens}

    async def _list_sites(self, client: GraphClient) -> list[Site]:
        sites: list[Site] = []
        async for raw in client.get_paginated(
            "/sites",
            params={"search": "*", "$select": "id,displayName,webUrl"},
        ):
            sites.append(Site.from_graph(raw))
        return sites

    async def _sync_drive(
        self,
        client: GraphClient,
        site: Site,
        drive: Drive,
        ctx: SyncContext,
        delta_token: str | None,
        skip_classifications: Counter[str],
    ) -> str | None:
        logger.info(
            "[sharepoint] Syncing drive %s on site %s", drive.name, site.display_name
        )

        items, new_token = await self._fetch_delta_with_resync(
            client, site, drive, delta_token, skip_classifications
        )
        if items is None:
            return delta_token

        cutoff = (
            datetime.now(timezone.utc) - timedelta(days=DEFAULT_MAX_AGE_DAYS)
            if delta_token is None
            else None
        )

        skipped_cutoff = 0
        skipped_deleted = 0
        emitted_folders = 0

        for item in items:
            if ctx.is_cancelled():
                return delta_token

            if item.get("deleted"):
                skipped_deleted += 1
                external_id = f"sharepoint:{site.id}:{item['id']}"
                await ctx.emit_deleted(external_id)
                continue

            if cutoff:
                modified = _parse_iso(item.get("lastModifiedDateTime"))
                if modified and modified < cutoff:
                    skipped_cutoff += 1
                    continue

            if "folder" in item:
                emitted_folders += 1

            await ctx.increment_scanned()

            try:
                await self._process_item(client, site, item, ctx)
            except Exception as e:
                external_id = f"sharepoint:{site.id}:{item['id']}"
                logger.warning("[sharepoint] Error processing %s: %s", external_id, e)
                await ctx.emit_error(external_id, str(e))

        logger.info(
            "[sharepoint] Drive %s/%s: %d items (folders=%d, deleted=%d, "
            "cutoff_skipped=%d)",
            site.display_name,
            drive.name,
            len(items),
            emitted_folders,
            skipped_deleted,
            skipped_cutoff,
        )
        return new_token

    async def _fetch_delta_with_resync(
        self,
        client: GraphClient,
        site: Site,
        drive: Drive,
        delta_token: str | None,
        skip_classifications: Counter[str],
    ) -> tuple[list[dict[str, Any]] | None, str | None]:
        """Fetch delta; on 410/resyncRequired, retry once from scratch.

        See https://learn.microsoft.com/en-us/graph/delta-query-overview
        — the server returns 410 Gone when a delta token can no longer be
        honored and the client must restart with no token.
        """
        url = f"/drives/{drive.id}/root/delta"
        params = {
            "$select": (
                "id,name,file,folder,size,webUrl,lastModifiedDateTime,"
                "createdDateTime,parentReference,content.downloadUrl"
            )
        }
        try:
            return await client.get_delta(url, delta_token=delta_token, params=params)
        except GraphAPIError as e:
            if delta_token is not None and _is_resync_required(e):
                logger.warning(
                    "[sharepoint] Drive %s requires resync (%s), restarting "
                    "from scratch",
                    drive.id,
                    e.diagnostic(),
                )
                try:
                    return await client.get_delta(url, delta_token=None, params=params)
                except GraphAPIError as retry_err:
                    e = retry_err
            await self._classify_site_error(
                client, site, e, skip_classifications, op="delta"
            )
            return None, delta_token

    async def _process_item(
        self,
        client: GraphClient,
        site: Site,
        item: dict[str, Any],
        ctx: SyncContext,
    ) -> None:
        is_folder = "folder" in item
        file_info = item.get("file", {})
        mime_type = file_info.get("mimeType", "")
        file_name = item.get("name", "")
        extension = _get_extension(file_name)

        drive_id = item.get("parentReference", {}).get("driveId", "unknown")
        item_id = item["id"]

        if not is_folder and _is_indexable(mime_type, extension):
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
            site_id=site.id,
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

    async def _classify_site_error(
        self,
        client: GraphClient,
        site: Site,
        err: GraphAPIError,
        skip_classifications: Counter[str],
        op: str,
    ) -> None:
        """Bucket a failed site/drive call by likely root cause and log it.

        Per Microsoft docs (https://learn.microsoft.com/en-us/graph/errors and
        the resolve-auth-errors guide), Graph does not publish stable
        innerError codes for Sites.Selected mismatches, IRM containers, or
        restricted content discovery — they all surface as generic
        accessDenied. We classify on the only documented signals
        (isPersonalSite, siteCollection.archivalDetails) plus a webUrl
        heuristic for private Teams channel sites.
        """
        if err.status_code != 403:
            classification = (
                "not_found" if err.status_code == 404 else f"http_{err.status_code}"
            )
            skip_classifications[classification] += 1
            logger.warning(
                "[sharepoint] %s failed for site %s (%s): %s",
                op,
                site.display_name,
                classification,
                err.diagnostic(),
            )
            return

        if _PRIVATE_CHANNEL_URL_RE.search(site.web_url):
            classification = "likely_private_channel"
            skip_classifications[classification] += 1
            logger.warning(
                "[sharepoint] 403 on site %s (%s): %s",
                site.display_name,
                classification,
                err.diagnostic(),
            )
            return

        try:
            raw_diag = await client.get_site_diagnostic(site.id)
        except GraphAPIError as probe_err:
            classification = (
                "site_level_denied"
                if probe_err.status_code == 403
                else f"probe_http_{probe_err.status_code}"
            )
            skip_classifications[classification] += 1
            logger.warning(
                "[sharepoint] 403 on site %s (%s): original=%s probe=%s",
                site.display_name,
                classification,
                err.diagnostic(),
                probe_err.diagnostic(),
            )
            return

        diag = SiteDiagnostic.from_graph(raw_diag)
        if diag.is_personal_site:
            classification = "personal_onedrive"
        elif diag.archive_status in {
            "recentlyArchived",
            "fullyArchived",
            "reactivating",
        }:
            classification = f"archived_{diag.archive_status}"
        else:
            classification = "accessDenied_unclassified"

        skip_classifications[classification] += 1
        logger.warning(
            "[sharepoint] 403 on site %s (%s): %s",
            site.display_name,
            classification,
            err.diagnostic(),
        )


def _is_resync_required(err: GraphAPIError) -> bool:
    if err.status_code == 410:
        return True
    code = (err.error_code or "").lower()
    inner = (err.inner_error_code or "").lower()
    return "resync" in code or "resync" in inner

"""OneDrive file syncer using delta queries."""

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
from .base import BaseSyncer, DEFAULT_MAX_AGE_DAYS

logger = logging.getLogger(__name__)

INDEXABLE_MIME_PREFIXES = ("text/", "application/pdf", "application/json")
INDEXABLE_EXTENSIONS = {
    ".txt",
    ".md",
    ".csv",
    ".json",
    ".xml",
    ".html",
    ".htm",
    ".pdf",
    ".doc",
    ".docx",
    ".xls",
    ".xlsx",
    ".ppt",
    ".pptx",
    ".rtf",
    ".odt",
    ".ods",
    ".odp",
}


class OneDriveSyncer(BaseSyncer):
    @property
    def name(self) -> str:
        return "onedrive"

    async def sync_for_user(
        self,
        client: GraphClient,
        user: dict[str, Any],
        ctx: SyncContext,
        delta_token: str | None,
        user_cache: dict[str, str] | None = None,
        group_cache: dict[str, str] | None = None,
        delta_tokens: dict[str, str] | None = None,
        token_key: str | None = None,
    ) -> str | None:
        user_id = user["id"]
        display_name = user.get("displayName", user_id)
        logger.info("[onedrive] Syncing drive for user %s", display_name)

        cutoff = (
            datetime.now(timezone.utc) - timedelta(days=DEFAULT_MAX_AGE_DAYS)
            if delta_token is None
            else None
        )

        total = 0
        skipped_folders = 0
        skipped_cutoff = 0
        skipped_deleted = 0
        last_resume_token: str | None = delta_token
        final_token: str | None = None

        try:
            pages = client.get_delta_pages(
                f"/users/{user_id}/drive/root/delta",
                delta_token=delta_token,
                params={
                    "$select": "id,name,file,folder,size,webUrl,lastModifiedDateTime,"
                    "createdDateTime,parentReference,content.downloadUrl"
                },
            )
            async for items, next_link, delta_link in pages:
                total += len(items)
                cancelled = False

                for item in items:
                    if ctx.is_cancelled():
                        cancelled = True
                        break

                    if item.get("deleted"):
                        skipped_deleted += 1
                        drive_id = item.get("parentReference", {}).get(
                            "driveId", "unknown"
                        )
                        external_id = f"onedrive:{drive_id}:{item['id']}"
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
                        await self._process_item(
                            client, user, item, ctx, user_cache, group_cache
                        )
                    except Exception as e:
                        drive_id = item.get("parentReference", {}).get(
                            "driveId", "unknown"
                        )
                        external_id = f"onedrive:{drive_id}:{item['id']}"
                        logger.warning(
                            "[onedrive] Error processing %s: %s", external_id, e
                        )
                        await ctx.emit_error(external_id, str(e))

                if cancelled:
                    return last_resume_token

                # Checkpoint: store nextLink mid-stream, or deltaLink on final page.
                resume = next_link or delta_link
                if resume:
                    last_resume_token = resume
                    if delta_tokens is not None and token_key is not None:
                        await self.save_delta_token(
                            ctx, delta_tokens, token_key, resume
                        )
                if delta_link:
                    final_token = delta_link
        except GraphAPIError as e:
            if delta_token is not None and _is_resync_required(e):
                logger.warning(
                    "[onedrive] Delta token for user %s requires resync (%s), "
                    "restarting from scratch",
                    display_name,
                    e.diagnostic(),
                )
                return await self.sync_for_user(
                    client,
                    user,
                    ctx,
                    None,
                    user_cache=user_cache,
                    group_cache=group_cache,
                    delta_tokens=delta_tokens,
                    token_key=token_key,
                )
            logger.warning(
                "[onedrive] Failed to fetch delta for user %s: %s", display_name, e
            )
            return last_resume_token

        skipped = skipped_folders + skipped_cutoff + skipped_deleted
        if skipped:
            logger.info(
                "[onedrive] User %s: %d items total, %d skipped "
                "(folders=%d, cutoff=%d, deleted=%d)",
                display_name,
                total,
                skipped,
                skipped_folders,
                skipped_cutoff,
                skipped_deleted,
            )

        return final_token or last_resume_token

    async def _process_item(
        self,
        client: GraphClient,
        user: dict[str, Any],
        item: dict[str, Any],
        ctx: SyncContext,
        user_cache: dict[str, str] | None = None,
        group_cache: dict[str, str] | None = None,
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
            content = generate_drive_item_content(item, user)
            content_id = await ctx.content_storage.save(content, "text/plain")

        try:
            graph_permissions = await client.list_item_permissions(drive_id, item_id)
        except Exception as e:
            logger.warning(
                "[onedrive] Failed to fetch permissions for %s: %s", item_id, e
            )
            graph_permissions = []
        doc = map_drive_item_to_document(
            item=item,
            content_id=content_id,
            source_type="one_drive",
            graph_permissions=graph_permissions,
            user_cache=user_cache,
            group_cache=group_cache,
            owner_email=user.get("mail") or user.get("userPrincipalName"),
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
                "[onedrive] Failed to extract content for %s: %s", item_id, e
            )
            content = generate_drive_item_content(item, {})
            return await ctx.content_storage.save(content, "text/plain")


def _get_extension(filename: str) -> str:
    dot_idx = filename.rfind(".")
    if dot_idx == -1:
        return ""
    return filename[dot_idx:].lower()


def _is_indexable(mime_type: str, extension: str) -> bool:
    if any(mime_type.startswith(p) for p in INDEXABLE_MIME_PREFIXES):
        return True
    return extension in INDEXABLE_EXTENSIONS


def _is_resync_required(err: GraphAPIError) -> bool:
    if err.status_code == 410:
        return True
    code = (err.error_code or "").lower()
    inner = (err.inner_error_code or "").lower()
    return "resync" in code or "resync" in inner

"""OneDrive file syncer using delta queries."""

import logging
from typing import Any

from omni_connector import SyncContext

from ..graph_client import GraphClient, GraphAPIError
from ..mappers import map_drive_item_to_document, generate_drive_item_content
from .base import BaseSyncer

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
    ) -> str | None:
        user_id = user["id"]
        display_name = user.get("displayName", user_id)
        logger.info("[onedrive] Syncing drive for user %s", display_name)

        try:
            items, new_token = await client.get_delta(
                f"/users/{user_id}/drive/root/delta",
                delta_token=delta_token,
                params={
                    "$select": "id,name,file,folder,size,webUrl,lastModifiedDateTime,"
                    "createdDateTime,parentReference,content.downloadUrl"
                },
            )
        except GraphAPIError as e:
            logger.warning(
                "[onedrive] Failed to fetch delta for user %s: %s", display_name, e
            )
            return delta_token

        for item in items:
            if ctx.is_cancelled():
                return delta_token

            await ctx.increment_scanned()

            # Handle deletions
            if item.get("deleted"):
                drive_id = item.get("parentReference", {}).get("driveId", "unknown")
                external_id = f"onedrive:{drive_id}:{item['id']}"
                await ctx.emit_deleted(external_id)
                continue

            # Skip folders
            if "folder" in item:
                continue

            try:
                await self._process_item(client, user, item, ctx)
            except Exception as e:
                drive_id = item.get("parentReference", {}).get("driveId", "unknown")
                external_id = f"onedrive:{drive_id}:{item['id']}"
                logger.warning("[onedrive] Error processing %s: %s", external_id, e)
                await ctx.emit_error(external_id, str(e))

        return new_token

    async def _process_item(
        self,
        client: GraphClient,
        user: dict[str, Any],
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
            content = generate_drive_item_content(item, user)

        content_id = await ctx.content_storage.save(content, "text/plain")
        doc = map_drive_item_to_document(
            item=item,
            content_id=content_id,
            source_type="onedrive",
            owner_email=user.get("mail") or user.get("userPrincipalName"),
        )
        await ctx.emit(doc)

    async def _download_content(
        self,
        client: GraphClient,
        item: dict[str, Any],
    ) -> str:
        """Download file content and return as text. Falls back to metadata."""
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


def _get_extension(filename: str) -> str:
    dot_idx = filename.rfind(".")
    if dot_idx == -1:
        return ""
    return filename[dot_idx:].lower()


def _is_indexable(mime_type: str, extension: str) -> bool:
    if any(mime_type.startswith(p) for p in INDEXABLE_MIME_PREFIXES):
        return True
    return extension in INDEXABLE_EXTENSIONS

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

    async def sync(
        self,
        client: GraphClient,
        ctx: SyncContext,
        state: dict[str, Any],
        source_config: dict[str, Any] | None = None,
        user_cache: dict[str, str] | None = None,
        group_cache: dict[str, str] | None = None,
    ) -> dict[str, Any]:
        """Run delta sync and shared-with-me sync in one user loop."""
        source_config = source_config or {}
        delta_tokens: dict[str, str] = state.get("delta_tokens", {})
        shared_state: dict[str, dict[str, str]] = state.get("shared_items", {})
        new_tokens: dict[str, str] = {}
        new_shared: dict[str, dict[str, str]] = {}

        users = await client.list_users()
        logger.info("[onedrive] Syncing across %d users", len(users))

        users = [
            u
            for u in users
            if ctx.should_index_user(u.get("mail") or u.get("userPrincipalName") or "")
        ]
        logger.info("[onedrive] %d users after filtering", len(users))

        for user in users:
            if ctx.is_cancelled():
                logger.info("[onedrive] Cancelled")
                return state

            user_id = user["id"]

            new_token = await self.sync_for_user(
                client,
                user,
                ctx,
                delta_tokens.get(user_id),
                user_cache=user_cache,
                group_cache=group_cache,
            )
            if new_token:
                new_tokens[user_id] = new_token

            new_shared[user_id] = await self._sync_shared_with_me(
                client,
                user,
                ctx,
                shared_state.get(user_id, {}),
                user_cache,
                group_cache,
            )

        return {"delta_tokens": new_tokens, "shared_items": new_shared}

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

        cutoff = (
            datetime.now(timezone.utc) - timedelta(days=DEFAULT_MAX_AGE_DAYS)
            if delta_token is None
            else None
        )

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

            # On initial sync, skip files older than the max age cutoff
            if cutoff:
                modified = _parse_iso(item.get("lastModifiedDateTime"))
                if modified and modified < cutoff:
                    continue

            try:
                await self._process_item(
                    client, user, item, ctx, user_cache, group_cache
                )
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

    async def _sync_shared_with_me(
        self,
        client: GraphClient,
        user: dict[str, Any],
        ctx: SyncContext,
        seen_items: dict[str, str],
        user_cache: dict[str, str] | None = None,
        group_cache: dict[str, str] | None = None,
    ) -> dict[str, str]:
        """Sync files shared with a user. Returns updated seen-items map.

        seen_items maps "driveId:itemId" to lastModifiedDateTime for
        incremental skip logic (sharedWithMe has no delta query support).
        """
        user_id = user["id"]
        display_name = user.get("displayName", user_id)
        logger.info("[onedrive] Syncing shared-with-me for user %s", display_name)

        try:
            shared_items = await client.list_shared_with_me(user_id)
        except GraphAPIError as e:
            logger.warning(
                "[onedrive] Failed to fetch sharedWithMe for %s: %s",
                display_name,
                e,
            )
            return seen_items

        new_seen: dict[str, str] = {}

        for item in shared_items:
            if ctx.is_cancelled():
                return seen_items

            remote = item.get("remoteItem")
            if not remote:
                continue

            if "folder" in remote:
                continue

            await ctx.increment_scanned()

            remote_drive_id = remote.get("parentReference", {}).get(
                "driveId", "unknown"
            )
            remote_item_id = remote["id"]
            key = f"{remote_drive_id}:{remote_item_id}"

            last_modified = remote.get("lastModifiedDateTime", "")
            new_seen[key] = last_modified

            if seen_items.get(key) == last_modified:
                continue

            try:
                await self._process_item(
                    client, user, remote, ctx, user_cache, group_cache
                )
            except Exception as e:
                external_id = f"onedrive:{remote_drive_id}:{remote_item_id}"
                logger.warning(
                    "[onedrive] Error processing shared item %s: %s",
                    external_id,
                    e,
                )
                await ctx.emit_error(external_id, str(e))

        return new_seen


def _get_extension(filename: str) -> str:
    dot_idx = filename.rfind(".")
    if dot_idx == -1:
        return ""
    return filename[dot_idx:].lower()


def _is_indexable(mime_type: str, extension: str) -> bool:
    if any(mime_type.startswith(p) for p in INDEXABLE_MIME_PREFIXES):
        return True
    return extension in INDEXABLE_EXTENSIONS

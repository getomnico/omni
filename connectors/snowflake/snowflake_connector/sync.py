from __future__ import annotations

import hashlib
import json
from collections.abc import Callable
from datetime import UTC, datetime, timedelta
from typing import Any

from omni_connector import Document, SyncContext, SyncMode

from .client import SnowflakeClient, default_session_factory
from .config import SnowflakeConfig, SnowflakeCredentials
from .mappers import SnowflakeDocumentDraft, database_document, schema_document, table_document
from .models import SnowflakeCheckpoint
from .permissions import SnowflakePermissions


class SnowflakeSync:
    def __init__(
        self,
        session_factory: Callable[
            [SnowflakeConfig, SnowflakeCredentials], Any
        ] = default_session_factory,
    ) -> None:
        self._session_factory = session_factory

    async def run(
        self,
        source_config: dict[str, Any],
        credentials: dict[str, Any],
        checkpoint_data: dict[str, Any] | None,
        ctx: SyncContext,
    ) -> None:
        config = SnowflakeConfig.model_validate(source_config)
        config.validate_for_use()
        if not config.sync_enabled:
            await ctx.complete(checkpoint=checkpoint_data or {})
            return
        parsed_credentials = SnowflakeCredentials.model_validate(_credential_payload(credentials))
        session = self._session_factory(config, parsed_credentials)
        client = SnowflakeClient(session)
        try:
            client.verify_identity()
            await self._sync_metadata(client, config, checkpoint_data, ctx)
        except Exception as error:
            await ctx.fail(self._safe_error(error))
        finally:
            session.close()

    async def _sync_metadata(
        self,
        client: SnowflakeClient,
        config: SnowflakeConfig,
        checkpoint_data: dict[str, Any] | None,
        ctx: SyncContext,
    ) -> None:
        now = datetime.now(UTC)
        source_fingerprint = _fingerprint(config)
        old_checkpoint = _parse_checkpoint(checkpoint_data)
        full = (
            ctx.sync_mode == SyncMode.FULL
            or old_checkpoint is None
            or old_checkpoint.source_fingerprint != source_fingerprint
        )
        if ctx.sync_mode not in (SyncMode.FULL, SyncMode.INCREMENTAL):
            raise ValueError(f"Snowflake does not support sync mode {ctx.sync_mode.value}")
        cutoff = now - timedelta(hours=3)
        changed_since = (
            None
            if full
            else _overlap_start(
                old_checkpoint
                or SnowflakeCheckpoint(mode="incremental", source_fingerprint=source_fingerprint),
                cutoff,
            )
        )
        users = client.users()
        grants = client.grants()
        edges = client.role_edges()
        user_roles = client.user_roles()
        permissions = SnowflakePermissions(users, grants, edges, user_roles)
        group_members = permissions.group_members()
        acl_fingerprint = _fingerprint(group_members)
        permission_repair = (
            old_checkpoint is not None and old_checkpoint.acl_fingerprint != acl_fingerprint
        )
        if permission_repair:
            changed_since = None
        prior_inventory = _inventory(ctx.connector_state)
        seen_inventory: set[str] = set()

        for group, members in group_members.items():
            if ctx.is_cancelled():
                await ctx.fail("Cancelled by user")
                return
            await ctx.emit_group_membership(group, members, group.removeprefix("snowflake:role:"))

        if full:
            for database_row in client.databases(config):
                if await self._emit_document(
                    database_document(database_row),
                    ctx,
                ):
                    await ctx.increment_scanned()
                seen_inventory.add(f"database:{database_row.database_id}")
            for schema_row in client.schemas(config):
                if await self._emit_document(
                    schema_document(schema_row),
                    ctx,
                ):
                    await ctx.increment_scanned()
                seen_inventory.add(f"schema:{schema_row.schema_id}")

        for table in client.tables(config, changed_since=changed_since):
            if (
                ctx.is_resume
                and old_checkpoint is not None
                and old_checkpoint.current_phase == "tables"
                and old_checkpoint.last_completed_key is not None
                and old_checkpoint.source_fingerprint == source_fingerprint
                and old_checkpoint.mode == ctx.sync_mode.value
                and table.table_id <= old_checkpoint.last_completed_key.removeprefix("table:")
            ):
                continue
            if ctx.is_cancelled():
                await ctx.fail("Cancelled by user")
                return
            columns = client.columns(table)
            users_for_document, groups_for_document = permissions.permissions_for(table)
            draft = table_document(table, columns, users_for_document, groups_for_document)
            if await self._emit_document(draft, ctx):
                await ctx.increment_scanned()
            seen_inventory.add(draft.external_id)
            completed_key = f"table:{table.table_id}"
            checkpoint = SnowflakeCheckpoint(
                mode=ctx.sync_mode.value,
                visibility_cutoff=cutoff,
                last_full_reconciliation_at=now
                if full
                else old_checkpoint.last_full_reconciliation_at
                if old_checkpoint
                else None,
                source_fingerprint=source_fingerprint,
                acl_fingerprint=acl_fingerprint,
                run_started_at=now,
                current_phase="tables",
                last_completed_key=completed_key,
                committed_watermark=cutoff,
            )
            await ctx.save_checkpoint(checkpoint.model_dump(mode="json"))

        deleted_ids = (
            prior_inventory - seen_inventory
            if full
            else set(client.deleted_external_ids(config, changed_since))
        )
        for external_id in sorted(deleted_ids):
            await ctx.emit_deleted(external_id)
        inventory = seen_inventory if full else (prior_inventory | seen_inventory) - deleted_ids
        await ctx.save_connector_state({"inventory": sorted(inventory)})

        # The durable checkpoint is promoted by connector-manager only after
        # completion; this final flush boundary prevents watermark loss.
        final = SnowflakeCheckpoint(
            mode=ctx.sync_mode.value,
            visibility_cutoff=cutoff,
            last_full_reconciliation_at=now
            if full
            else old_checkpoint.last_full_reconciliation_at
            if old_checkpoint
            else None,
            source_fingerprint=source_fingerprint,
            acl_fingerprint=acl_fingerprint,
            committed_watermark=cutoff,
        )
        await ctx.complete(checkpoint=final.model_dump(mode="json"))

    async def _emit_document(self, draft: SnowflakeDocumentDraft, ctx: SyncContext) -> bool:
        content_id = await ctx.content_storage.save(draft.content)
        document = Document(
            external_id=draft.external_id,
            title=draft.title,
            content_id=content_id,
            metadata=draft.metadata,
            permissions=draft.permissions,
            attributes=draft.attributes,
        )
        if ctx.sync_mode == SyncMode.INCREMENTAL:
            await ctx.emit_updated(document)
        else:
            await ctx.emit(document)
        return True

    @staticmethod
    def _safe_error(error: Exception) -> str:
        return f"Snowflake metadata sync failed: {type(error).__name__}"


def _credential_payload(credentials: dict[str, Any]) -> dict[str, Any]:
    nested = credentials.get("credentials")
    if isinstance(nested, dict):
        return nested
    return credentials


def _inventory(value: dict[str, Any]) -> set[str]:
    raw = value.get("inventory")
    if not isinstance(raw, list) or not all(isinstance(item, str) for item in raw):
        return set()
    return set(raw)


def _parse_checkpoint(value: dict[str, Any] | None) -> SnowflakeCheckpoint | None:
    if value is None:
        return None
    return SnowflakeCheckpoint.model_validate(value)


def _overlap_start(checkpoint: SnowflakeCheckpoint, cutoff: datetime) -> str:
    watermark = checkpoint.committed_watermark or checkpoint.visibility_cutoff or cutoff
    return (watermark - timedelta(hours=6)).isoformat()


def _fingerprint(value: object) -> str:
    encoded = json.dumps(value, sort_keys=True, default=str, separators=(",", ":")).encode()
    return hashlib.sha256(encoded).hexdigest()

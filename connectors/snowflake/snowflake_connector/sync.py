from __future__ import annotations

import asyncio
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

FULL_REPAIR_INTERVAL = timedelta(days=7)


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
        session = await asyncio.to_thread(self._session_factory, config, parsed_credentials)
        client = SnowflakeClient(session)
        try:
            await asyncio.to_thread(client.verify_identity)
            await self._sync_metadata(client, config, checkpoint_data, ctx)
        except Exception as error:
            await ctx.fail(self._safe_error(error))
        finally:
            await asyncio.to_thread(session.close)

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
        # Keep operations on the Snowflake connection serialized. The driver
        # exposes synchronous cursors and does not guarantee concurrent use of
        # one connection, even though each call must stay off the event loop.
        users = await asyncio.to_thread(client.users)
        grants = await asyncio.to_thread(client.grants)
        edges = await asyncio.to_thread(client.role_edges)
        user_roles = await asyncio.to_thread(client.user_roles)
        permissions = SnowflakePermissions(users, grants, edges, user_roles)
        group_members = permissions.group_members()
        acl_fingerprint = _fingerprint(
            {
                "users": [user.model_dump(mode="json") for user in users],
                "grants": [grant.model_dump(mode="json") for grant in grants],
                "edges": [edge.model_dump(mode="json") for edge in edges],
                "user_roles": [role.model_dump(mode="json") for role in user_roles],
            }
        )
        permission_repair = (
            old_checkpoint is not None and old_checkpoint.acl_fingerprint != acl_fingerprint
        )
        periodic_repair = (
            old_checkpoint is not None
            and old_checkpoint.last_full_reconciliation_at is not None
            and now - old_checkpoint.last_full_reconciliation_at >= FULL_REPAIR_INTERVAL
        )
        full_reconciliation = full or permission_repair or periodic_repair
        changed_since = (
            None
            if full_reconciliation
            else _overlap_start(
                old_checkpoint
                or SnowflakeCheckpoint(mode="incremental", source_fingerprint=source_fingerprint),
                cutoff,
            )
        )
        prior_inventory = _inventory(ctx.connector_state)
        seen_inventory: set[str] = set()

        for group, members in group_members.items():
            if ctx.is_cancelled():
                await ctx.fail("Cancelled by user")
                return
            await ctx.emit_group_membership(group, members, group.removeprefix("snowflake:role:"))

        if full_reconciliation:
            for database_row in await asyncio.to_thread(
                client.databases, config, changed_since=None
            ):
                users_for_document, groups_for_document = permissions.permissions_for_database(
                    database_row.database_name
                )
                if await self._emit_document(
                    database_document(database_row, users_for_document, groups_for_document),
                    ctx,
                ):
                    await ctx.increment_scanned()
                seen_inventory.add(f"database:{database_row.database_id}")
            for schema_row in await asyncio.to_thread(client.schemas, config, changed_since=None):
                users_for_document, groups_for_document = permissions.permissions_for_schema(
                    schema_row.database_name, schema_row.schema_name
                )
                if await self._emit_document(
                    schema_document(schema_row, users_for_document, groups_for_document),
                    ctx,
                ):
                    await ctx.increment_scanned()
                seen_inventory.add(f"schema:{schema_row.schema_id}")

        databases = (
            await asyncio.to_thread(client.databases, config, changed_since=changed_since)
            if not full_reconciliation
            else []
        )
        schemas = (
            await asyncio.to_thread(client.schemas, config, changed_since=changed_since)
            if not full_reconciliation
            else []
        )
        for database_row in databases:
            users_for_document, groups_for_document = permissions.permissions_for_database(
                database_row.database_name
            )
            if await self._emit_document(
                database_document(database_row, users_for_document, groups_for_document), ctx
            ):
                await ctx.increment_scanned()
            seen_inventory.add(f"database:{database_row.database_id}")
        for schema_row in schemas:
            users_for_document, groups_for_document = permissions.permissions_for_schema(
                schema_row.database_name, schema_row.schema_name
            )
            if await self._emit_document(
                schema_document(schema_row, users_for_document, groups_for_document), ctx
            ):
                await ctx.increment_scanned()
            seen_inventory.add(f"schema:{schema_row.schema_id}")

        tables = await asyncio.to_thread(client.tables, config, changed_since=changed_since)
        for table in tables:
            if (
                ctx.is_resume
                and old_checkpoint is not None
                and old_checkpoint.current_phase == "tables"
                and old_checkpoint.last_completed_key is not None
                and old_checkpoint.source_fingerprint == source_fingerprint
                and old_checkpoint.mode == ctx.sync_mode.value
                and table_id_from_key(f"table:{table.table_id}")
                <= table_id_from_key(old_checkpoint.last_completed_key)
            ):
                continue
            if ctx.is_cancelled():
                await ctx.fail("Cancelled by user")
                return
            columns = await asyncio.to_thread(client.columns, table)
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
                if full_reconciliation
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
            if full_reconciliation
            else prior_inventory.intersection(
                await asyncio.to_thread(client.deleted_external_ids, config, changed_since)
            )
        )
        for external_id in sorted(deleted_ids):
            await ctx.emit_deleted(external_id)
        inventory = (
            seen_inventory
            if full_reconciliation
            else (prior_inventory | seen_inventory) - deleted_ids
        )
        await ctx.save_connector_state({"inventory": sorted(inventory)})

        # The durable checkpoint is promoted by connector-manager only after
        # completion; this final flush boundary prevents watermark loss.
        final = SnowflakeCheckpoint(
            mode=ctx.sync_mode.value,
            visibility_cutoff=cutoff,
            last_full_reconciliation_at=now
            if full_reconciliation
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


def table_id_from_key(value: str) -> int:
    key = value.removeprefix("table:")
    try:
        return int(key)
    except ValueError as error:
        raise ValueError(f"invalid Snowflake table checkpoint key: {value}") from error


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

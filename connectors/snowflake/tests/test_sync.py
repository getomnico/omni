from __future__ import annotations

from datetime import UTC, datetime, timedelta
from typing import Any

import pytest
from omni_connector import SyncMode

from snowflake_connector.config import SnowflakeConfig
from snowflake_connector.models import (
    ColumnRow,
    DatabaseRow,
    SchemaRow,
    SnowflakeCheckpoint,
    SnowflakeGrant,
    SnowflakeRoleEdge,
    SnowflakeUser,
    SnowflakeUserRole,
    TableRow,
)
from snowflake_connector.sync import SnowflakeSync, _fingerprint


class Storage:
    async def save(self, content: str) -> str:
        return f"content:{len(content)}"


class Context:
    def __init__(
        self,
        *,
        sync_mode: str = "incremental",
        checkpoint: dict[str, Any] | None = None,
        connector_state: dict[str, Any] | None = None,
        is_resume: bool = False,
    ) -> None:
        self.sync_mode = SyncMode(sync_mode)
        self.connector_state = connector_state or {}
        self.is_resume = is_resume
        self.content_storage = Storage()
        self.updated: list[str] = []
        self.deleted: list[str] = []
        self.checkpoints: list[dict[str, Any]] = []
        self.completed: dict[str, Any] | None = None
        self.failed: str | None = None
        self.groups: list[str] = []

    def is_cancelled(self) -> bool:
        return False

    async def emit_group_membership(self, group: str, members: list[str], name: str) -> None:
        self.groups.append(group)

    async def emit_updated(self, document: Any) -> None:
        self.updated.append(document.external_id)

    async def emit(self, document: Any) -> None:
        self.updated.append(document.external_id)

    async def emit_deleted(self, external_id: str) -> None:
        self.deleted.append(external_id)

    async def increment_scanned(self) -> None:
        return None

    async def save_checkpoint(self, checkpoint: dict[str, Any]) -> None:
        self.checkpoints.append(checkpoint)

    async def save_connector_state(self, state: dict[str, Any]) -> None:
        self.connector_state = state

    async def complete(self, checkpoint: dict[str, Any] | None = None) -> None:
        self.completed = checkpoint

    async def fail(self, message: str) -> None:
        self.failed = message


class Client:
    def __init__(self, tables: list[TableRow], deleted: list[str] | None = None) -> None:
        self._tables = tables
        self._deleted = deleted or []
        self.table_changed_since: str | None = None
        self.database_changed_since: str | None = None
        self.schema_changed_since: str | None = None

    def verify_identity(self) -> tuple[str, str, str | None]:
        return ("ACME", "OMNI", "AWS_US_EAST_1")

    def users(self) -> list[SnowflakeUser]:
        return [SnowflakeUser(name="ALICE", email="alice@example.com")]

    def grants(self) -> list[SnowflakeGrant]:
        return []

    def role_edges(self) -> list[SnowflakeRoleEdge]:
        return []

    def user_roles(self) -> list[SnowflakeUserRole]:
        return []

    def databases(
        self, config: SnowflakeConfig, *, changed_since: str | None = None
    ) -> list[DatabaseRow]:
        self.database_changed_since = changed_since
        return [DatabaseRow(database_id="1", database_name="ANALYTICS")]

    def schemas(
        self, config: SnowflakeConfig, *, changed_since: str | None = None
    ) -> list[SchemaRow]:
        self.schema_changed_since = changed_since
        return [SchemaRow(schema_id="2", database_name="ANALYTICS", schema_name="PUBLIC")]

    def tables(
        self, config: SnowflakeConfig, *, changed_since: str | None = None
    ) -> list[TableRow]:
        self.table_changed_since = changed_since
        return self._tables

    def columns(self, table: TableRow) -> list[ColumnRow]:
        return [
            ColumnRow(
                table_id=table.table_id,
                column_name="ID",
                ordinal_position=1,
                data_type="NUMBER",
            )
        ]

    def deleted_external_ids(self, config: SnowflakeConfig, since: str | None) -> list[str]:
        return self._deleted


def config() -> SnowflakeConfig:
    return SnowflakeConfig(
        account_url="https://acme.snowflakecomputing.com",
        warehouse="W",
        role="R",
        databases=["ANALYTICS"],
    )


def acl_fingerprint() -> str:
    return _fingerprint(
        {
            "users": [
                SnowflakeUser(name="ALICE", email="alice@example.com").model_dump(mode="json")
            ],
            "grants": [],
            "edges": [],
            "user_roles": [],
        }
    )


def table(table_id: str) -> TableRow:
    return TableRow(
        table_id=table_id,
        database_name="ANALYTICS",
        schema_name="PUBLIC",
        object_name=f"TABLE_{table_id}",
        object_type="BASE TABLE",
    )


@pytest.mark.asyncio
async def test_incremental_sync_indexes_changed_database_and_schema_and_uses_watermark() -> None:
    client = Client([table("100")])
    now = datetime.now(UTC)
    checkpoint = SnowflakeCheckpoint(
        mode="incremental",
        source_fingerprint=_fingerprint(config()),
        acl_fingerprint=acl_fingerprint(),
        committed_watermark=now - timedelta(hours=8),
        last_full_reconciliation_at=now,
    ).model_dump(mode="json")
    context = Context(checkpoint=checkpoint, connector_state={"inventory": []})

    await SnowflakeSync()._sync_metadata(client, config(), checkpoint, context)  # type: ignore[arg-type]

    assert client.database_changed_since is not None
    assert client.schema_changed_since is not None
    assert client.table_changed_since is not None
    assert context.completed is not None
    assert {"database:1", "schema:2", "table:100"}.issubset(set(context.updated))


@pytest.mark.asyncio
async def test_resume_compares_numeric_table_ids_and_emits_deletions() -> None:
    client = Client([table("100")], deleted=["table:99"])
    now = datetime.now(UTC)
    checkpoint = SnowflakeCheckpoint(
        mode="incremental",
        source_fingerprint=_fingerprint(config()),
        acl_fingerprint=acl_fingerprint(),
        committed_watermark=now - timedelta(hours=1),
        last_full_reconciliation_at=now,
        current_phase="tables",
        last_completed_key="table:99",
    ).model_dump(mode="json")
    context = Context(
        checkpoint=checkpoint,
        connector_state={"inventory": ["table:99"]},
        is_resume=True,
    )

    await SnowflakeSync()._sync_metadata(client, config(), checkpoint, context)  # type: ignore[arg-type]

    assert "table:100" in context.updated
    assert context.deleted == ["table:99"]


@pytest.mark.asyncio
async def test_acl_change_forces_conservative_reconciliation() -> None:
    client = Client([table("100")])
    now = datetime.now(UTC)
    checkpoint = SnowflakeCheckpoint(
        mode="incremental",
        source_fingerprint=_fingerprint(config()),
        acl_fingerprint="stale",
        committed_watermark=now - timedelta(hours=1),
        last_full_reconciliation_at=now,
    ).model_dump(mode="json")
    context = Context(checkpoint=checkpoint)

    await SnowflakeSync()._sync_metadata(client, config(), checkpoint, context)  # type: ignore[arg-type]

    assert client.table_changed_since is None
    assert context.completed is not None

from __future__ import annotations

from datetime import datetime
from typing import Literal

from pydantic import BaseModel, ConfigDict, Field


class SnowflakeCheckpoint(BaseModel):
    """Versioned source checkpoint; progress is only committed after flush."""

    model_config = ConfigDict(extra="forbid")

    schema_version: Literal[1] = 1
    mode: Literal["full", "incremental"]
    visibility_cutoff: datetime | None = None
    last_full_reconciliation_at: datetime | None = None
    source_fingerprint: str
    acl_fingerprint: str | None = None
    run_started_at: datetime | None = None
    current_phase: str | None = None
    last_completed_key: str | None = None
    committed_watermark: datetime | None = None


class DatabaseRow(BaseModel):
    database_id: str
    database_name: str
    comment: str | None = None
    owner_role: str | None = None
    created_at: datetime | None = None
    last_altered: datetime | None = None
    deleted: datetime | None = None


class SchemaRow(BaseModel):
    schema_id: str
    database_name: str
    schema_name: str
    comment: str | None = None
    owner_role: str | None = None
    created_at: datetime | None = None
    last_altered: datetime | None = None
    deleted: datetime | None = None


class TableRow(BaseModel):
    table_id: str
    database_name: str
    schema_name: str
    object_name: str
    object_type: str
    comment: str | None = None
    owner_role: str | None = None
    created_at: datetime | None = None
    last_ddl: datetime | None = None
    deleted: datetime | None = None
    is_transient: bool = False
    is_iceberg: bool = False
    is_dynamic: bool = False
    is_hybrid: bool = False
    is_event: bool = False


class ColumnRow(BaseModel):
    table_id: str
    column_name: str
    ordinal_position: int
    data_type: str
    comment: str | None = None
    is_nullable: bool | None = None
    default_expression: str | None = None
    identity_generation: str | None = None
    expression: str | None = None
    alias: str | None = None


class SnowflakeGrant(BaseModel):
    grantee_name: str
    grantee_type: str
    privilege: str | None = None
    object_type: str | None = None
    object_name: str | None = None
    object_database: str | None = None
    object_schema: str | None = None
    deleted_on: datetime | None = None


class SnowflakeUser(BaseModel):
    name: str
    email: str | None = None
    disabled: bool = False


class SnowflakeRoleEdge(BaseModel):
    parent_role: str
    child_role: str


class SnowflakeUserRole(BaseModel):
    user_name: str
    role_name: str


class SnowflakeObject(BaseModel):
    external_id: str
    title: str
    object_type: str
    database_name: str
    schema_name: str | None = None
    comment: str | None = None
    owner_role: str | None = None
    columns: list[ColumnRow] = Field(default_factory=list)
    tags: dict[str, str] = Field(default_factory=dict)


ProviderTableType = Literal["TABLE", "VIEW", "EXTERNAL TABLE", "EVENT TABLE", "MATERIALIZED VIEW"]

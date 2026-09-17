from __future__ import annotations

import json
from dataclasses import dataclass

from omni_connector import DocumentMetadata, DocumentPermissions

from .models import ColumnRow, DatabaseRow, SchemaRow, TableRow


@dataclass(frozen=True)
class SnowflakeDocumentDraft:
    external_id: str
    title: str
    content: str
    metadata: DocumentMetadata
    attributes: dict[str, object]
    permissions: DocumentPermissions


def database_document(
    row: DatabaseRow, users: list[str] | None = None, groups: list[str] | None = None
) -> SnowflakeDocumentDraft:
    return SnowflakeDocumentDraft(
        external_id=f"database:{row.database_id}",
        title=row.database_name,
        content=f"Snowflake database: {row.database_name}",
        metadata=DocumentMetadata(
            title=row.database_name,
            created_at=row.created_at,
            updated_at=row.last_altered,
            extra={
                "object_type": "DATABASE",
                "owner_role": row.owner_role,
                "comment": row.comment,
            },
        ),
        attributes={
            "database": row.database_name,
            "object_type": "DATABASE",
            "owner_role": row.owner_role,
        },
        permissions=DocumentPermissions(public=False, users=users or [], groups=groups or []),
    )


def schema_document(
    row: SchemaRow, users: list[str] | None = None, groups: list[str] | None = None
) -> SnowflakeDocumentDraft:
    title = f"{row.database_name}.{row.schema_name}"
    return SnowflakeDocumentDraft(
        external_id=f"schema:{row.schema_id}",
        title=title,
        content=f"Snowflake schema: {title}",
        metadata=DocumentMetadata(
            title=title,
            created_at=row.created_at,
            updated_at=row.last_altered,
            extra={
                "object_type": "SCHEMA",
                "owner_role": row.owner_role,
                "comment": row.comment,
            },
        ),
        attributes={
            "database": row.database_name,
            "schema": row.schema_name,
            "object_type": "SCHEMA",
            "owner_role": row.owner_role,
        },
        permissions=DocumentPermissions(public=False, users=users or [], groups=groups or []),
    )


def table_document(
    row: TableRow,
    columns: list[ColumnRow],
    users: list[str],
    groups: list[str],
    tags: dict[str, str] | None = None,
) -> SnowflakeDocumentDraft:
    title = f"{row.database_name}.{row.schema_name}.{row.object_name}"
    return SnowflakeDocumentDraft(
        external_id=f"table:{row.table_id}",
        title=title,
        content=_table_content(row, columns, tags or {}),
        metadata=DocumentMetadata(
            title=title,
            created_at=row.created_at,
            updated_at=row.last_ddl,
            extra={
                "object_type": row.object_type,
                "database": row.database_name,
                "schema": row.schema_name,
                "owner_role": row.owner_role,
                "transient": row.is_transient,
                "iceberg": row.is_iceberg,
                "dynamic": row.is_dynamic,
                "hybrid": row.is_hybrid,
                "event": row.is_event,
                "tags": tags or {},
            },
        ),
        attributes={
            "database": row.database_name,
            "schema": row.schema_name,
            "object_type": row.object_type,
            "owner_role": row.owner_role,
            "column_name": [column.column_name for column in columns],
            "tag": list((tags or {}).keys()),
        },
        permissions=DocumentPermissions(public=False, users=users, groups=groups),
    )


def _table_content(row: TableRow, columns: list[ColumnRow], tags: dict[str, str]) -> str:
    lines = [
        f"Snowflake {row.object_type}: {row.database_name}.{row.schema_name}.{row.object_name}",
        f"Comment: {row.comment or '(none)'}",
        f"Owner role: {row.owner_role or '(unknown)'}",
        "Columns:",
    ]
    for column in sorted(columns, key=lambda item: item.ordinal_position):
        details = [column.data_type]
        if column.is_nullable is not None:
            details.append("nullable" if column.is_nullable else "not null")
        if column.comment:
            details.append(f"comment: {column.comment}")
        lines.append(f"- {column.column_name}: {', '.join(details)}")
    if tags:
        lines.append("Tags: " + json.dumps(tags, sort_keys=True))
    return "\n".join(lines)

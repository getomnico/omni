from __future__ import annotations

import re
from collections.abc import Iterable, Mapping
from dataclasses import dataclass
from typing import Any, Protocol
from urllib.parse import urlparse

from .config import SnowflakeConfig, SnowflakeCredentials
from .models import (
    ColumnRow,
    DatabaseRow,
    SchemaRow,
    SnowflakeGrant,
    SnowflakeRoleEdge,
    SnowflakeUser,
    SnowflakeUserRole,
    TableRow,
)


class SnowflakeSession(Protocol):
    def execute(self, statement: str) -> Iterable[Mapping[str, object]]: ...
    def close(self) -> None: ...


class _OfficialSnowflakeSession:
    def __init__(self, connection: object) -> None:
        self._connection: Any = connection

    def execute(self, statement: str) -> Iterable[Mapping[str, object]]:
        cursor = self._connection.cursor()
        try:
            cursor.execute(statement)
            columns = [description[0] for description in cursor.description or []]
            return [dict(zip(columns, row, strict=True)) for row in cursor.fetchall()]
        finally:
            cursor.close()

    def close(self) -> None:
        self._connection.close()


@dataclass
class SnowflakeClient:
    session: SnowflakeSession

    def rows(self, statement: str) -> list[dict[str, object]]:
        rows = self.session.execute(statement)
        result: list[dict[str, object]] = []
        for row in rows:
            if not isinstance(row, Mapping):
                raise TypeError("Snowflake provider returned a non-object row")
            result.append({str(key).upper(): value for key, value in row.items()})
        return result

    def databases(
        self, config: SnowflakeConfig, *, changed_since: str | None = None
    ) -> list[DatabaseRow]:
        clauses = ["DELETED IS NULL"]
        if changed_since is not None:
            clauses.append(_changed_since_clause(changed_since, "LAST_ALTERED", "CREATED"))
        rows = self.rows(f"""
            SELECT DATABASE_ID, DATABASE_NAME, COMMENT, DATABASE_OWNER AS OWNER_ROLE,
                   CREATED AS CREATED_AT, LAST_ALTERED, DELETED
            FROM SNOWFLAKE.ACCOUNT_USAGE.DATABASES
            WHERE {" AND ".join(clauses)}
            ORDER BY DATABASE_ID
        """)
        return [
            DatabaseRow.model_validate(_lower_keys(row))
            for row in rows
            if _included_database(row, config)
        ]

    def schemas(
        self, config: SnowflakeConfig, *, changed_since: str | None = None
    ) -> list[SchemaRow]:
        clauses = ["DELETED IS NULL"]
        if changed_since is not None:
            clauses.append(_changed_since_clause(changed_since, "LAST_ALTERED", "CREATED"))
        rows = self.rows(f"""
            SELECT SCHEMA_ID, CATALOG_NAME AS DATABASE_NAME, SCHEMA_NAME, COMMENT,
                   SCHEMA_OWNER AS OWNER_ROLE, CREATED AS CREATED_AT, LAST_ALTERED, DELETED
            FROM SNOWFLAKE.ACCOUNT_USAGE.SCHEMATA
            WHERE {" AND ".join(clauses)}
            ORDER BY SCHEMA_ID
        """)
        return [
            SchemaRow.model_validate(_lower_keys(row))
            for row in rows
            if _included_schema(row, config)
        ]

    def tables(
        self, config: SnowflakeConfig, *, changed_since: str | None = None
    ) -> list[TableRow]:
        clauses = ["DELETED IS NULL"]
        if changed_since is not None:
            cutoff = _quote_literal(changed_since)
            clauses.append(
                f"(LAST_DDL >= TO_TIMESTAMP_TZ('{cutoff}') "
                f"OR CREATED >= TO_TIMESTAMP_TZ('{cutoff}'))"
            )
        statement = f"""
            SELECT TABLE_ID, TABLE_CATALOG AS DATABASE_NAME, TABLE_SCHEMA AS SCHEMA_NAME,
                   TABLE_NAME AS OBJECT_NAME, TABLE_TYPE AS OBJECT_TYPE, COMMENT,
                   TABLE_OWNER AS OWNER_ROLE, CREATED AS CREATED_AT, LAST_DDL, DELETED,
                   IS_TRANSIENT, IS_ICEBERG, IS_DYNAMIC, IS_HYBRID
            FROM SNOWFLAKE.ACCOUNT_USAGE.TABLES
            WHERE {" AND ".join(clauses)}
            ORDER BY TABLE_ID
        """
        return [
            TableRow.model_validate(_lower_keys(row))
            for row in self.rows(statement)
            if _included_table(row, config)
        ]

    def deleted_external_ids(self, config: SnowflakeConfig, since: str | None) -> list[str]:
        if since is None:
            return []
        cutoff = _quote_literal(since)
        rows = self.rows(f"""
            SELECT 'database:' || DATABASE_ID AS EXTERNAL_ID
            FROM SNOWFLAKE.ACCOUNT_USAGE.DATABASES
            WHERE DELETED >= TO_TIMESTAMP_TZ('{cutoff}')
            UNION ALL
            SELECT 'schema:' || SCHEMA_ID AS EXTERNAL_ID
            FROM SNOWFLAKE.ACCOUNT_USAGE.SCHEMATA
            WHERE DELETED >= TO_TIMESTAMP_TZ('{cutoff}')
            UNION ALL
            SELECT 'table:' || TABLE_ID AS EXTERNAL_ID
            FROM SNOWFLAKE.ACCOUNT_USAGE.TABLES
            WHERE DELETED >= TO_TIMESTAMP_TZ('{cutoff}')
        """)
        return [
            external_id
            for row in rows
            if (external_id := _optional_text(row, "EXTERNAL_ID")) is not None
        ]

    def columns(self, table: TableRow) -> list[ColumnRow]:
        rows = self.rows(f"""
            SELECT TABLE_ID, COLUMN_NAME, ORDINAL_POSITION, DATA_TYPE, COMMENT,
                   IS_NULLABLE, COLUMN_DEFAULT AS DEFAULT_EXPRESSION,
                   IDENTITY_GENERATION, EXPRESSION
            FROM SNOWFLAKE.ACCOUNT_USAGE.COLUMNS
            WHERE TABLE_ID = {int(table.table_id)}
            ORDER BY ORDINAL_POSITION
        """)
        return [ColumnRow.model_validate(_lower_keys(_normalize_column(row))) for row in rows]

    def grants(self) -> list[SnowflakeGrant]:
        # GRANTS_TO_ROLES contains both role grants and direct user object
        # grants. GRANTS_TO_USERS is only the role-assignment view, so using it
        # here would miss direct grants and query columns that do not exist.
        rows = self.rows("""
            SELECT GRANTEE_NAME, GRANTED_TO AS GRANTEE_TYPE,
                   PRIVILEGE, GRANTED_ON AS OBJECT_TYPE, NAME AS OBJECT_NAME,
                   TABLE_CATALOG AS OBJECT_DATABASE, TABLE_SCHEMA AS OBJECT_SCHEMA,
                   DELETED_ON
            FROM SNOWFLAKE.ACCOUNT_USAGE.GRANTS_TO_ROLES
            WHERE DELETED_ON IS NULL
        """)
        return [SnowflakeGrant.model_validate(_lower_keys(row)) for row in rows]

    def users(self) -> list[SnowflakeUser]:
        return [
            SnowflakeUser.model_validate(_lower_keys(row))
            for row in self.rows("""
            SELECT NAME, EMAIL, DISABLED FROM SNOWFLAKE.ACCOUNT_USAGE.USERS
            WHERE DELETED_ON IS NULL
        """)
        ]

    def user_roles(self) -> list[SnowflakeUserRole]:
        return [
            SnowflakeUserRole.model_validate(_lower_keys(row))
            for row in self.rows("""
            SELECT GRANTEE_NAME AS USER_NAME, ROLE AS ROLE_NAME
            FROM SNOWFLAKE.ACCOUNT_USAGE.GRANTS_TO_USERS
            WHERE DELETED_ON IS NULL
        """)
        ]

    def role_edges(self) -> list[SnowflakeRoleEdge]:
        return [
            SnowflakeRoleEdge.model_validate(_lower_keys(row))
            for row in self.rows("""
            SELECT GRANTEE_NAME AS PARENT_ROLE, NAME AS CHILD_ROLE
            FROM SNOWFLAKE.ACCOUNT_USAGE.GRANTS_TO_ROLES
            WHERE GRANTED_ON IN ('ROLE', 'DATABASE_ROLE') AND DELETED_ON IS NULL
        """)
        ]

    def verify_identity(
        self, expected_account: str | None = None, expected_user: str | None = None
    ) -> tuple[str, str, str | None]:
        rows = self.rows(
            "SELECT CURRENT_ACCOUNT() AS ACCOUNT, CURRENT_USER() AS USER, "
            "CURRENT_REGION() AS REGION"
        )
        if len(rows) != 1:
            raise ValueError("Snowflake identity query returned an invalid response")
        row = rows[0]
        account = _required_text(row, "ACCOUNT")
        user = _required_text(row, "USER")
        region = _optional_text(row, "REGION")
        if expected_account is not None and account.casefold() != expected_account.casefold():
            raise ValueError("Snowflake credential belongs to a different account")
        if expected_user is not None and user.casefold() != expected_user.casefold():
            raise ValueError("Snowflake credential belongs to a different user")
        return account, user, region

    def user_email(self, user: str) -> str | None:
        escaped = _quote_literal(user)
        rows = self.rows(f"""
            SELECT EMAIL
            FROM SNOWFLAKE.ACCOUNT_USAGE.USERS
            WHERE NAME = '{escaped}' AND DELETED_ON IS NULL
        """)
        if len(rows) != 1:
            return None
        return _optional_text(rows[0], "EMAIL")


def _included_database(row: Mapping[str, object], config: SnowflakeConfig) -> bool:
    name = _optional_text(row, "DATABASE_NAME")
    return name is not None and any(name.casefold() == item.casefold() for item in config.databases)


def _included_schema(row: Mapping[str, object], config: SnowflakeConfig) -> bool:
    database = _optional_text(row, "DATABASE_NAME")
    schema = _optional_text(row, "SCHEMA_NAME")
    if database is None or schema is None or not _included_database(row, config):
        return False
    qualified = f"{database}.{schema}"
    if config.schemas_allowlist and not any(
        qualified.casefold() == item.casefold() for item in config.schemas_allowlist
    ):
        return False
    return not config.schemas_denylist or all(
        qualified.casefold() != item.casefold() for item in config.schemas_denylist
    )


def _included_table(row: Mapping[str, object], config: SnowflakeConfig) -> bool:
    object_type = _optional_text(row, "OBJECT_TYPE")
    if object_type is None or not _included_schema(row, config):
        return False
    configured_types = {item.casefold() for item in config.included_object_types}
    # Older saved configs used TABLE for Snowflake's BASE TABLE value.
    return object_type.casefold() in configured_types or (
        object_type.casefold() == "base table" and "table" in configured_types
    )


def _lower_keys(row: Mapping[str, object]) -> dict[str, object]:
    normalized = {key.lower(): value for key, value in row.items()}
    for key in ("database_id", "schema_id", "table_id"):
        value = normalized.get(key)
        if isinstance(value, int):
            normalized[key] = str(value)
    return normalized


def _normalize_column(row: Mapping[str, object]) -> dict[str, object]:
    normalized = dict(row)
    nullable = normalized.get("IS_NULLABLE")
    if isinstance(nullable, str):
        normalized["IS_NULLABLE"] = nullable.upper() == "YES"
    return normalized


def _optional_text(row: Mapping[str, object], key: str) -> str | None:
    value = row.get(key)
    return value if isinstance(value, str) and value else None


def _required_text(row: Mapping[str, object], key: str) -> str:
    value = _optional_text(row, key)
    if value is None:
        raise ValueError(f"Snowflake identity response omitted {key}")
    return value


def _quote_literal(value: str) -> str:
    return value.replace("'", "''")


def _changed_since_clause(cutoff: str, *columns: str) -> str:
    quoted = _quote_literal(cutoff)
    return "(" + " OR ".join(f"{column} >= TO_TIMESTAMP_TZ('{quoted}')" for column in columns) + ")"


def _quote_identifier(value: str) -> str:
    return '"' + value.replace('"', '""') + '"'


def _account_identifier(account_url: str) -> str:
    hostname = urlparse(account_url).hostname
    if hostname is None:
        raise ValueError("Snowflake account URL has no hostname")
    suffix = ".snowflakecomputing.com"
    return hostname[: -len(suffix)] if hostname.casefold().endswith(suffix) else hostname


def validate_mcp_endpoint(url: str, account_url: str) -> str:
    endpoint = urlparse(url)
    account = urlparse(account_url)
    if (
        endpoint.scheme != "https"
        or endpoint.username
        or endpoint.password
        or endpoint.query
        or endpoint.fragment
    ):
        raise ValueError("Snowflake MCP endpoint must be credential-free HTTPS")
    if endpoint.port not in (None, 443) or not endpoint.hostname:
        raise ValueError("Snowflake MCP endpoint has an invalid port or host")
    if account.hostname is None or endpoint.hostname.casefold() != account.hostname.casefold():
        raise ValueError("Snowflake MCP endpoint must belong to the configured account")
    if not re.fullmatch(r"/api/v2/databases/[^/]+/schemas/[^/]+/mcp-servers/[^/]+", endpoint.path):
        raise ValueError("Snowflake MCP endpoint must use the managed MCP server path")
    return url.rstrip("/")


def default_oauth_session_factory(config: SnowflakeConfig, access_token: str) -> SnowflakeSession:
    import snowflake.connector

    account = _account_identifier(config.account_url)
    connection_kwargs: dict[str, object] = {
        "account": account,
        "authenticator": "oauth",
        "token": access_token,
    }
    if config.warehouse is not None:
        connection_kwargs["warehouse"] = config.warehouse
    if config.role is not None:
        connection_kwargs["role"] = config.role
    connection = snowflake.connector.connect(**connection_kwargs)
    return _OfficialSnowflakeSession(connection)


def default_session_factory(
    config: SnowflakeConfig, credentials: SnowflakeCredentials
) -> SnowflakeSession:
    import snowflake.connector
    from cryptography.hazmat.primitives import serialization

    key = serialization.load_pem_private_key(
        credentials.private_key.encode(),
        password=credentials.private_key_passphrase.encode()
        if credentials.private_key_passphrase
        else None,
    )
    private_key = key.private_bytes(
        serialization.Encoding.DER,
        serialization.PrivateFormat.PKCS8,
        serialization.NoEncryption(),
    )
    account = _account_identifier(config.account_url)
    if config.warehouse is None or config.role is None:
        raise ValueError("metadata credentials require warehouse and role")
    connection = snowflake.connector.connect(
        account=account,
        user=credentials.username,
        private_key=private_key,
        warehouse=config.warehouse,
        role=config.role,
    )
    return _OfficialSnowflakeSession(connection)

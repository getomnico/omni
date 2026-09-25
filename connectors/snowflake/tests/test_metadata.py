from collections.abc import Iterable, Mapping
from datetime import UTC, datetime

from snowflake_connector.client import SnowflakeClient
from snowflake_connector.config import SnowflakeConfig
from snowflake_connector.models import (
    SnowflakeGrant,
    SnowflakeRoleEdge,
    SnowflakeUser,
    SnowflakeUserRole,
    TableRow,
)
from snowflake_connector.permissions import SnowflakePermissions


class Session:
    def __init__(self, rows: list[Mapping[str, object]]) -> None:
        self.rows = rows

    def execute(self, statement: str) -> Iterable[Mapping[str, object]]:
        return self.rows

    def close(self) -> None:
        return None


def config() -> SnowflakeConfig:
    return SnowflakeConfig(
        account_url="https://acme.snowflakecomputing.com",
        warehouse="OMNI_WH",
        role="OMNI_METADATA",
        databases=["ANALYTICS"],
    )


def test_metadata_id_and_timestamp_aliases_are_concrete() -> None:
    client = SnowflakeClient(
        Session(
            [
                {
                    "DATABASE_ID": 42,
                    "DATABASE_NAME": "ANALYTICS",
                    "COMMENT": None,
                    "OWNER_ROLE": "OWNER",
                    "CREATED_AT": None,
                    "LAST_ALTERED": None,
                    "DELETED": None,
                }
            ]
        )
    )
    databases = client.databases(config())
    assert databases[0].database_id == "42"


def test_role_inheritance_expands_assigned_roles_downward() -> None:
    users = [SnowflakeUser(name="alice", email="alice@example.com")]
    grants = [
        SnowflakeGrant(
            grantee_name="ANALYST",
            grantee_type="ACCOUNT ROLE",
            object_type="TABLE",
            object_name="TABLE_A",
            object_database="ANALYTICS",
            object_schema="PUBLIC",
        )
    ]
    edges = [SnowflakeRoleEdge(parent_role="SYSADMIN", child_role="ANALYST")]
    user_roles = [SnowflakeUserRole(user_name="alice", role_name="SYSADMIN")]
    permissions = SnowflakePermissions(users, grants, edges, user_roles)
    table = TableRow(
        table_id="1",
        database_name="ANALYTICS",
        schema_name="PUBLIC",
        object_name="TABLE_A",
        object_type="BASE TABLE",
    )

    direct_users, groups = permissions.permissions_for(table)
    memberships = permissions.group_members()
    assert direct_users == []
    assert groups == ["snowflake:role:analyst"]
    assert memberships["snowflake:role:analyst"] == ["alice@example.com"]


def test_grants_query_uses_account_usage_columns_and_direct_users() -> None:
    class GrantSession(Session):
        def execute(self, statement: str) -> Iterable[Mapping[str, object]]:
            assert "GRANTED_TO AS GRANTEE_TYPE" in statement
            assert "GRANTED_ON AS OBJECT_TYPE" in statement
            assert "TABLE_CATALOG AS OBJECT_DATABASE" in statement
            assert "FROM SNOWFLAKE.ACCOUNT_USAGE.GRANTS_TO_ROLES" in statement
            assert "GRANTS_TO_USERS" not in statement
            return [
                {
                    "GRANTEE_NAME": "ALICE",
                    "GRANTEE_TYPE": "USER",
                    "PRIVILEGE": "SELECT",
                    "OBJECT_TYPE": "TABLE",
                    "OBJECT_NAME": "TABLE_A",
                    "OBJECT_DATABASE": "ANALYTICS",
                    "OBJECT_SCHEMA": "PUBLIC",
                    "DELETED_ON": None,
                }
            ]

    grants = SnowflakeClient(GrantSession([])).grants()
    assert grants[0].grantee_type == "USER"
    assert grants[0].object_database == "ANALYTICS"


def test_table_query_accepts_base_table_without_nonexistent_columns() -> None:
    class TableSession(Session):
        def execute(self, statement: str) -> Iterable[Mapping[str, object]]:
            assert "IS_MATERIALIZED" not in statement
            assert "TABLE_TYPE AS OBJECT_TYPE" in statement
            return [
                {
                    "TABLE_ID": 100,
                    "DATABASE_NAME": "ANALYTICS",
                    "SCHEMA_NAME": "PUBLIC",
                    "OBJECT_NAME": "TABLE_A",
                    "OBJECT_TYPE": "BASE TABLE",
                    "CREATED_AT": None,
                    "LAST_DDL": None,
                    "DELETED": None,
                    "IS_TRANSIENT": False,
                    "IS_ICEBERG": False,
                    "IS_DYNAMIC": False,
                    "IS_HYBRID": False,
                    "IS_EVENT": False,
                }
            ]

    tables = SnowflakeClient(TableSession([])).tables(config())
    assert tables[0].object_type == "BASE TABLE"


def test_role_edge_query_uses_name_for_granted_child_role() -> None:
    class EdgeSession(Session):
        def execute(self, statement: str) -> Iterable[Mapping[str, object]]:
            assert "NAME AS CHILD_ROLE" in statement
            assert "GRANTED_ON IN ('ROLE', 'DATABASE_ROLE')" in statement
            return [{"PARENT_ROLE": "SYSADMIN", "CHILD_ROLE": "ANALYST"}]

    edges = SnowflakeClient(EdgeSession([])).role_edges()
    assert edges[0].parent_role == "SYSADMIN"
    assert edges[0].child_role == "ANALYST"


def test_direct_user_grants_and_revocations_are_conservative() -> None:
    users = [SnowflakeUser(name="alice", email="alice@example.com")]
    table = TableRow(
        table_id="1",
        database_name="ANALYTICS",
        schema_name="PUBLIC",
        object_name="TABLE_A",
        object_type="BASE TABLE",
    )
    permissions = SnowflakePermissions(
        users,
        [
            SnowflakeGrant(
                grantee_name="ALICE",
                grantee_type="USER",
                privilege="SELECT",
                object_type="TABLE",
                object_name="TABLE_A",
                object_database="ANALYTICS",
                object_schema="PUBLIC",
            ),
            SnowflakeGrant(
                grantee_name="ALICE",
                grantee_type="USER",
                privilege="SELECT",
                object_type="TABLE",
                object_name="OTHER",
                object_database="ANALYTICS",
                object_schema="PUBLIC",
                deleted_on=datetime(2026, 1, 1, tzinfo=UTC),
            ),
        ],
        [],
        [],
    )
    assert permissions.permissions_for(table) == (["alice@example.com"], [])


def test_unrelated_parent_grant_does_not_grant_table_visibility() -> None:
    users = [SnowflakeUser(name="alice", email="alice@example.com")]
    permissions = SnowflakePermissions(
        users,
        [
            SnowflakeGrant(
                grantee_name="ANALYST",
                grantee_type="ACCOUNT ROLE",
                privilege="USAGE",
                object_type="DATABASE",
                object_name="OTHER_DB",
            )
        ],
        [],
        [],
    )
    table = TableRow(
        table_id="1",
        database_name="ANALYTICS",
        schema_name="PUBLIC",
        object_name="TABLE_A",
        object_type="BASE TABLE",
    )
    assert permissions.permissions_for(table) == ([], [])

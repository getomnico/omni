from collections.abc import Iterable, Mapping

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
            grantee_name="ANALYST", grantee_type="ROLE", object_name="ANALYTICS.PUBLIC.TABLE_A"
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
        object_type="TABLE",
    )

    direct_users, groups = permissions.permissions_for(table)
    memberships = permissions.group_members()
    assert direct_users == []
    assert groups == ["snowflake:role:analyst"]
    assert memberships["snowflake:role:analyst"] == ["alice@example.com"]

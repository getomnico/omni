from __future__ import annotations

from collections import defaultdict

from .models import (
    SnowflakeGrant,
    SnowflakeRoleEdge,
    SnowflakeUser,
    SnowflakeUserRole,
    TableRow,
)


class SnowflakePermissions:
    """Conservative role and direct-user visibility mapping."""

    def __init__(
        self,
        users: list[SnowflakeUser],
        grants: list[SnowflakeGrant],
        edges: list[SnowflakeRoleEdge],
        user_roles: list[SnowflakeUserRole],
    ) -> None:
        self._users = {
            user.name.casefold(): user for user in users if not user.disabled and user.email
        }
        self._grants = grants
        self._user_roles = user_roles
        self._children: dict[str, set[str]] = defaultdict(set)
        for edge in edges:
            self._children[edge.parent_role.casefold()].add(edge.child_role.casefold())

    def permissions_for(self, table: TableRow) -> tuple[list[str], list[str]]:
        object_name = f"{table.database_name}.{table.schema_name}.{table.object_name}".casefold()
        role_names: set[str] = set()
        direct_users: set[str] = set()
        for grant in self._grants:
            if grant.deleted_on is not None or not self._grant_matches(grant, object_name):
                continue
            if grant.grantee_type.casefold() == "role":
                role_names.update(self._descendants(grant.grantee_name))
            elif grant.grantee_type.casefold() == "user":
                direct_users.add(grant.grantee_name.casefold())

        groups = [f"snowflake:role:{role}" for role in sorted(role_names)]
        emails = [
            self._users[user].email
            for user in direct_users
            if user in self._users and self._users[user].email is not None
        ]
        return sorted([email for email in emails if email is not None]), groups

    def group_members(self) -> dict[str, list[str]]:
        memberships: dict[str, set[str]] = {"snowflake:role:public": set()}
        for user in self._users.values():
            if user.email is not None:
                memberships["snowflake:role:public"].add(user.email)
        for assignment in self._user_roles:
            assigned_user = self._users.get(assignment.user_name.casefold())
            if assigned_user is None or assigned_user.email is None:
                continue
            for role in self._descendants(assignment.role_name):
                memberships.setdefault(f"snowflake:role:{role}", set()).add(assigned_user.email)
        return {group: sorted(members) for group, members in memberships.items()}

    def _descendants(self, role: str) -> set[str]:
        pending = [role.casefold()]
        result: set[str] = set()
        while pending:
            current = pending.pop()
            if current in result:
                continue
            result.add(current)
            pending.extend(self._children.get(current, ()))
        return result

    @staticmethod
    def _grant_matches(grant: SnowflakeGrant, object_name: str) -> bool:
        if grant.object_name is None:
            return False
        candidate = grant.object_name.casefold()
        return (
            candidate == object_name
            or candidate == object_name.rsplit(".", 1)[-1]
            or candidate == "*"
        )

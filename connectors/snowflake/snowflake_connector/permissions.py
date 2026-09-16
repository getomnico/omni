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
        return self._permissions_for_names(
            [
                table.database_name,
                f"{table.database_name}.{table.schema_name}",
                f"{table.database_name}.{table.schema_name}.{table.object_name}",
            ]
        )

    def permissions_for_database(self, database_name: str) -> tuple[list[str], list[str]]:
        return self._permissions_for_names([database_name])

    def permissions_for_schema(
        self, database_name: str, schema_name: str
    ) -> tuple[list[str], list[str]]:
        return self._permissions_for_names([database_name, f"{database_name}.{schema_name}"])

    def _permissions_for_names(self, object_names: list[str]) -> tuple[list[str], list[str]]:
        role_names: set[str] = set()
        direct_users: set[str] = set()
        for grant in self._grants:
            if grant.deleted_on is not None or not self._grant_matches(grant, object_names):
                continue
            if grant.grantee_type.casefold() == "role":
                # Group membership expands an assigned role into its granted
                # child roles, so a document only needs the role receiving
                # the object grant. Including ancestors here would overgrant
                # users whose role does not inherit this grant.
                role_names.add(grant.grantee_name.casefold())
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
    def _grant_matches(grant: SnowflakeGrant, object_names: list[str]) -> bool:
        if grant.object_name is None:
            return False
        candidate = grant.object_name.casefold()
        targets = {name.casefold() for name in object_names}
        return candidate in targets or any(
            candidate.endswith(".*") and target.startswith(candidate[:-1]) for target in targets
        )

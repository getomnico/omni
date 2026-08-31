"""Resolution of Salesforce sharing semantics into Omni document permissions.

Salesforce visibility is granted through four mechanisms, mirrored here:

1. Record ownership — the owner (user) is granted; when the owner is a queue,
   the queue's members are granted.
2. Role hierarchy — with "Grant Access Using Hierarchies" enabled, users in the
   owner's role and every ancestor role can see the record.
3. Sharing rules / manual shares — rows in the per-object *Share tables grant
   access to a user, a public group, or a role (role shares include all roles
   below it in the hierarchy).
4. Org-wide defaults — objects configured as public-read grant everyone.

Group emails are synthesized from Salesforce ids because groups and roles have
no email addresses; Omni matches these opaque strings exactly.
"""

from __future__ import annotations

from collections.abc import Iterable
from dataclasses import dataclass, field

from omni_connector import DocumentPermissions

from .models import (
    GroupMemberRecord,
    GroupRecord,
    RoleRecord,
    ShareRecord,
    UserRecord,
    group_email,
    role_email,
)

# Salesforce id prefixes for UserOrGroupId targets.
USER_ID_PREFIX = "005"
GROUP_ID_PREFIX = "00G"
ROLE_ID_PREFIX = "00E"


@dataclass(frozen=True)
class RecordGrants:
    """Resolved permission grants for one record."""

    users: tuple[str, ...] = ()
    groups: tuple[str, ...] = ()

    def merge(self, other: RecordGrants) -> RecordGrants:
        return RecordGrants(
            users=tuple(sorted(set(self.users) | set(other.users))),
            groups=tuple(sorted(set(self.groups) | set(other.groups))),
        )

    def to_permissions(self, public: bool) -> DocumentPermissions:
        return DocumentPermissions(
            public=public,
            users=list(self.users),
            groups=list(self.groups),
        )


@dataclass
class SalesforceDirectory:
    """In-memory org directory built during the people phase of a sync."""

    users_by_id: dict[str, UserRecord] = field(default_factory=dict)
    groups_by_id: dict[str, GroupRecord] = field(default_factory=dict)
    group_members_by_id: dict[str, list[GroupMemberRecord]] = field(default_factory=dict)
    roles_by_id: dict[str, RoleRecord] = field(default_factory=dict)
    users_by_role: dict[str, list[UserRecord]] = field(default_factory=dict)

    def email_for_user(self, user_id: str) -> str | None:
        user = self.users_by_id.get(user_id)
        if user is None or not user.email:
            return None
        return user.email

    def role_of(self, user_id: str) -> str | None:
        user = self.users_by_id.get(user_id)
        if user is None:
            return None
        return user.user_role_id

    def _expand_group(self, group_id: str, seen: set[str]) -> set[str]:
        """Expand a group to member emails, following nested groups and roles
        with cycle protection. Roles inside a group contribute their direct
        members."""
        if group_id in seen:
            return set()
        seen.add(group_id)
        emails: set[str] = set()
        for member in self.group_members_by_id.get(group_id, ()):
            target = member.user_or_group_id
            if target.startswith(USER_ID_PREFIX):
                email = self.email_for_user(target)
                if email:
                    emails.add(email)
            elif target.startswith(GROUP_ID_PREFIX):
                emails.update(self._expand_group(target, seen))
            elif target.startswith(ROLE_ID_PREFIX):
                for user in self.users_by_role.get(target, ()):
                    if user.email:
                        emails.add(user.email)
        return emails

    def group_member_emails(self, group_id: str) -> set[str]:
        return self._expand_group(group_id, set())

    def _role_and_descendant_emails(self, role_id: str, seen: set[str]) -> set[str]:
        """Emails of everyone in the role and all roles below it."""
        if role_id in seen:
            return set()
        seen.add(role_id)
        emails = {u.email for u in self.users_by_role.get(role_id, ()) if u.email}
        for other_id, role in self.roles_by_id.items():
            if role.parent_role_id == role_id:
                emails.update(self._role_and_descendant_emails(other_id, seen))
        return emails

    def role_and_descendants(self, role_id: str) -> set[str]:
        return self._role_and_descendant_emails(role_id, set())

    def _ancestor_roles(self, role_id: str) -> list[str]:
        """The role and every ancestor role up the hierarchy."""
        chain: list[str] = []
        current: str | None = role_id
        seen: set[str] = set()
        while current is not None and current not in seen:
            chain.append(current)
            seen.add(current)
            role = self.roles_by_id.get(current)
            current = role.parent_role_id if role is not None else None
        return chain

    def owner_grants(self, owner_id: str | None, include_hierarchy: bool = True) -> RecordGrants:
        """Grants derived from record ownership."""
        if owner_id is None:
            return RecordGrants()
        if owner_id.startswith(GROUP_ID_PREFIX):
            # Queue-owned record: visible to the queue's members via the
            # queue's synced group membership.
            return RecordGrants(groups=(group_email(owner_id),))
        email = self.email_for_user(owner_id)
        users: list[str] = [email] if email else []
        groups: list[str] = []
        role_id = self.role_of(owner_id)
        if role_id is not None and include_hierarchy:
            # Role hierarchy: the owner's role and every ancestor role. Each
            # direct-role group contains only that role's own members, so a
            # record is visible exactly to the owner's peers and managers —
            # not to peers of managers (who are not on this record's chain).
            groups.extend(role_email(role) for role in self._ancestor_roles(role_id))
        return RecordGrants(users=tuple(users), groups=tuple(groups))

    def share_grants(self, shares: Iterable[ShareRecord]) -> RecordGrants:
        """Grants derived from share rows (sharing rules + manual shares)."""
        users: set[str] = set()
        groups: set[str] = set()
        for share in shares:
            if share.access_level in ("None", "ReadOnly", None):
                continue
            target = share.user_or_group_id
            if target.startswith(USER_ID_PREFIX):
                email = self.email_for_user(target)
                if email:
                    users.add(email)
            elif target.startswith(GROUP_ID_PREFIX):
                groups.add(group_email(target))
            elif target.startswith(ROLE_ID_PREFIX):
                # Sharing to a role grants the role and everything below it.
                groups.add(role_email(target))
        return RecordGrants(users=tuple(sorted(users)), groups=tuple(sorted(groups)))

    def group_memberships(self) -> list[tuple[str, set[str], str | None]]:
        """(group_email, member_emails, display_name) for every group and role."""
        memberships: list[tuple[str, set[str], str | None]] = []
        for group_id, group in self.groups_by_id.items():
            memberships.append(
                (group_email(group_id), self.group_member_emails(group_id), group.name)
            )
        for role_id, role in self.roles_by_id.items():
            # The role group (role + descendants) backs role-based shares; the
            # direct group backs owner hierarchy grants.
            memberships.append((role_email(role_id), self.role_and_descendants(role_id), role.name))
        return memberships


def build_directory(
    users: Iterable[UserRecord],
    groups: Iterable[GroupRecord],
    group_members: Iterable[GroupMemberRecord],
    roles: Iterable[RoleRecord],
) -> SalesforceDirectory:
    directory = SalesforceDirectory()
    for user in users:
        directory.users_by_id[user.id] = user
    for group in groups:
        directory.groups_by_id[group.id] = group
    for member in group_members:
        directory.group_members_by_id.setdefault(member.group_id, []).append(member)
    for role in roles:
        directory.roles_by_id[role.id] = role
    for user in users:
        if user.user_role_id is not None:
            directory.users_by_role.setdefault(user.user_role_id, []).append(user)
    return directory

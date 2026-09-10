"""Typed representations of Salesforce objects and connector state."""

from __future__ import annotations

import hashlib
import json
from collections.abc import Mapping
from dataclasses import dataclass, field, replace
from datetime import datetime
from enum import StrEnum

from .config import REALTIME_POLL_SECONDS, SyncRunMode, validate_object_names

CHECKPOINT_VERSION = 2
# Group emails are synthesized from Salesforce group/role ids because Salesforce
# groups have no email address of their own. The suffix is opaque; Omni matches
# these exact strings.
GROUP_EMAIL_SUFFIX = "@salesforce.groups"
ROLE_EMAIL_SUFFIX = "@salesforce.roles"
# Salesforce grants hierarchy access to ancestor roles. Those grants must not
# include other users in the same role (peers) or subordinates, so they use a
# distinct synthetic group that only ever holds a role's direct members.
DIRECT_ROLE_EMAIL_SUFFIX = "@salesforce.role_direct"


def _as_str(value: object) -> str | None:
    if value is None:
        return None
    if isinstance(value, str):
        return value if value else None
    raise ValueError(f"expected string, got {type(value).__name__}: {value!r}")


def _as_required_str(value: object, field_name: str) -> str:
    parsed = _as_str(value)
    if parsed is None:
        raise ValueError(f"missing required field {field_name}")
    return parsed


def _as_bool(value: object) -> bool | None:
    if value is None:
        return None
    if isinstance(value, bool):
        return value
    raise ValueError(f"expected boolean, got {type(value).__name__}: {value!r}")


def _as_float(value: object) -> float | None:
    if value is None:
        return None
    if isinstance(value, (int, float)):
        return float(value)
    raise ValueError(f"expected number, got {type(value).__name__}: {value!r}")


def _as_datetime(value: object) -> datetime | None:
    if value is None:
        return None
    if isinstance(value, datetime):
        return value
    if isinstance(value, str):
        try:
            return datetime.fromisoformat(value.replace("Z", "+00:00"))
        except ValueError as e:
            raise ValueError(f"invalid timestamp {value!r}") from e
    raise ValueError(f"expected timestamp, got {type(value).__name__}: {value!r}")


def _nested_name(value: object) -> str | None:
    """Extract a name from a relationship subquery result (e.g. Account.Name)."""
    if value is None:
        return None
    if isinstance(value, Mapping):
        return _as_str(value.get("Name"))
    raise ValueError(f"expected relationship object, got {type(value).__name__}: {value!r}")


def group_email(group_id: str) -> str:
    return f"{group_id}{GROUP_EMAIL_SUFFIX}"


def role_email(role_id: str) -> str:
    return f"{role_id}{ROLE_EMAIL_SUFFIX}"


def direct_role_email(role_id: str) -> str:
    return f"{role_id}{DIRECT_ROLE_EMAIL_SUFFIX}"


@dataclass(frozen=True)
class AccountRecord:
    id: str
    name: str | None
    industry: str | None
    phone: str | None
    website: str | None
    billing_city: str | None
    billing_state: str | None
    billing_country: str | None
    number_of_employees: float | None
    annual_revenue: float | None
    description: str | None
    type: str | None
    owner_id: str | None
    created_date: datetime | None
    system_modstamp: datetime | None

    @classmethod
    def from_record(cls, raw: Mapping[str, object]) -> AccountRecord:
        return cls(
            id=_as_required_str(raw.get("Id"), "Id"),
            name=_as_str(raw.get("Name")),
            industry=_as_str(raw.get("Industry")),
            phone=_as_str(raw.get("Phone")),
            website=_as_str(raw.get("Website")),
            billing_city=_as_str(raw.get("BillingCity")),
            billing_state=_as_str(raw.get("BillingState")),
            billing_country=_as_str(raw.get("BillingCountry")),
            number_of_employees=_as_float(raw.get("NumberOfEmployees")),
            annual_revenue=_as_float(raw.get("AnnualRevenue")),
            description=_as_str(raw.get("Description")),
            type=_as_str(raw.get("Type")),
            owner_id=_as_str(raw.get("OwnerId")),
            created_date=_as_datetime(raw.get("CreatedDate")),
            system_modstamp=_as_datetime(raw.get("SystemModstamp")),
        )


@dataclass(frozen=True)
class ContactRecord:
    id: str
    name: str | None
    first_name: str | None
    last_name: str | None
    email: str | None
    phone: str | None
    title: str | None
    department: str | None
    account_id: str | None
    account_name: str | None
    mailing_city: str | None
    mailing_state: str | None
    mailing_country: str | None
    owner_id: str | None
    created_date: datetime | None
    system_modstamp: datetime | None

    @classmethod
    def from_record(cls, raw: Mapping[str, object]) -> ContactRecord:
        return cls(
            id=_as_required_str(raw.get("Id"), "Id"),
            name=_as_str(raw.get("Name")),
            first_name=_as_str(raw.get("FirstName")),
            last_name=_as_str(raw.get("LastName")),
            email=_as_str(raw.get("Email")),
            phone=_as_str(raw.get("Phone")),
            title=_as_str(raw.get("Title")),
            department=_as_str(raw.get("Department")),
            account_id=_as_str(raw.get("AccountId")),
            account_name=_nested_name(raw.get("Account")),
            mailing_city=_as_str(raw.get("MailingCity")),
            mailing_state=_as_str(raw.get("MailingState")),
            mailing_country=_as_str(raw.get("MailingCountry")),
            owner_id=_as_str(raw.get("OwnerId")),
            created_date=_as_datetime(raw.get("CreatedDate")),
            system_modstamp=_as_datetime(raw.get("SystemModstamp")),
        )


@dataclass(frozen=True)
class OpportunityRecord:
    id: str
    name: str | None
    amount: float | None
    stage_name: str | None
    close_date: datetime | None
    probability: float | None
    type: str | None
    lead_source: str | None
    description: str | None
    account_id: str | None
    account_name: str | None
    owner_id: str | None
    created_date: datetime | None
    system_modstamp: datetime | None

    @classmethod
    def from_record(cls, raw: Mapping[str, object]) -> OpportunityRecord:
        return cls(
            id=_as_required_str(raw.get("Id"), "Id"),
            name=_as_str(raw.get("Name")),
            amount=_as_float(raw.get("Amount")),
            stage_name=_as_str(raw.get("StageName")),
            close_date=_as_datetime(raw.get("CloseDate")),
            probability=_as_float(raw.get("Probability")),
            type=_as_str(raw.get("Type")),
            lead_source=_as_str(raw.get("LeadSource")),
            description=_as_str(raw.get("Description")),
            account_id=_as_str(raw.get("AccountId")),
            account_name=_nested_name(raw.get("Account")),
            owner_id=_as_str(raw.get("OwnerId")),
            created_date=_as_datetime(raw.get("CreatedDate")),
            system_modstamp=_as_datetime(raw.get("SystemModstamp")),
        )


@dataclass(frozen=True)
class LeadRecord:
    id: str
    name: str | None
    first_name: str | None
    last_name: str | None
    email: str | None
    phone: str | None
    company: str | None
    title: str | None
    industry: str | None
    status: str | None
    lead_source: str | None
    description: str | None
    owner_id: str | None
    created_date: datetime | None
    system_modstamp: datetime | None

    @classmethod
    def from_record(cls, raw: Mapping[str, object]) -> LeadRecord:
        return cls(
            id=_as_required_str(raw.get("Id"), "Id"),
            name=_as_str(raw.get("Name")),
            first_name=_as_str(raw.get("FirstName")),
            last_name=_as_str(raw.get("LastName")),
            email=_as_str(raw.get("Email")),
            phone=_as_str(raw.get("Phone")),
            company=_as_str(raw.get("Company")),
            title=_as_str(raw.get("Title")),
            industry=_as_str(raw.get("Industry")),
            status=_as_str(raw.get("Status")),
            lead_source=_as_str(raw.get("LeadSource")),
            description=_as_str(raw.get("Description")),
            owner_id=_as_str(raw.get("OwnerId")),
            created_date=_as_datetime(raw.get("CreatedDate")),
            system_modstamp=_as_datetime(raw.get("SystemModstamp")),
        )


@dataclass(frozen=True)
class CaseRecord:
    id: str
    case_number: str | None
    subject: str | None
    description: str | None
    status: str | None
    priority: str | None
    type: str | None
    origin: str | None
    contact_id: str | None
    account_id: str | None
    account_name: str | None
    owner_id: str | None
    created_date: datetime | None
    system_modstamp: datetime | None

    @classmethod
    def from_record(cls, raw: Mapping[str, object]) -> CaseRecord:
        return cls(
            id=_as_required_str(raw.get("Id"), "Id"),
            case_number=_as_str(raw.get("CaseNumber")),
            subject=_as_str(raw.get("Subject")),
            description=_as_str(raw.get("Description")),
            status=_as_str(raw.get("Status")),
            priority=_as_str(raw.get("Priority")),
            type=_as_str(raw.get("Type")),
            origin=_as_str(raw.get("Origin")),
            contact_id=_as_str(raw.get("ContactId")),
            account_id=_as_str(raw.get("AccountId")),
            account_name=_nested_name(raw.get("Account")),
            owner_id=_as_str(raw.get("OwnerId")),
            created_date=_as_datetime(raw.get("CreatedDate")),
            system_modstamp=_as_datetime(raw.get("SystemModstamp")),
        )


@dataclass(frozen=True)
class TaskRecord:
    id: str
    subject: str | None
    description: str | None
    status: str | None
    priority: str | None
    activity_date: datetime | None
    who_id: str | None
    what_id: str | None
    owner_id: str | None
    created_date: datetime | None
    system_modstamp: datetime | None

    @classmethod
    def from_record(cls, raw: Mapping[str, object]) -> TaskRecord:
        return cls(
            id=_as_required_str(raw.get("Id"), "Id"),
            subject=_as_str(raw.get("Subject")),
            description=_as_str(raw.get("Description")),
            status=_as_str(raw.get("Status")),
            priority=_as_str(raw.get("Priority")),
            activity_date=_as_datetime(raw.get("ActivityDate")),
            who_id=_as_str(raw.get("WhoId")),
            what_id=_as_str(raw.get("WhatId")),
            owner_id=_as_str(raw.get("OwnerId")),
            created_date=_as_datetime(raw.get("CreatedDate")),
            system_modstamp=_as_datetime(raw.get("SystemModstamp")),
        )


@dataclass(frozen=True)
class UserRecord:
    id: str
    name: str | None
    first_name: str | None
    last_name: str | None
    email: str | None
    title: str | None
    department: str | None
    manager_id: str | None
    user_role_id: str | None
    is_active: bool | None
    employee_number: str | None
    system_modstamp: datetime | None

    @classmethod
    def from_record(cls, raw: Mapping[str, object]) -> UserRecord:
        return cls(
            id=_as_required_str(raw.get("Id"), "Id"),
            name=_as_str(raw.get("Name")),
            first_name=_as_str(raw.get("FirstName")),
            last_name=_as_str(raw.get("LastName")),
            email=_as_str(raw.get("Email")),
            title=_as_str(raw.get("Title")),
            department=_as_str(raw.get("Department")),
            manager_id=_as_str(raw.get("ManagerId")),
            user_role_id=_as_str(raw.get("UserRoleId")),
            is_active=_as_bool(raw.get("IsActive")),
            employee_number=_as_str(raw.get("EmployeeNumber")),
            system_modstamp=_as_datetime(raw.get("SystemModstamp")),
        )


class GroupType(StrEnum):
    """Salesforce Group.Type values used for share resolution."""

    PUBLIC = "Public"
    QUEUE = "Queue"
    REGULAR = "Regular"
    ROLE = "Role"
    ROLE_AND_SUBORDINATES = "RoleAndSubordinates"
    ROLE_AND_SUBORDINATES_INTERNAL = "RoleAndSubordinatesInternal"

    @classmethod
    def parse(cls, value: object) -> GroupType | None:
        parsed = _as_str(value)
        if parsed is None:
            return None
        try:
            return cls(parsed)
        except ValueError:
            return None

    @property
    def is_role_group(self) -> bool:
        return self in {
            GroupType.ROLE,
            GroupType.ROLE_AND_SUBORDINATES,
            GroupType.ROLE_AND_SUBORDINATES_INTERNAL,
        }


@dataclass(frozen=True)
class GroupRecord:
    id: str
    name: str | None
    type: GroupType | None
    # Salesforce-generated 00G role groups point at their role via RelatedId.
    related_id: str | None

    @classmethod
    def from_record(cls, raw: Mapping[str, object]) -> GroupRecord:
        return cls(
            id=_as_required_str(raw.get("Id"), "Id"),
            name=_as_str(raw.get("Name")),
            type=GroupType.parse(raw.get("Type")),
            related_id=_as_str(raw.get("RelatedId")),
        )


@dataclass(frozen=True)
class GroupMemberRecord:
    id: str
    group_id: str
    user_or_group_id: str

    @classmethod
    def from_record(cls, raw: Mapping[str, object]) -> GroupMemberRecord:
        return cls(
            id=_as_required_str(raw.get("Id"), "Id"),
            group_id=_as_required_str(raw.get("GroupId"), "GroupId"),
            user_or_group_id=_as_required_str(raw.get("UserOrGroupId"), "UserOrGroupId"),
        )


@dataclass(frozen=True)
class RoleRecord:
    id: str
    name: str | None
    parent_role_id: str | None

    @classmethod
    def from_record(cls, raw: Mapping[str, object]) -> RoleRecord:
        return cls(
            id=_as_required_str(raw.get("Id"), "Id"),
            name=_as_str(raw.get("Name")),
            parent_role_id=_as_str(raw.get("ParentRoleId")),
        )


@dataclass(frozen=True)
class AccessLevel:
    """A validated Salesforce share access level.

    Only the explicit no-access value is treated as a non-grant; every other
    value (including future read-capable levels) grants access so a provider
    addition cannot silently hide a document from an authorized user.
    """

    value: str

    @classmethod
    def parse(cls, value: object) -> AccessLevel | None:
        parsed = _as_str(value)
        return cls(parsed) if parsed is not None else None

    @property
    def grants_access(self) -> bool:
        return self.value.strip().lower() != "none"


@dataclass(frozen=True)
class RowCause:
    """A validated Salesforce share RowCause value."""

    value: str

    @classmethod
    def parse(cls, value: object) -> RowCause | None:
        parsed = _as_str(value)
        return cls(parsed) if parsed is not None else None


@dataclass(frozen=True)
class ShareRecord:
    id: str
    parent_id: str
    user_or_group_id: str
    access_level: AccessLevel | None
    row_cause: RowCause | None

    @classmethod
    def from_record(
        cls, raw: Mapping[str, object], parent_field: str, access_level_field: str
    ) -> ShareRecord:
        return cls(
            id=_as_required_str(raw.get("Id"), "Id"),
            parent_id=_as_required_str(raw.get(parent_field), parent_field),
            user_or_group_id=_as_required_str(raw.get("UserOrGroupId"), "UserOrGroupId"),
            access_level=AccessLevel.parse(raw.get(access_level_field)),
            row_cause=RowCause.parse(raw.get("RowCause")),
        )


class AuthMode(StrEnum):
    """How the connector authenticates against Salesforce."""

    BEARER = "bearer"
    JWT = "jwt"


@dataclass(frozen=True)
class SalesforceAuth:
    """Typed authentication settings, decoded from the service credentials.

    Two modes, chosen by which fields are present:
    - bearer: a static ``access_token`` (plus ``instance_url``) pasted into the
      web UI. Fine for a test session, but tokens expire.
    - jwt: ``client_id`` (connected-app consumer key) + ``private_key`` (PEM)
      + ``username``; the client mints its own short-lived access tokens via
      the OAuth 2.0 JWT bearer flow and refreshes them automatically.
    """

    mode: AuthMode
    access_token: str | None = None
    instance_url: str | None = None
    client_id: str | None = None
    private_key: str | None = None
    username: str | None = None
    login_url: str = "https://login.salesforce.com"

    @classmethod
    def from_mapping(cls, raw: Mapping[str, object] | None) -> SalesforceAuth:
        if raw is None:
            raise ValueError("Missing credentials")
        # Connector-manager may merge org setup fields (including a JWT
        # client id) into a user's OAuth credential. Prefer the user access
        # token whenever one is present so MCP cannot silently fall back to the
        # org JWT.
        access_token = _as_str(raw.get("access_token"))
        if access_token is not None:
            return cls(
                mode=AuthMode.BEARER,
                access_token=access_token,
                instance_url=_as_str(raw.get("instance_url")),
            )

        client_id = _as_str(raw.get("client_id"))
        private_key = _as_str(raw.get("private_key"))
        username = _as_str(raw.get("username"))
        if client_id is not None and private_key is not None and username is not None:
            login_url = _as_str(raw.get("login_url")) or "https://login.salesforce.com"
            return cls(
                mode=AuthMode.JWT,
                client_id=client_id,
                private_key=private_key,
                username=username,
                login_url=login_url,
            )
        if access_token is None:
            raise ValueError(
                "Missing credentials: provide access_token (+ instance_url), "
                "or client_id + private_key + username for JWT auth"
            )
        return cls(
            mode=AuthMode.BEARER,
            access_token=access_token,
            instance_url=_as_str(raw.get("instance_url")),
        )


@dataclass(frozen=True)
class SalesforceSourceConfig:
    """Typed source configuration, decoded from the source config mapping."""

    instance_url: str | None = None
    enabled_objects: frozenset[str] = frozenset()
    public_read_objects: frozenset[str] = frozenset()
    grant_access_using_hierarchies: bool = True
    sync_users: bool = True
    sync_groups: bool = True
    sync_shares: bool = True
    realtime_poll_seconds: int = REALTIME_POLL_SECONDS

    @classmethod
    def from_mapping(cls, raw: Mapping[str, object] | None) -> SalesforceSourceConfig:
        if raw is None:
            return cls()
        instance_url = _as_str(raw.get("instance_url"))
        enabled = _string_set(raw.get("enabled_objects"))
        public_read = _string_set(raw.get("public_read_objects"))
        validate_object_names(enabled, "enabled_objects")
        validate_object_names(public_read, "public_read_objects")
        return cls(
            instance_url=instance_url,
            enabled_objects=frozenset(enabled),
            public_read_objects=frozenset(public_read),
            grant_access_using_hierarchies=_bool_or(raw, "grant_access_using_hierarchies", True),
            sync_users=_bool_or(raw, "sync_users", True),
            sync_groups=_bool_or(raw, "sync_groups", True),
            sync_shares=_bool_or(raw, "sync_shares", True),
            realtime_poll_seconds=_int_or(raw, "realtime_poll_seconds", REALTIME_POLL_SECONDS),
        )

    def validate(self) -> None:
        """Reject settings that cannot be resolved without user data."""
        if not self.sync_users and (self.sync_groups or self.sync_shares):
            raise ValueError(
                "sync_groups/sync_shares require sync_users: without user data "
                "group and share memberships cannot be resolved"
            )
        if not self.sync_users and self.grant_access_using_hierarchies:
            raise ValueError(
                "grant_access_using_hierarchies requires sync_users: owner roles "
                "cannot be resolved without user data"
            )


def _string_set(value: object) -> frozenset[str]:
    if value is None:
        return frozenset()
    if isinstance(value, list):
        items = []
        for item in value:
            parsed = _as_str(item)
            if parsed is not None:
                items.append(parsed)
        return frozenset(items)
    raise ValueError(f"expected list of strings, got {type(value).__name__}: {value!r}")


def _bool_or(raw: Mapping[str, object], key: str, default: bool) -> bool:
    value = raw.get(key)
    if value is None:
        return default
    parsed = _as_bool(value)
    return default if parsed is None else parsed


def _int_or(raw: Mapping[str, object], key: str, default: int) -> int:
    value = raw.get(key)
    if value is None:
        return default
    parsed = _as_int(value)
    return default if parsed is None else parsed


def _as_int(value: object) -> int | None:
    if value is None:
        return None
    if isinstance(value, bool):
        raise ValueError(f"expected integer, got boolean: {value!r}")
    if isinstance(value, int):
        return value
    if isinstance(value, float) and value.is_integer():
        return int(value)
    if isinstance(value, str):
        try:
            return int(value)
        except ValueError as e:
            raise ValueError(f"invalid integer {value!r}") from e
    raise ValueError(f"expected integer, got {type(value).__name__}: {value!r}")


@dataclass(frozen=True)
class RecordCursor:
    """Keyset cursor for a partially-synced object scan.

    A delta scan cursor must carry both keyset components. A cursor with only
    one of them cannot prove where the scan stopped, so it is rejected and the
    scan restarts from the committed boundary rather than skipping records.
    """

    last_id: str | None = None
    last_system_modstamp: str | None = None

    @property
    def is_delta_ready(self) -> bool:
        return self.last_id is not None and self.last_system_modstamp is not None

    @classmethod
    def from_mapping(cls, raw: object) -> RecordCursor | None:
        if not isinstance(raw, Mapping):
            return None
        return cls(
            last_id=_as_str(raw.get("last_id")),
            last_system_modstamp=_as_str(raw.get("last_system_modstamp")),
        )

    def to_json(self) -> dict[str, object]:
        data: dict[str, object] = {}
        if self.last_id is not None:
            data["last_id"] = self.last_id
        if self.last_system_modstamp is not None:
            data["last_system_modstamp"] = self.last_system_modstamp
        return data


@dataclass(frozen=True)
class ObjectState:
    """Committed per-object coverage that survives across runs.

    ``watermark`` is the last fully covered record boundary (used as the delta
    scan start); ``deletion_through`` is the last fully covered deletion
    boundary for this object.
    """

    watermark: str | None = None
    deletion_through: str | None = None

    @classmethod
    def from_mapping(cls, raw: object) -> ObjectState:
        if not isinstance(raw, Mapping):
            return cls()
        return cls(
            watermark=_as_str(raw.get("watermark")),
            deletion_through=_as_str(raw.get("deletion_through")),
        )

    def to_json(self) -> dict[str, object]:
        return {"watermark": self.watermark, "deletion_through": self.deletion_through}


@dataclass(frozen=True)
class PeopleState:
    """Committed people/group reconciliation state.

    Without this state a restart cannot tell which groups disappeared, so
    stale memberships would keep granting access indefinitely.
    """

    user_fingerprints: dict[str, str] = field(default_factory=dict)
    active_emails: frozenset[str] = field(default_factory=frozenset)
    group_emails: frozenset[str] = field(default_factory=frozenset)
    memberships: dict[str, tuple[str, ...]] = field(default_factory=dict)

    @classmethod
    def from_mapping(cls, raw: object) -> PeopleState | None:
        if not isinstance(raw, Mapping):
            return None
        return cls(
            user_fingerprints=_string_map(raw.get("user_fingerprints")),
            active_emails=frozenset(_string_list(raw.get("active_emails"))),
            group_emails=frozenset(_string_list(raw.get("group_emails"))),
            memberships=_string_tuple_map(raw.get("memberships")),
        )

    def to_json(self) -> dict[str, object]:
        return {
            "user_fingerprints": self.user_fingerprints,
            "active_emails": sorted(self.active_emails),
            "group_emails": sorted(self.group_emails),
            "memberships": {key: list(value) for key, value in self.memberships.items()},
        }


@dataclass(frozen=True)
class ShareSnapshot:
    """Committed share-grant fingerprints keyed by share object and parent id.

    Used to detect share additions, changes, and revocations across runs
    without relying on parent ``SystemModstamp``. When the snapshot exceeds the
    configured bound it is dropped and the connector falls back to periodic
    full permission reconciliation instead of persisting unbounded state.
    """

    grants: dict[str, dict[str, str]] = field(default_factory=dict)
    captured_at: str | None = None
    oversized: bool = False

    @classmethod
    def from_mapping(cls, raw: object) -> ShareSnapshot | None:
        if not isinstance(raw, Mapping):
            return None
        grants_value = raw.get("grants")
        grants: dict[str, dict[str, str]] = {}
        if isinstance(grants_value, Mapping):
            for object_name, parents in grants_value.items():
                if isinstance(object_name, str) and isinstance(parents, Mapping):
                    grants[object_name] = {
                        key: value
                        for key, value in _string_map(parents).items()
                    }
        return cls(
            grants=grants,
            captured_at=_as_str(raw.get("captured_at")),
            oversized=_bool_or(raw, "oversized", False),
        )

    def to_json(self) -> dict[str, object]:
        return {
            "grants": self.grants,
            "captured_at": self.captured_at,
            "oversized": self.oversized,
        }


@dataclass(frozen=True)
class RunProgress:
    """Run-scoped in-progress pass state.

    Identified by ``run_id`` so a checkpoint written by another run can never
    be mistaken for this run's progress. Never published to the source: only
    ``complete()`` promotes a checkpoint, and completion clears progress.
    """

    run_id: str
    mode: SyncRunMode
    window_end: str
    started_at: str
    current_object: str | None = None
    record_cursor: RecordCursor | None = None
    records_completed: tuple[str, ...] = ()
    deletions_completed: tuple[str, ...] = ()
    full_reconciliation: tuple[str, ...] = ()

    @classmethod
    def from_mapping(cls, raw: object) -> RunProgress | None:
        if not isinstance(raw, Mapping):
            return None
        run_id = _as_str(raw.get("run_id"))
        window_end = _as_str(raw.get("window_end"))
        started_at = _as_str(raw.get("started_at"))
        mode_value = _as_str(raw.get("mode"))
        if run_id is None or window_end is None or started_at is None or mode_value is None:
            return None
        try:
            mode = SyncRunMode(mode_value)
        except ValueError:
            return None
        return cls(
            run_id=run_id,
            mode=mode,
            window_end=window_end,
            started_at=started_at,
            current_object=_as_str(raw.get("current_object")),
            record_cursor=RecordCursor.from_mapping(raw.get("record_cursor")),
            records_completed=_string_tuple(raw.get("records_completed")),
            deletions_completed=_string_tuple(raw.get("deletions_completed")),
            full_reconciliation=_string_tuple(raw.get("full_reconciliation")),
        )

    def to_json(self) -> dict[str, object]:
        return {
            "run_id": self.run_id,
            "mode": self.mode.value,
            "window_end": self.window_end,
            "started_at": self.started_at,
            "current_object": self.current_object,
            "record_cursor": self.record_cursor.to_json() if self.record_cursor else None,
            "records_completed": list(self.records_completed),
            "deletions_completed": list(self.deletions_completed),
            "full_reconciliation": list(self.full_reconciliation),
        }


@dataclass(frozen=True)
class SalesforceCheckpoint:
    """Checkpoint passed between runs.

    ``objects`` is committed coverage; ``progress`` is run-scoped in-progress
    state; ``people`` and ``share_snapshot`` are committed reconciliation
    state. A source checkpoint is only ever promoted from a completed run, so
    it always has ``progress=None``.
    """

    version: int = CHECKPOINT_VERSION
    objects: dict[str, ObjectState] = field(default_factory=dict)
    progress: RunProgress | None = None
    people: PeopleState | None = None
    share_snapshot: ShareSnapshot | None = None
    synced_at: str | None = None

    @classmethod
    def from_mapping(cls, raw: Mapping[str, object] | None) -> SalesforceCheckpoint:
        if raw is None:
            return cls()
        version = _as_int(raw.get("version"))
        if version != CHECKPOINT_VERSION:
            return cls()
        objects: dict[str, ObjectState] = {}
        raw_objects = raw.get("objects")
        if isinstance(raw_objects, Mapping):
            for key, value in raw_objects.items():
                if isinstance(key, str) and isinstance(value, Mapping):
                    objects[key] = ObjectState.from_mapping(value)
        return cls(
            version=version,
            objects=objects,
            progress=RunProgress.from_mapping(raw.get("progress")),
            people=PeopleState.from_mapping(raw.get("people")),
            share_snapshot=ShareSnapshot.from_mapping(raw.get("share_snapshot")),
            synced_at=_as_str(raw.get("synced_at")),
        )

    def without_progress(self) -> SalesforceCheckpoint:
        return replace(self, progress=None)

    def state_for(self, object_name: str) -> ObjectState:
        return self.objects.get(object_name, ObjectState())

    def to_json(self) -> dict[str, object]:
        return {
            "version": self.version,
            "objects": {key: value.to_json() for key, value in self.objects.items()},
            "progress": self.progress.to_json() if self.progress is not None else None,
            "people": self.people.to_json() if self.people is not None else None,
            "share_snapshot": (
                self.share_snapshot.to_json() if self.share_snapshot is not None else None
            ),
            "synced_at": self.synced_at,
        }


def _string_map(value: object) -> dict[str, str]:
    if not isinstance(value, Mapping):
        return {}
    result: dict[str, str] = {}
    for key, item in value.items():
        if isinstance(key, str):
            parsed = _as_str(item)
            if parsed is not None:
                result[key] = parsed
    return result


def _string_list(value: object) -> list[str]:
    if not isinstance(value, list):
        return []
    result: list[str] = []
    for item in value:
        parsed = _as_str(item)
        if parsed is not None:
            result.append(parsed)
    return result


def _string_tuple(value: object) -> tuple[str, ...]:
    return tuple(_string_list(value))


def _string_tuple_map(value: object) -> dict[str, tuple[str, ...]]:
    if not isinstance(value, Mapping):
        return {}
    result: dict[str, tuple[str, ...]] = {}
    for key, item in value.items():
        if isinstance(key, str):
            result[key] = _string_tuple(item)
    return result


def person_fingerprint(user: UserRecord) -> str:
    """Stable fingerprint of the fields that drive person/group decisions.

    ``SystemModstamp`` alone cannot detect profile edits when the field is
    hidden or unreliable, so compare the canonical field set instead.
    """

    payload = {
        "name": user.name,
        "first_name": user.first_name,
        "last_name": user.last_name,
        "email": user.email,
        "title": user.title,
        "department": user.department,
        "manager_id": user.manager_id,
        "user_role_id": user.user_role_id,
        "is_active": user.is_active,
        "employee_number": user.employee_number,
    }
    return hashlib.sha256(json.dumps(payload, sort_keys=True).encode("utf-8")).hexdigest()

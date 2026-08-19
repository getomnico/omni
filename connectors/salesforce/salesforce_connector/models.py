"""Typed representations of Salesforce objects and connector state."""

from __future__ import annotations

from collections.abc import Mapping
from dataclasses import dataclass, field
from datetime import datetime
from enum import StrEnum

from .config import REALTIME_POLL_SECONDS

CHECKPOINT_VERSION = 1
# Group emails are synthesized from Salesforce group/role ids because Salesforce
# groups have no email address of their own. The suffix is opaque; Omni matches
# these exact strings.
GROUP_EMAIL_SUFFIX = "@salesforce.groups"
ROLE_EMAIL_SUFFIX = "@salesforce.roles"


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


@dataclass(frozen=True)
class GroupRecord:
    id: str
    name: str | None
    type: str | None

    @classmethod
    def from_record(cls, raw: Mapping[str, object]) -> GroupRecord:
        return cls(
            id=_as_required_str(raw.get("Id"), "Id"),
            name=_as_str(raw.get("Name")),
            type=_as_str(raw.get("Type")),
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
class ShareRecord:
    id: str
    parent_id: str
    user_or_group_id: str
    access_level: str | None
    row_cause: str | None

    @classmethod
    def from_record(cls, raw: Mapping[str, object], parent_field: str) -> ShareRecord:
        return cls(
            id=_as_required_str(raw.get("Id"), "Id"),
            parent_id=_as_required_str(raw.get(parent_field), parent_field),
            user_or_group_id=_as_required_str(raw.get("UserOrGroupId"), "UserOrGroupId"),
            access_level=_as_str(raw.get("AccessLevel")),
            row_cause=_as_str(raw.get("RowCause")),
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
    """Keyset cursor for a partially-synced object scan."""

    last_id: str | None = None
    last_system_modstamp: str | None = None

    @classmethod
    def from_mapping(cls, raw: Mapping[str, object] | None) -> RecordCursor | None:
        if raw is None:
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
class SalesforceCheckpoint:
    """Resume cursor persisted between sync runs.

    ``record_cursors`` hold keyset positions for partially-scanned objects;
    ``watermarks`` are the incremental cursors (max SystemModstamp per object);
    ``deleted_through`` is the end of the last getDeleted window. People and
    shares are re-queried on every run (they are small), so they carry no
    resume state.
    """

    version: int = CHECKPOINT_VERSION
    records_synced: dict[str, bool] = field(default_factory=dict)
    record_cursors: dict[str, RecordCursor] = field(default_factory=dict)
    watermarks: dict[str, str] = field(default_factory=dict)
    deleted_through: str | None = None
    synced_at: str | None = None

    @classmethod
    def from_mapping(cls, raw: Mapping[str, object] | None) -> SalesforceCheckpoint:
        if raw is None:
            return cls()
        version = _as_int(raw.get("version"))
        if version != CHECKPOINT_VERSION:
            return cls()
        record_cursors: dict[str, RecordCursor] = {}
        raw_cursors = raw.get("record_cursors")
        if isinstance(raw_cursors, Mapping):
            for key, value in raw_cursors.items():
                if isinstance(key, str) and isinstance(value, Mapping):
                    cursor = RecordCursor.from_mapping(value)
                    if cursor is not None:
                        record_cursors[key] = cursor
        return cls(
            version=version,
            records_synced=_string_bool_map(raw.get("records_synced")),
            record_cursors=record_cursors,
            watermarks=_string_map(raw.get("watermarks")),
            deleted_through=_as_str(raw.get("deleted_through")),
            synced_at=_as_str(raw.get("synced_at")),
        )

    def to_json(self) -> dict[str, object]:
        return {
            "version": self.version,
            "records_synced": self.records_synced,
            "record_cursors": {
                key: cursor.to_json() for key, cursor in self.record_cursors.items()
            },
            "watermarks": self.watermarks,
            "deleted_through": self.deleted_through,
            "synced_at": self.synced_at,
        }


def _string_bool_map(value: object) -> dict[str, bool]:
    if not isinstance(value, Mapping):
        return {}
    result: dict[str, bool] = {}
    for key, item in value.items():
        if isinstance(key, str):
            parsed = _as_bool(item)
            if parsed is not None:
                result[key] = parsed
    return result


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


class SyncPhase(StrEnum):
    """Overall progress phase of a sync run."""

    PEOPLE = "people"
    SHARES = "shares"
    RECORDS = "records"
    DELETES = "deletes"
    COMPLETE = "complete"

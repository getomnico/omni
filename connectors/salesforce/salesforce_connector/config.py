"""Configuration constants and typed object configs for the Salesforce connector."""

from __future__ import annotations

import json
from dataclasses import dataclass
from enum import StrEnum
from hashlib import sha256

# Salesforce REST API version
API_VERSION = "v62.0"

# Max records per SOQL page (Salesforce limit)
PAGE_SIZE = 2000

# Look back this far past the watermark when running a delta pass, so records
# that changed while the previous run was in flight are not missed.
DELTA_OVERLAP_SECONDS = 900

# Emit a checkpoint every N records during object scans.
CHECKPOINT_INTERVAL = 500

# Salesforce only retains deleted records for this long. getDeleted cannot
# cover deletions older than the provider's reported `earliestDateAvailable`.
DELETION_RETENTION_DAYS = 30

# Maximum number of shared parents persisted in the checkpoint share snapshot.
# Beyond this the connector cannot safely diff share grants from checkpoint
# state, so it reconciles every share-enabled object on every pass instead.
MAX_SHARE_SNAPSHOT_ENTRIES = 20_000


class AttributeValueType(StrEnum):
    """Type of one structured document attribute."""

    TEXT = "text"
    NUMBER = "number"
    DATETIME = "datetime"


class SalesforceObjectName(StrEnum):
    """Salesforce objects this connector can sync as records."""

    ACCOUNT = "Account"
    CONTACT = "Contact"
    OPPORTUNITY = "Opportunity"
    LEAD = "Lead"
    CASE = "Case"
    TASK = "Task"


class SyncRunMode(StrEnum):
    """The record-scan mode of one in-progress pass."""

    FULL = "full"
    INCREMENTAL = "incremental"


@dataclass(frozen=True)
class SalesforceAttribute:
    """A structured attribute emitted on documents for one object type."""

    key: str
    field: str
    value_type: AttributeValueType = AttributeValueType.TEXT


@dataclass(frozen=True)
class SalesforceObjectConfig:
    """Configuration for one Salesforce object type."""

    name: SalesforceObjectName
    title_fields: tuple[str, ...] = ()
    fields: tuple[str, ...] = ()
    # Optional relationship field to traverse for account names (e.g. "Account").
    account_relationship: str | None = None
    # Optional share object (e.g. "AccountShare") and its parent lookup field.
    share_object: str | None = None
    share_parent_field: str | None = None
    # Standard share objects expose object-specific access-level columns
    # (AccountAccessLevel, CaseAccessLevel, ...), not a generic AccessLevel.
    share_access_level_field: str | None = None
    attributes: tuple[SalesforceAttribute, ...] = ()

    def all_fields(self) -> tuple[str, ...]:
        fields = list(self.fields)
        if self.account_relationship:
            fields.append(f"{self.account_relationship}.Name")
        return tuple(fields)


def _attrs(*items: SalesforceAttribute) -> tuple[SalesforceAttribute, ...]:
    return items


SALESFORCE_OBJECT_CONFIGS: tuple[SalesforceObjectConfig, ...] = (
    SalesforceObjectConfig(
        name=SalesforceObjectName.ACCOUNT,
        title_fields=("Name",),
        fields=(
            "Id",
            "Name",
            "Industry",
            "Phone",
            "Website",
            "BillingCity",
            "BillingState",
            "BillingCountry",
            "NumberOfEmployees",
            "AnnualRevenue",
            "Description",
            "Type",
            "OwnerId",
            "CreatedDate",
            "SystemModstamp",
        ),
        share_object="AccountShare",
        share_parent_field="AccountId",
        share_access_level_field="AccountAccessLevel",
        attributes=_attrs(
            SalesforceAttribute("account_name", "Name"),
            SalesforceAttribute("industry", "Industry"),
            SalesforceAttribute("type", "Type"),
            SalesforceAttribute("billing_country", "BillingCountry"),
            SalesforceAttribute("annual_revenue", "AnnualRevenue", AttributeValueType.NUMBER),
            SalesforceAttribute("phone", "Phone"),
            SalesforceAttribute("website", "Website"),
        ),
    ),
    SalesforceObjectConfig(
        name=SalesforceObjectName.CONTACT,
        title_fields=("Name",),
        fields=(
            "Id",
            "Name",
            "FirstName",
            "LastName",
            "Email",
            "Phone",
            "Title",
            "Department",
            "AccountId",
            "MailingCity",
            "MailingState",
            "MailingCountry",
            "OwnerId",
            "CreatedDate",
            "SystemModstamp",
        ),
        account_relationship="Account",
        share_object="ContactShare",
        share_parent_field="ContactId",
        share_access_level_field="ContactAccessLevel",
        attributes=_attrs(
            SalesforceAttribute("account_name", "Account.Name"),
            SalesforceAttribute("account_id", "AccountId"),
            SalesforceAttribute("email", "Email"),
            SalesforceAttribute("title", "Title"),
            SalesforceAttribute("department", "Department"),
        ),
    ),
    SalesforceObjectConfig(
        name=SalesforceObjectName.OPPORTUNITY,
        title_fields=("Name",),
        fields=(
            "Id",
            "Name",
            "Amount",
            "StageName",
            "CloseDate",
            "Probability",
            "Type",
            "LeadSource",
            "Description",
            "AccountId",
            "OwnerId",
            "CreatedDate",
            "SystemModstamp",
        ),
        account_relationship="Account",
        share_object="OpportunityShare",
        share_parent_field="OpportunityId",
        share_access_level_field="OpportunityAccessLevel",
        attributes=_attrs(
            SalesforceAttribute("account_name", "Account.Name"),
            SalesforceAttribute("account_id", "AccountId"),
            SalesforceAttribute("stage", "StageName"),
            SalesforceAttribute("amount", "Amount", AttributeValueType.NUMBER),
            SalesforceAttribute("close_date", "CloseDate", AttributeValueType.DATETIME),
            SalesforceAttribute("probability", "Probability", AttributeValueType.NUMBER),
            SalesforceAttribute("type", "Type"),
            SalesforceAttribute("lead_source", "LeadSource"),
        ),
    ),
    SalesforceObjectConfig(
        name=SalesforceObjectName.LEAD,
        title_fields=("Name",),
        fields=(
            "Id",
            "Name",
            "FirstName",
            "LastName",
            "Email",
            "Phone",
            "Company",
            "Title",
            "Industry",
            "Status",
            "LeadSource",
            "Description",
            "OwnerId",
            "CreatedDate",
            "SystemModstamp",
        ),
        share_object="LeadShare",
        share_parent_field="LeadId",
        share_access_level_field="LeadAccessLevel",
        attributes=_attrs(
            SalesforceAttribute("company", "Company"),
            SalesforceAttribute("lead_source", "LeadSource"),
            SalesforceAttribute("industry", "Industry"),
            SalesforceAttribute("email", "Email"),
            SalesforceAttribute("title", "Title"),
            SalesforceAttribute("status", "Status"),
        ),
    ),
    SalesforceObjectConfig(
        name=SalesforceObjectName.CASE,
        title_fields=("Subject",),
        fields=(
            "Id",
            "CaseNumber",
            "Subject",
            "Description",
            "Status",
            "Priority",
            "Type",
            "Origin",
            "ContactId",
            "AccountId",
            "OwnerId",
            "CreatedDate",
            "SystemModstamp",
        ),
        account_relationship="Account",
        share_object="CaseShare",
        share_parent_field="CaseId",
        share_access_level_field="CaseAccessLevel",
        attributes=_attrs(
            SalesforceAttribute("case_number", "CaseNumber"),
            SalesforceAttribute("status", "Status"),
            SalesforceAttribute("priority", "Priority"),
            SalesforceAttribute("type", "Type"),
            SalesforceAttribute("origin", "Origin"),
            SalesforceAttribute("account_name", "Account.Name"),
            SalesforceAttribute("account_id", "AccountId"),
            SalesforceAttribute("contact_id", "ContactId"),
        ),
    ),
    SalesforceObjectConfig(
        name=SalesforceObjectName.TASK,
        title_fields=("Subject",),
        fields=(
            "Id",
            "Subject",
            "Description",
            "Status",
            "Priority",
            "ActivityDate",
            "WhoId",
            "WhatId",
            "OwnerId",
            "CreatedDate",
            "SystemModstamp",
        ),
        attributes=_attrs(
            SalesforceAttribute("status", "Status"),
            SalesforceAttribute("priority", "Priority"),
            SalesforceAttribute("activity_date", "ActivityDate", AttributeValueType.DATETIME),
            SalesforceAttribute("who_id", "WhoId"),
            SalesforceAttribute("what_id", "WhatId"),
        ),
    ),
)

# Objects always synced in addition to the configurable record objects.
PEOPLE_OBJECTS = ("User", "Group", "GroupMember", "UserRole")

SALESFORCE_OBJECT_TYPES: tuple[SalesforceObjectName, ...] = tuple(
    config.name for config in SALESFORCE_OBJECT_CONFIGS
)


def config_for(object_type: str) -> SalesforceObjectConfig | None:
    for config in SALESFORCE_OBJECT_CONFIGS:
        if config.name == object_type:
            return config
    return None


def validate_object_names(object_names: frozenset[str], setting: str) -> None:
    """Reject unknown object names instead of silently ignoring them."""
    unknown = sorted(object_names.difference({name.value for name in SALESFORCE_OBJECT_TYPES}))
    if unknown:
        raise ValueError(f"Unknown {setting}: {', '.join(unknown)}")


def enabled_object_configs(
    enabled_objects: frozenset[str],
) -> tuple[SalesforceObjectConfig, ...]:
    if not enabled_objects:
        return SALESFORCE_OBJECT_CONFIGS
    return tuple(config for config in SALESFORCE_OBJECT_CONFIGS if config.name in enabled_objects)


def schema_fingerprint(
    enabled_objects: frozenset[str],
    public_read_objects: frozenset[str],
    *,
    sync_users: bool = True,
    sync_groups: bool = True,
    sync_shares: bool = True,
    grant_access_using_hierarchies: bool = True,
) -> str:
    """Hash of the synced schema and permission-affecting settings.

    Stored in connector_state; when it changes (fields/objects added or
    removed, visibility or sharing settings changed, API version bump) saved
    watermarks no longer cover everything the index expects, so a full resync
    is required to remove stale permissions and attributes."""
    payload = {
        "api_version": API_VERSION,
        "objects": [
            {
                "name": config.name.value,
                "fields": sorted(config.all_fields()),
                "share_object": config.share_object,
                "share_access_level_field": config.share_access_level_field,
                "attributes": [
                    {"key": attr.key, "field": attr.field, "type": attr.value_type.value}
                    for attr in config.attributes
                ],
            }
            for config in enabled_object_configs(enabled_objects)
        ],
        "enabled_objects": sorted(enabled_objects),
        "public_read_objects": sorted(public_read_objects),
        "sync_users": sync_users,
        "sync_groups": sync_groups,
        "sync_shares": sync_shares,
        "grant_access_using_hierarchies": grant_access_using_hierarchies,
    }
    return sha256(json.dumps(payload, sort_keys=True).encode("utf-8")).hexdigest()

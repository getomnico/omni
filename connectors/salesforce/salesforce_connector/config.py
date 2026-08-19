"""Configuration constants for the Salesforce connector."""

from __future__ import annotations

import json
from dataclasses import dataclass
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

# Default poll interval for the realtime sync loop.
REALTIME_POLL_SECONDS = 60


@dataclass(frozen=True)
class SalesforceAttribute:
    """A structured attribute emitted on documents for one object type."""

    key: str
    field: str
    value_type: str = "text"  # "text" | "number" | "datetime"


@dataclass(frozen=True)
class SalesforceObjectConfig:
    """Configuration for one Salesforce object type."""

    name: str
    record_model: str
    title_fields: tuple[str, ...] = ()
    fields: tuple[str, ...] = ()
    # Optional relationship field to traverse for account names (e.g. "Account").
    account_relationship: str | None = None
    # Optional share object (e.g. "AccountShare") and its parent lookup field.
    share_object: str | None = None
    share_parent_field: str | None = None
    public_read_default: bool = False
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
        name="Account",
        record_model="AccountRecord",
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
        public_read_default=True,
        attributes=_attrs(
            SalesforceAttribute("account_name", "Name"),
            SalesforceAttribute("industry", "Industry"),
            SalesforceAttribute("type", "Type"),
            SalesforceAttribute("billing_country", "BillingCountry"),
            SalesforceAttribute("annual_revenue", "AnnualRevenue", "number"),
            SalesforceAttribute("phone", "Phone"),
            SalesforceAttribute("website", "Website"),
        ),
    ),
    SalesforceObjectConfig(
        name="Contact",
        record_model="ContactRecord",
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
        public_read_default=True,
        attributes=_attrs(
            SalesforceAttribute("account_name", "Account.Name"),
            SalesforceAttribute("account_id", "AccountId"),
            SalesforceAttribute("email", "Email"),
            SalesforceAttribute("title", "Title"),
            SalesforceAttribute("department", "Department"),
        ),
    ),
    SalesforceObjectConfig(
        name="Opportunity",
        record_model="OpportunityRecord",
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
        public_read_default=False,
        attributes=_attrs(
            SalesforceAttribute("account_name", "Account.Name"),
            SalesforceAttribute("account_id", "AccountId"),
            SalesforceAttribute("stage", "StageName"),
            SalesforceAttribute("amount", "Amount", "number"),
            SalesforceAttribute("close_date", "CloseDate", "datetime"),
            SalesforceAttribute("probability", "Probability", "number"),
            SalesforceAttribute("type", "Type"),
            SalesforceAttribute("lead_source", "LeadSource"),
        ),
    ),
    SalesforceObjectConfig(
        name="Lead",
        record_model="LeadRecord",
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
        public_read_default=False,
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
        name="Case",
        record_model="CaseRecord",
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
        public_read_default=False,
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
        name="Task",
        record_model="TaskRecord",
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
        public_read_default=False,
        attributes=_attrs(
            SalesforceAttribute("status", "Status"),
            SalesforceAttribute("priority", "Priority"),
            SalesforceAttribute("activity_date", "ActivityDate", "datetime"),
            SalesforceAttribute("who_id", "WhoId"),
            SalesforceAttribute("what_id", "WhatId"),
        ),
    ),
)

# Objects always synced in addition to the configurable record objects.
PEOPLE_OBJECTS = ("User", "Group", "GroupMember", "UserRole")

SALESFORCE_OBJECT_TYPES: tuple[str, ...] = tuple(
    config.name for config in SALESFORCE_OBJECT_CONFIGS
)


def config_for(object_type: str) -> SalesforceObjectConfig | None:
    for config in SALESFORCE_OBJECT_CONFIGS:
        if config.name == object_type:
            return config
    return None


def enabled_object_configs(
    enabled_objects: frozenset[str],
) -> tuple[SalesforceObjectConfig, ...]:
    if not enabled_objects:
        return SALESFORCE_OBJECT_CONFIGS
    return tuple(config for config in SALESFORCE_OBJECT_CONFIGS if config.name in enabled_objects)


def schema_fingerprint(enabled_objects: frozenset[str], public_read_objects: frozenset[str]) -> str:
    """Hash of the synced schema. Stored in connector_state; when it changes
    (fields/objects added or removed, API version bump) saved watermarks are
    invalid and a full resync is required."""
    payload = {
        "api_version": API_VERSION,
        "objects": [
            {
                "name": config.name,
                "fields": sorted(config.all_fields()),
                "share_object": config.share_object,
                "attributes": [
                    {"key": attr.key, "field": attr.field, "type": attr.value_type}
                    for attr in config.attributes
                ],
                "public_read_default": config.public_read_default,
            }
            for config in enabled_object_configs(enabled_objects)
        ],
        "enabled_objects": sorted(enabled_objects),
        "public_read_objects": sorted(public_read_objects),
    }
    return sha256(json.dumps(payload, sort_keys=True).encode("utf-8")).hexdigest()

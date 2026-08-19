"""Mapping of typed Salesforce records to Omni Documents."""

from __future__ import annotations

import re
from collections.abc import Mapping
from dataclasses import fields

from omni_connector import Document, DocumentMetadata, DocumentPermissions

from .config import SalesforceObjectConfig, config_for
from .models import (
    AccountRecord,
    CaseRecord,
    ContactRecord,
    LeadRecord,
    OpportunityRecord,
    TaskRecord,
)

RecordModel = (
    AccountRecord | ContactRecord | OpportunityRecord | LeadRecord | CaseRecord | TaskRecord
)

# Fields never rendered into content or attributes; they are either structural
# (Id) or carried as metadata/attributes under cleaner keys.
_SKIP_FIELDS = frozenset({"id", "owner_id", "created_date", "system_modstamp", "account_id"})


def _attribute_value(value: object) -> object:
    if value is None:
        return None
    if isinstance(value, bool):
        return str(value).lower()
    return value.isoformat() if hasattr(value, "isoformat") else value


def attributes_for(
    object_type: str, record: RecordModel, owner_email: str | None
) -> dict[str, object]:
    """Structured attributes for one record.

    Common keys are present on every object; per-object keys come from the
    object config. Attribute keys are stable and referenced by the manifest's
    search operators.
    """
    config = config_for(object_type)
    if config is None:
        raise ValueError(f"no config for object type {object_type}")

    attrs: dict[str, object] = {
        "source_type": "salesforce",
        "object_type": object_type,
        "salesforce_id": record.id,
        "owner_id": _attribute_value(getattr(record, "owner_id", None)),
        "owner_email": _attribute_value(owner_email),
        "created_date": _attribute_value(getattr(record, "created_date", None)),
        "modified_date": _attribute_value(getattr(record, "system_modstamp", None)),
    }
    for spec in config.attributes:
        value = _attribute_value(_field_value(record, spec.field))
        if value is not None:
            attrs[spec.key] = value
    return {key: value for key, value in attrs.items() if value is not None}


def _field_value(record: RecordModel, field_name: str) -> object:
    if field_name == "Account.Name":
        return getattr(record, "account_name", None)
    # Salesforce field names (e.g. "NumberOfEmployees") map to snake_case
    # dataclass attributes (e.g. "number_of_employees").
    attribute = re.sub(r"(?<!^)(?=[A-Z])", "_", field_name).lower()
    return getattr(record, attribute, None)


def map_record_to_document(
    *,
    object_type: str,
    record: RecordModel,
    content_id: str,
    instance_url: str,
    owner_email: str | None,
    permissions: DocumentPermissions,
    attributes: Mapping[str, object],
) -> Document:
    """Build the Omni Document for one Salesforce record."""
    config = config_for(object_type)
    if config is None:
        raise ValueError(f"no config for object type {object_type}")
    title = _title_for(config, record)
    external_id = f"{object_type}:{record.id}"
    return Document(
        external_id=external_id,
        title=title,
        content_id=content_id,
        metadata=DocumentMetadata(
            title=title,
            author=owner_email,
            created_at=getattr(record, "created_date", None),
            updated_at=getattr(record, "system_modstamp", None),
            content_type=object_type,
            mime_type="text/plain",
            url=f"{instance_url}/{record.id}",
        ),
        permissions=permissions,
        attributes=dict(attributes),
    )


def _title_for(config: SalesforceObjectConfig, record: RecordModel) -> str:
    for field_name in config.title_fields:
        value = _field_value(record, field_name)
        if isinstance(value, str) and value:
            return value
    if isinstance(record, (ContactRecord, LeadRecord)) and record.email:
        return record.email
    fallback = getattr(record, "name", None) or getattr(record, "subject", None)
    if isinstance(fallback, str) and fallback:
        return fallback
    return f"{config.name} {record.id}"


def generate_content(object_type: str, record: RecordModel) -> str:
    """Searchable plain-text content for one record."""
    config = config_for(object_type)
    if config is None:
        raise ValueError(f"no config for object type {object_type}")

    lines = [f"Salesforce {object_type}", ""]
    title = _title_for(config, record)
    lines.append(f"Title: {title}")
    lines.append("")

    for field in fields(record):
        if field.name in _SKIP_FIELDS:
            continue
        value = getattr(record, field.name)
        if value is None:
            continue
        if isinstance(value, bool):
            rendered = str(value).lower()
        elif hasattr(value, "isoformat"):
            rendered = value.isoformat()
        else:
            rendered = str(value)
        lines.append(f"{_format_label(field.name)}: {rendered}")

    account_name = getattr(record, "account_name", None)
    if isinstance(account_name, str) and account_name:
        lines.append(f"Account Name: {account_name}")

    return "\n".join(lines)


def _format_label(field_name: str) -> str:
    result = []
    for i, char in enumerate(field_name):
        if char.isupper() and i > 0 and not field_name[i - 1].isupper():
            result.append(" ")
        result.append(char)
    return "".join(result).replace("_", " ").title()

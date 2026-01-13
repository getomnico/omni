"""Object-to-Document mapping functions for HubSpot objects."""

from datetime import datetime, timezone
from typing import Any

from omni_connector import Document, DocumentMetadata, DocumentPermissions

from .config import HUBSPOT_OBJECT_CONFIGS


def map_hubspot_object_to_document(
    object_type: str,
    obj: dict[str, Any],
    content_id: str,
    portal_id: str | None = None,
) -> Document:
    """
    Map a HubSpot object to an Omni Document.

    Mapping strategy:
    - external_id: "{object_type}:{hubspot_id}" (e.g., "contact:12345")
    - title: Derived from object type (e.g., contact name, deal name)
    - content_id: Pre-stored content reference
    - metadata: Timestamps, URL, author, mime_type
    - permissions: Set to public (CRM data is org-wide)
    - attributes: Object type, hubspot properties for filtering

    Args:
        object_type: Type of HubSpot object
        obj: HubSpot object data as dict
        content_id: ID from content storage
        portal_id: Optional HubSpot portal ID for URL generation

    Returns:
        Omni Document instance
    """
    config = HUBSPOT_OBJECT_CONFIGS.get(object_type, {})
    properties = obj.get("properties", {})

    # Generate external_id
    hubspot_id = obj.get("id")
    external_id = f"{object_type}:{hubspot_id}"

    # Generate title based on object type
    title = _get_title(object_type, properties, config)

    # Parse timestamps
    created_at = _parse_timestamp(
        properties.get("createdate") or properties.get("hs_createdate")
    )
    updated_at = _parse_timestamp(
        properties.get("hs_lastmodifieddate") or properties.get("createdate")
    )

    # Build HubSpot URL
    url = _build_hubspot_url(object_type, hubspot_id, portal_id)

    return Document(
        external_id=external_id,
        title=title,
        content_id=content_id,
        metadata=DocumentMetadata(
            author=properties.get("hubspot_owner_id"),
            created_at=created_at,
            updated_at=updated_at,
            url=url,
            mime_type="text/plain",
            extra={
                "hubspot_id": hubspot_id,
                "object_type": object_type,
            },
        ),
        permissions=DocumentPermissions(public=True),
        attributes={
            "source_type": "hubspot",
            "object_type": object_type,
            "hubspot_id": hubspot_id,
        },
    )


def generate_content(object_type: str, obj: dict[str, Any]) -> str:
    """
    Generate searchable text content from a HubSpot object.

    Args:
        object_type: Type of HubSpot object
        obj: HubSpot object data as dict

    Returns:
        Plain text content for indexing
    """
    properties = obj.get("properties", {})
    lines: list[str] = []

    # Add object type header
    lines.append(f"HubSpot {object_type.title()}")
    lines.append("")

    # Add title
    config = HUBSPOT_OBJECT_CONFIGS.get(object_type, {})
    title = _get_title(object_type, properties, config)
    lines.append(f"Title: {title}")
    lines.append("")

    # Add all non-empty properties
    for key, value in properties.items():
        if value is not None and value != "":
            # Clean up property name for display
            display_key = key.replace("_", " ").replace("hs ", "").title()
            lines.append(f"{display_key}: {value}")

    return "\n".join(lines)


def _get_title(
    object_type: str,
    properties: dict[str, Any],
    config: dict[str, Any],
) -> str:
    """Generate title based on object type."""
    title_fields = config.get("title_fields", [])

    # Try configured title fields first
    for field in title_fields:
        if value := properties.get(field):
            return str(value)

    # Fallback titles by type
    fallbacks = {
        "contacts": _contact_title(properties),
        "companies": properties.get("name", "Unnamed Company"),
        "deals": properties.get("dealname", "Unnamed Deal"),
        "tickets": properties.get("subject", "Untitled Ticket"),
        "calls": _activity_title("Call", properties),
        "emails": properties.get("hs_email_subject", "No Subject"),
        "meetings": properties.get("hs_meeting_title", "Untitled Meeting"),
        "notes": _activity_title("Note", properties),
        "tasks": properties.get("hs_task_subject", "Untitled Task"),
    }

    return fallbacks.get(
        object_type, f"{object_type.title()} {properties.get('hs_object_id', '')}"
    )


def _contact_title(properties: dict[str, Any]) -> str:
    """Generate contact title from name fields."""
    first = properties.get("firstname", "")
    last = properties.get("lastname", "")
    email = properties.get("email", "")

    if first or last:
        return f"{first} {last}".strip()
    return email or "Unknown Contact"


def _activity_title(activity_type: str, properties: dict[str, Any]) -> str:
    """Generate title for activities (calls, notes)."""
    timestamp = properties.get("hs_timestamp")
    if timestamp:
        parsed = _parse_timestamp(timestamp)
        if parsed:
            return f"{activity_type} - {parsed.strftime('%Y-%m-%d %H:%M')}"
    return f"{activity_type} - Unknown Date"


def _parse_timestamp(value: str | int | None) -> datetime | None:
    """Parse HubSpot timestamp to datetime."""
    if not value:
        return None
    try:
        # HubSpot uses milliseconds since epoch or ISO 8601
        if isinstance(value, int) or (isinstance(value, str) and value.isdigit()):
            return datetime.fromtimestamp(int(value) / 1000, tz=timezone.utc)
        if isinstance(value, str):
            return datetime.fromisoformat(value.replace("Z", "+00:00"))
    except (ValueError, TypeError):
        pass
    return None


def _build_hubspot_url(
    object_type: str,
    hubspot_id: str | None,
    portal_id: str | None,
) -> str | None:
    """Build URL to the HubSpot record."""
    if not hubspot_id:
        return None
    if not portal_id:
        return None

    # Map object types to URL paths
    url_paths = {
        "contacts": "contact",
        "companies": "company",
        "deals": "deal",
        "tickets": "ticket",
        "calls": "record/0-48",  # Engagements use different paths
        "emails": "record/0-49",
        "meetings": "record/0-47",
        "notes": "record/0-46",
        "tasks": "record/0-27",
    }

    path = url_paths.get(object_type)
    if not path:
        return None

    return f"https://app.hubspot.com/contacts/{portal_id}/{path}/{hubspot_id}"

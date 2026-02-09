"""Map Microsoft Graph API responses to Omni Document models."""

from datetime import datetime, timezone
from typing import Any

from omni_connector import Document, DocumentMetadata, DocumentPermissions


def map_drive_item_to_document(
    item: dict[str, Any],
    content_id: str,
    source_type: str = "onedrive",
    owner_email: str | None = None,
    site_id: str | None = None,
) -> Document:
    """Map a OneDrive/SharePoint driveItem to an Omni Document."""
    parent_ref = item.get("parentReference", {})
    drive_id = parent_ref.get("driveId", "unknown")
    item_id = item["id"]

    if source_type == "sharepoint" and site_id:
        external_id = f"sharepoint:{site_id}:{item_id}"
    else:
        external_id = f"onedrive:{drive_id}:{item_id}"

    file_info = item.get("file", {})
    mime_type = file_info.get("mimeType")
    size = item.get("size")

    return Document(
        external_id=external_id,
        title=item.get("name", "Untitled"),
        content_id=content_id,
        metadata=DocumentMetadata(
            created_at=_parse_iso(item.get("createdDateTime")),
            updated_at=_parse_iso(item.get("lastModifiedDateTime")),
            url=item.get("webUrl"),
            mime_type=mime_type,
            size=str(size) if size is not None else None,
            path=parent_ref.get("path"),
            extra={
                "drive_id": drive_id,
                "item_id": item_id,
            },
        ),
        permissions=DocumentPermissions(
            public=(source_type == "sharepoint"),
            users=[owner_email] if owner_email else [],
        ),
        attributes={
            "source_type": source_type,
        },
    )


def map_message_to_document(
    message: dict[str, Any],
    user_id: str,
    user_email: str | None,
    content_id: str,
) -> Document:
    """Map an Outlook message to an Omni Document."""
    msg_id = message["id"]
    external_id = f"mail:{user_id}:{msg_id}"

    sender = message.get("from", {}).get("emailAddress", {})
    sender_name = sender.get("name") or sender.get("address")

    return Document(
        external_id=external_id,
        title=message.get("subject") or "No Subject",
        content_id=content_id,
        metadata=DocumentMetadata(
            author=sender_name,
            created_at=_parse_iso(message.get("sentDateTime")),
            updated_at=_parse_iso(message.get("receivedDateTime")),
            url=message.get("webLink"),
            mime_type="message/rfc822",
            extra={
                "message_id": msg_id,
                "has_attachments": message.get("hasAttachments", False),
            },
        ),
        permissions=DocumentPermissions(
            public=False,
            users=[user_email] if user_email else [],
        ),
        attributes={
            "source_type": "microsoft_mail",
        },
    )


def map_event_to_document(
    event: dict[str, Any],
    user_id: str,
    content_id: str,
) -> Document:
    """Map an Outlook calendar event to an Omni Document."""
    event_id = event["id"]
    external_id = f"calendar:{user_id}:{event_id}"

    organizer = event.get("organizer", {}).get("emailAddress", {})
    organizer_name = organizer.get("name") or organizer.get("address")

    attendee_emails = []
    for att in event.get("attendees", []):
        email = att.get("emailAddress", {}).get("address")
        if email:
            attendee_emails.append(email)
    org_email = organizer.get("address")
    if org_email and org_email not in attendee_emails:
        attendee_emails.append(org_email)

    start_dt = _parse_graph_datetime(event.get("start"))
    end_dt = _parse_graph_datetime(event.get("end"))

    return Document(
        external_id=external_id,
        title=event.get("subject") or "Untitled Event",
        content_id=content_id,
        metadata=DocumentMetadata(
            author=organizer_name,
            created_at=start_dt,
            updated_at=start_dt,
            url=event.get("webLink"),
            mime_type="text/calendar",
            extra={
                "event_id": event_id,
                "is_all_day": event.get("isAllDay", False),
                "is_cancelled": event.get("isCancelled", False),
            },
        ),
        permissions=DocumentPermissions(
            public=False,
            users=attendee_emails,
        ),
        attributes={
            "source_type": "microsoft_calendar",
        },
    )


def generate_drive_item_content(item: dict[str, Any], user: dict[str, Any]) -> str:
    """Generate metadata-based content for a drive item."""
    lines = [
        f"File: {item.get('name', 'Untitled')}",
    ]
    parent_path = item.get("parentReference", {}).get("path")
    if parent_path:
        lines.append(f"Path: {parent_path}")
    size = item.get("size")
    if size is not None:
        lines.append(f"Size: {size} bytes")
    mime_type = item.get("file", {}).get("mimeType")
    if mime_type:
        lines.append(f"Type: {mime_type}")
    owner = user.get("displayName")
    if owner:
        lines.append(f"Owner: {owner}")
    return "\n".join(lines)


def generate_message_content(message: dict[str, Any], body_text: str) -> str:
    """Generate searchable text content for an email message."""
    lines = [f"Subject: {message.get('subject', 'No Subject')}"]

    sender = message.get("from", {}).get("emailAddress", {})
    if sender:
        lines.append(f"From: {sender.get('name', '')} <{sender.get('address', '')}>")

    to_addrs = [
        r.get("emailAddress", {}).get("address", "")
        for r in message.get("toRecipients", [])
    ]
    if to_addrs:
        lines.append(f"To: {', '.join(to_addrs)}")

    cc_addrs = [
        r.get("emailAddress", {}).get("address", "")
        for r in message.get("ccRecipients", [])
    ]
    if cc_addrs:
        lines.append(f"Cc: {', '.join(cc_addrs)}")

    received = message.get("receivedDateTime")
    if received:
        lines.append(f"Date: {received}")

    lines.append("")
    lines.append(body_text)
    return "\n".join(lines)


def generate_event_content(event: dict[str, Any]) -> str:
    """Generate searchable text content for a calendar event."""
    lines = [f"Event: {event.get('subject', 'Untitled Event')}"]

    start = event.get("start", {})
    end = event.get("end", {})
    if start.get("dateTime"):
        lines.append(f"Start: {start['dateTime']} ({start.get('timeZone', 'UTC')})")
    if end.get("dateTime"):
        lines.append(f"End: {end['dateTime']} ({end.get('timeZone', 'UTC')})")

    location = event.get("location", {})
    if location.get("displayName"):
        lines.append(f"Location: {location['displayName']}")

    organizer = event.get("organizer", {}).get("emailAddress", {})
    if organizer:
        lines.append(
            f"Organizer: {organizer.get('name', '')} <{organizer.get('address', '')}>"
        )

    attendees = event.get("attendees", [])
    if attendees:
        names = [a.get("emailAddress", {}).get("address", "") for a in attendees]
        lines.append(f"Attendees: {', '.join(names)}")

    if event.get("isAllDay"):
        lines.append("All-day event")

    if event.get("isCancelled"):
        lines.append("CANCELLED")

    body = event.get("body", {})
    if body.get("content"):
        lines.append("")
        content = body["content"]
        if body.get("contentType", "").lower() == "html":
            import re

            content = re.sub(r"<[^>]+>", " ", content)
            content = re.sub(r"\s+", " ", content).strip()
        lines.append(content)

    return "\n".join(lines)


def _parse_iso(value: str | None) -> datetime | None:
    if not value:
        return None
    try:
        return datetime.fromisoformat(value.replace("Z", "+00:00"))
    except (ValueError, TypeError):
        return None


def _parse_graph_datetime(dt_obj: dict[str, Any] | None) -> datetime | None:
    """Parse Graph API dateTime object {dateTime: '...', timeZone: '...'}."""
    if not dt_obj:
        return None
    raw = dt_obj.get("dateTime")
    if not raw:
        return None
    try:
        # Graph returns naive datetimes with a separate timeZone field.
        # For indexing purposes we treat them as UTC.
        parsed = datetime.fromisoformat(raw)
        if parsed.tzinfo is None:
            parsed = parsed.replace(tzinfo=timezone.utc)
        return parsed
    except (ValueError, TypeError):
        return None

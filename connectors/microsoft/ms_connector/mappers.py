"""Map Microsoft Graph API responses to Omni Document models."""

from __future__ import annotations

import re
from datetime import date, datetime, timezone
from typing import TYPE_CHECKING, Any

from omni_connector import Document, DocumentMetadata, DocumentPermissions

if TYPE_CHECKING:
    from .syncers.teams import TeamsChatMessageGroup, TeamsMessageGroup

_HTML_TAG_RE = re.compile(r"<[^>]+>")
_WHITESPACE_RE = re.compile(r"\s+")


def strip_html(html: str) -> str:
    """Naive HTML tag stripping for message/event bodies."""
    text = _HTML_TAG_RE.sub(" ", html)
    text = _WHITESPACE_RE.sub(" ", text)
    return text.strip()


_MIME_TO_CONTENT_TYPE: dict[str, str] = {
    "application/vnd.openxmlformats-officedocument.wordprocessingml.document": "document",
    "application/msword": "document",
    "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet": "spreadsheet",
    "application/vnd.ms-excel": "spreadsheet",
    "application/vnd.openxmlformats-officedocument.presentationml.presentation": "presentation",
    "application/vnd.ms-powerpoint": "presentation",
    "application/pdf": "pdf",
}


def map_drive_item_to_document(
    item: dict[str, Any],
    content_id: str,
    source_type: str = "one_drive",
    graph_permissions: list[dict[str, Any]] | None = None,
    user_cache: dict[str, str] | None = None,
    group_cache: dict[str, str] | None = None,
    owner_email: str | None = None,
    site_id: str | None = None,
) -> Document:
    """Map a OneDrive/SharePoint driveItem to an Omni Document.

    Resolves Graph API permission entries into Omni's permission model
    using pre-built user_cache (id→email) and group_cache (id→email).
    """
    parent_ref = item.get("parentReference", {})
    drive_id = parent_ref.get("driveId", "unknown")
    item_id = item["id"]

    if source_type == "share_point" and site_id:
        external_id = f"sharepoint:{site_id}:{item_id}"
    else:
        external_id = f"onedrive:{drive_id}:{item_id}"

    file_info = item.get("file", {})
    mime_type = file_info.get("mimeType")
    size = item.get("size")

    doc_perms = _resolve_graph_permissions(
        graph_permissions or [],
        user_cache or {},
        group_cache or {},
        owner_email,
    )

    return Document(
        external_id=external_id,
        title=item.get("name", "Untitled"),
        content_id=content_id,
        metadata=DocumentMetadata(
            created_at=_parse_iso(item.get("createdDateTime")),
            updated_at=_parse_iso(item.get("lastModifiedDateTime")),
            url=item.get("webUrl"),
            mime_type=mime_type,
            content_type=_MIME_TO_CONTENT_TYPE.get(mime_type) if mime_type else None,
            size=str(size) if size is not None else None,
            path=parent_ref.get("path"),
            extra={
                "drive_id": drive_id,
                "item_id": item_id,
            },
        ),
        permissions=doc_perms,
        attributes={
            "source_type": source_type,
        },
    )


def _resolve_graph_permissions(
    graph_permissions: list[dict[str, Any]],
    user_cache: dict[str, str],
    group_cache: dict[str, str],
    owner_email: str | None,
) -> DocumentPermissions:
    """Map Microsoft Graph permission entries to Omni DocumentPermissions."""
    is_public = False
    users: set[str] = set()
    groups: set[str] = set()

    for perm in graph_permissions:
        # Sharing link permissions
        link = perm.get("link")
        if link:
            scope = link.get("scope", "")
            if scope == "anonymous":
                is_public = True
            elif scope == "organization":
                # Org-wide link — everyone in the tenant can access
                is_public = True

            # Specific-people links have grantedToIdentitiesV2
            for identity in perm.get("grantedToIdentitiesV2", []):
                _resolve_identity(identity, user_cache, group_cache, users, groups)
            # Fallback to deprecated grantedToIdentities
            for identity in perm.get("grantedToIdentities", []):
                _resolve_identity(identity, user_cache, group_cache, users, groups)

        # Direct user/group grants
        granted_to = perm.get("grantedToV2") or perm.get("grantedTo")
        if granted_to:
            _resolve_identity(granted_to, user_cache, group_cache, users, groups)

        # Email invitations
        invitation = perm.get("invitation")
        if invitation:
            email = invitation.get("email")
            if email:
                users.add(email.lower())

    # Fallback: if no permissions resolved, use the drive owner
    if not is_public and not users and not groups and owner_email:
        users.add(owner_email.lower())

    return DocumentPermissions(
        public=is_public,
        users=sorted(users),
        groups=sorted(groups),
    )


def _resolve_identity(
    identity: dict[str, Any],
    user_cache: dict[str, str],
    group_cache: dict[str, str],
    users: set[str],
    groups: set[str],
) -> None:
    """Resolve a Graph identitySet to user emails or group emails."""
    # Check for site group
    site_group = identity.get("siteGroup")
    if site_group:
        group_id = site_group.get("id", "")
        if group_id in group_cache:
            groups.add(group_cache[group_id].lower())
        return

    # Check for user
    user = identity.get("user")
    if user:
        user_id = user.get("id", "")
        if user_id in user_cache:
            users.add(user_cache[user_id].lower())


def _collect_message_participants(message: dict[str, Any]) -> set[str]:
    """Collect all participant emails (from, to, cc) from a message."""
    participants: set[str] = set()
    sender_email = message.get("from", {}).get("emailAddress", {}).get("address")
    if sender_email:
        participants.add(sender_email.lower())
    for recipient in message.get("toRecipients", []):
        addr = recipient.get("emailAddress", {}).get("address")
        if addr:
            participants.add(addr.lower())
    for recipient in message.get("ccRecipients", []):
        addr = recipient.get("emailAddress", {}).get("address")
        if addr:
            participants.add(addr.lower())
    return participants


def map_message_to_document(
    message: dict[str, Any],
    content_id: str,
) -> Document:
    """Map an Outlook message to an Omni Document.

    Uses internetMessageId as external_id to deduplicate the same message
    across multiple users' mailboxes. Permissions include all participants.
    """
    msg_id = message["id"]
    internet_msg_id = message.get("internetMessageId") or msg_id
    external_id = f"mail:{internet_msg_id}"

    sender = message.get("from", {}).get("emailAddress", {})
    sender_name = sender.get("name") or sender.get("address")
    sender_email = sender.get("address", "").lower()
    participants = _collect_message_participants(message)

    to_addrs = [
        r.get("emailAddress", {}).get("address", "").lower()
        for r in message.get("toRecipients", [])
        if r.get("emailAddress", {}).get("address")
    ]
    cc_addrs = [
        r.get("emailAddress", {}).get("address", "").lower()
        for r in message.get("ccRecipients", [])
        if r.get("emailAddress", {}).get("address")
    ]

    attributes: dict[str, Any] = {
        "source_type": "outlook",
    }
    if sender_email:
        attributes["sender"] = sender_email
    if to_addrs:
        attributes["to"] = to_addrs
    if cc_addrs:
        attributes["cc"] = cc_addrs
    sent_dt = message.get("sentDateTime")
    if sent_dt:
        date_str = sent_dt[:10] if len(sent_dt) >= 10 else sent_dt
        attributes["date"] = date_str

    return Document(
        external_id=external_id,
        title=message.get("subject") or "No Subject",
        content_id=content_id,
        metadata=DocumentMetadata(
            author=sender_name,
            created_at=_parse_iso(message.get("sentDateTime")),
            updated_at=_parse_iso(message.get("receivedDateTime")),
            url=message.get("webLink"),
            content_type="email",
            mime_type="message/rfc822",
            extra={
                "message_id": msg_id,
                "has_attachments": message.get("hasAttachments", False),
            },
        ),
        permissions=DocumentPermissions(
            public=False,
            users=sorted(participants),
        ),
        attributes=attributes,
    )


def map_attachment_to_document(
    attachment: dict[str, Any],
    message: dict[str, Any],
    content_id: str,
) -> Document:
    """Map an Outlook mail attachment to an Omni Document.

    Each attachment is indexed as a separate document. Permissions
    are inherited from the parent message participants.
    """
    internet_msg_id = message.get("internetMessageId") or message["id"]
    att_id = attachment["id"]
    external_id = f"mail:{internet_msg_id}:att:{att_id}"

    filename = attachment.get("name", "Untitled Attachment")
    participants = _collect_message_participants(message)

    return Document(
        external_id=external_id,
        title=filename,
        content_id=content_id,
        metadata=DocumentMetadata(
            created_at=_parse_iso(message.get("receivedDateTime")),
            updated_at=_parse_iso(message.get("receivedDateTime")),
            url=message.get("webLink"),
            content_type=attachment.get("contentType"),
            mime_type=attachment.get("contentType"),
            size=str(attachment["size"]) if attachment.get("size") else None,
            extra={
                "parent_message_id": internet_msg_id,
                "attachment_id": att_id,
            },
        ),
        permissions=DocumentPermissions(
            public=False,
            users=sorted(participants),
        ),
        attributes={
            "source_type": "outlook",
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

    attributes: dict[str, Any] = {
        "source_type": "outlook_calendar",
    }
    org_addr = organizer.get("address", "").lower()
    if org_addr:
        attributes["organizer"] = org_addr
    if attendee_emails:
        attributes["attendees"] = [e.lower() for e in attendee_emails]
    if start_dt:
        attributes["date"] = start_dt.strftime("%Y-%m-%d")

    return Document(
        external_id=external_id,
        title=event.get("subject") or "Untitled Event",
        content_id=content_id,
        metadata=DocumentMetadata(
            author=organizer_name,
            created_at=start_dt,
            updated_at=start_dt,
            url=event.get("webLink"),
            content_type="event",
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
        attributes=attributes,
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
            content = strip_html(content)
        lines.append(content)

    return "\n".join(lines)


def _parse_iso(value: str | None) -> datetime | None:
    if not value:
        return None
    try:
        return datetime.fromisoformat(value.replace("Z", "+00:00"))
    except (ValueError, TypeError):
        return None


def map_teams_messages_to_document(
    group: TeamsMessageGroup,
    content_id: str,
) -> Document:
    """Map a group of Teams channel messages to an Omni Document."""
    external_id = group.external_id

    if group.is_thread:
        title = f"Thread in {group.channel_name} - {group.team_name}"
    else:
        title = f"{group.channel_name} - {group.team_name} - {group.date}"

    authors = sorted(set(group.authors))

    return Document(
        external_id=external_id,
        title=title,
        content_id=content_id,
        metadata=DocumentMetadata(
            author=authors[0] if len(authors) == 1 else "Multiple authors",
            created_at=group.first_timestamp,
            updated_at=group.last_timestamp,
            content_type="message",
            mime_type="text/plain",
            path=f"{group.team_name}/{group.channel_name}",
            extra={
                "teams": {
                    "team_id": group.team_id,
                    "channel_id": group.channel_id,
                    "message_count": group.message_count,
                    "authors": authors,
                    "date": str(group.date),
                }
            },
        ),
        permissions=group.permissions,
        attributes={
            "source_type": "ms_teams",
            "team_name": group.team_name,
            "channel_name": group.channel_name,
            "is_thread": group.is_thread,
        },
    )


def generate_teams_message_content(
    messages: list[tuple[dict[str, Any], str]],
) -> str:
    """Generate searchable text content from Teams messages.

    Each entry is (raw_message_dict, sender_display_name).
    """
    lines = []
    for msg, sender_name in messages:
        timestamp = msg.get("createdDateTime", "")
        body = msg.get("body", {})
        content = body.get("content", "")
        if body.get("contentType", "").lower() == "html":
            content = strip_html(content)
        lines.append(f"{sender_name} [{timestamp}]: {content}")
    return "\n\n".join(lines)


def map_teams_chat_messages_to_document(
    group: TeamsChatMessageGroup,
    content_id: str,
) -> Document:
    """Map a group of Teams chat messages to an Omni Document."""
    external_id = group.external_id

    if group.chat_topic:
        title = f"{group.chat_topic} - {group.date}"
    elif group.chat_type == "oneOnOne":
        names = group.participant_names[:2]
        title = f"Chat: {' & '.join(names)} - {group.date}"
    elif group.chat_type == "meeting":
        title = f"Meeting chat - {group.date}"
    else:
        names = group.participant_names[:3]
        suffix = (
            f" +{len(group.participant_names) - 3}"
            if len(group.participant_names) > 3
            else ""
        )
        title = f"Group chat: {', '.join(names)}{suffix} - {group.date}"

    authors = sorted(set(group.authors))

    return Document(
        external_id=external_id,
        title=title,
        content_id=content_id,
        metadata=DocumentMetadata(
            author=authors[0] if len(authors) == 1 else "Multiple authors",
            created_at=group.first_timestamp,
            updated_at=group.last_timestamp,
            content_type="message",
            mime_type="text/plain",
            extra={
                "teams_chat": {
                    "chat_id": group.chat_id,
                    "chat_type": group.chat_type,
                    "message_count": group.message_count,
                    "authors": authors,
                    "date": str(group.date),
                }
            },
        ),
        permissions=group.permissions,
        attributes={
            "source_type": "ms_teams",
            "chat_type": group.chat_type,
        },
    )


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

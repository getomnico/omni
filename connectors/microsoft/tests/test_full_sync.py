"""Integration tests: each Microsoft 365 source type syncs independently."""

import os
from unittest.mock import patch

import pytest
import httpx

from omni_connector.testing import count_events, get_events, wait_for_sync

GROUP_ID = "grp-eng-001"

pytestmark = pytest.mark.integration

USER_ID = "user-001"
DRIVE_ID = "drive-abc"
ITEM_ID = "item-001"
MSG_ID = "msg-001"
EVENT_ID = "evt-001"
SITE_ID = "site-001"
SP_DRIVE_ID = "sp-drive-001"
SP_ITEM_ID = "sp-item-001"


def _make_user() -> dict:
    return {
        "id": USER_ID,
        "displayName": "Alice Smith",
        "mail": "alice@contoso.com",
        "userPrincipalName": "alice@contoso.com",
    }


async def test_onedrive_sync(
    harness, seed, onedrive_source_id, mock_graph_api, cm_client: httpx.AsyncClient
):
    mock_graph_api.add_user(_make_user())
    mock_graph_api.add_drive_item(
        USER_ID,
        {
            "id": ITEM_ID,
            "name": "report.txt",
            "file": {"mimeType": "text/plain"},
            "size": 1024,
            "webUrl": "https://contoso-my.sharepoint.com/personal/alice/Documents/report.txt",
            "createdDateTime": "2024-03-10T08:00:00Z",
            "lastModifiedDateTime": "2024-06-15T12:30:00Z",
            "parentReference": {
                "driveId": DRIVE_ID,
                "path": "/drive/root:/Documents",
            },
        },
    )
    mock_graph_api.set_file_content(DRIVE_ID, ITEM_ID, b"Quarterly report content")

    resp = await cm_client.post(
        "/sync",
        json={"source_id": onedrive_source_id, "sync_type": "full"},
    )
    assert resp.status_code == 200, resp.text
    sync_run_id = resp.json()["sync_run_id"]

    row = await wait_for_sync(harness.db_pool, sync_run_id, timeout=60)
    assert (
        row["status"] == "completed"
    ), f"Sync ended with status={row['status']}, error={row.get('error_message')}"

    n_events = await count_events(
        harness.db_pool, onedrive_source_id, "document_created"
    )
    assert n_events == 1, f"Expected 1 document_created event, got {n_events}"

    events = await get_events(harness.db_pool, onedrive_source_id)
    doc_ids = {
        e["payload"]["document_id"]
        for e in events
        if e["event_type"] == "document_created"
    }
    assert any(
        did.startswith("onedrive:") for did in doc_ids
    ), f"No onedrive doc in {doc_ids}"

    state = await seed.get_connector_state(onedrive_source_id)
    assert state is not None, "connector_state should be saved after sync"


async def test_outlook_sync(
    harness, seed, outlook_source_id, mock_graph_api, cm_client: httpx.AsyncClient
):
    mock_graph_api.add_user(_make_user())
    mock_graph_api.add_mail_message(
        USER_ID,
        {
            "id": MSG_ID,
            "internetMessageId": "<msg001@contoso.com>",
            "subject": "Project Update",
            "bodyPreview": "Here is the latest update...",
            "body": {
                "contentType": "text",
                "content": "Here is the latest update on the project.",
            },
            "from": {
                "emailAddress": {"name": "Bob Jones", "address": "bob@contoso.com"}
            },
            "toRecipients": [
                {
                    "emailAddress": {
                        "name": "Alice Smith",
                        "address": "alice@contoso.com",
                    }
                }
            ],
            "ccRecipients": [],
            "receivedDateTime": "2024-06-20T09:00:00Z",
            "sentDateTime": "2024-06-20T08:55:00Z",
            "webLink": "https://outlook.office365.com/mail/inbox/msg-001",
            "hasAttachments": False,
        },
    )

    resp = await cm_client.post(
        "/sync",
        json={"source_id": outlook_source_id, "sync_type": "full"},
    )
    assert resp.status_code == 200, resp.text
    sync_run_id = resp.json()["sync_run_id"]

    row = await wait_for_sync(harness.db_pool, sync_run_id, timeout=60)
    assert (
        row["status"] == "completed"
    ), f"Sync ended with status={row['status']}, error={row.get('error_message')}"

    n_events = await count_events(
        harness.db_pool, outlook_source_id, "document_created"
    )
    assert n_events == 1, f"Expected 1 document_created event, got {n_events}"

    events = await get_events(harness.db_pool, outlook_source_id)
    doc_ids = {
        e["payload"]["document_id"]
        for e in events
        if e["event_type"] == "document_created"
    }
    assert any(did.startswith("mail:") for did in doc_ids), f"No mail doc in {doc_ids}"

    state = await seed.get_connector_state(outlook_source_id)
    assert state is not None, "connector_state should be saved after sync"


async def test_outlook_calendar_sync(
    harness,
    seed,
    outlook_calendar_source_id,
    mock_graph_api,
    cm_client: httpx.AsyncClient,
):
    mock_graph_api.add_user(_make_user())
    mock_graph_api.add_calendar_event(
        USER_ID,
        {
            "id": EVENT_ID,
            "subject": "Sprint Planning",
            "body": {"contentType": "text", "content": "Discuss sprint goals."},
            "start": {"dateTime": "2024-06-25T10:00:00", "timeZone": "UTC"},
            "end": {"dateTime": "2024-06-25T11:00:00", "timeZone": "UTC"},
            "location": {"displayName": "Conference Room A"},
            "organizer": {
                "emailAddress": {"name": "Alice Smith", "address": "alice@contoso.com"}
            },
            "attendees": [
                {
                    "emailAddress": {"name": "Bob Jones", "address": "bob@contoso.com"},
                    "type": "required",
                }
            ],
            "webLink": "https://outlook.office365.com/calendar/evt-001",
            "isAllDay": False,
            "isCancelled": False,
        },
    )

    resp = await cm_client.post(
        "/sync",
        json={"source_id": outlook_calendar_source_id, "sync_type": "full"},
    )
    assert resp.status_code == 200, resp.text
    sync_run_id = resp.json()["sync_run_id"]

    row = await wait_for_sync(harness.db_pool, sync_run_id, timeout=60)
    assert (
        row["status"] == "completed"
    ), f"Sync ended with status={row['status']}, error={row.get('error_message')}"

    n_events = await count_events(
        harness.db_pool, outlook_calendar_source_id, "document_created"
    )
    assert n_events == 1, f"Expected 1 document_created event, got {n_events}"

    events = await get_events(harness.db_pool, outlook_calendar_source_id)
    doc_ids = {
        e["payload"]["document_id"]
        for e in events
        if e["event_type"] == "document_created"
    }
    assert any(
        did.startswith("calendar:") for did in doc_ids
    ), f"No calendar doc in {doc_ids}"

    state = await seed.get_connector_state(outlook_calendar_source_id)
    assert state is not None, "connector_state should be saved after sync"


async def test_sharepoint_sync(
    harness, seed, sharepoint_source_id, mock_graph_api, cm_client: httpx.AsyncClient
):
    mock_graph_api.add_site(
        {
            "id": SITE_ID,
            "displayName": "Engineering",
            "webUrl": "https://contoso.sharepoint.com/sites/engineering",
        }
    )
    mock_graph_api.add_site_drive_item(
        SITE_ID,
        {
            "id": SP_ITEM_ID,
            "name": "design-doc.md",
            "file": {"mimeType": "text/markdown"},
            "size": 2048,
            "webUrl": "https://contoso.sharepoint.com/sites/engineering/Documents/design-doc.md",
            "createdDateTime": "2024-04-01T10:00:00Z",
            "lastModifiedDateTime": "2024-06-10T14:00:00Z",
            "parentReference": {
                "driveId": SP_DRIVE_ID,
                "path": "/drive/root:/Documents",
            },
        },
    )
    mock_graph_api.set_file_content(
        SP_DRIVE_ID, SP_ITEM_ID, b"# Design Document\nArchitecture overview"
    )

    resp = await cm_client.post(
        "/sync",
        json={"source_id": sharepoint_source_id, "sync_type": "full"},
    )
    assert resp.status_code == 200, resp.text
    sync_run_id = resp.json()["sync_run_id"]

    row = await wait_for_sync(harness.db_pool, sync_run_id, timeout=60)
    assert (
        row["status"] == "completed"
    ), f"Sync ended with status={row['status']}, error={row.get('error_message')}"

    n_events = await count_events(
        harness.db_pool, sharepoint_source_id, "document_created"
    )
    assert n_events == 1, f"Expected 1 document_created event, got {n_events}"

    events = await get_events(harness.db_pool, sharepoint_source_id)
    doc_ids = {
        e["payload"]["document_id"]
        for e in events
        if e["event_type"] == "document_created"
    }
    assert any(
        did.startswith("sharepoint:") for did in doc_ids
    ), f"No sharepoint doc in {doc_ids}"

    state = await seed.get_connector_state(sharepoint_source_id)
    assert state is not None, "connector_state should be saved after sync"


async def test_group_membership_sync(
    harness, seed, onedrive_source_id, mock_graph_api, cm_client: httpx.AsyncClient
):
    mock_graph_api.add_user(_make_user())
    mock_graph_api.add_group(
        {
            "id": GROUP_ID,
            "displayName": "Engineering",
            "mail": "engineering@contoso.com",
            "mailEnabled": True,
            "securityEnabled": True,
        }
    )
    mock_graph_api.add_group_member(
        GROUP_ID,
        {
            "id": "user-001",
            "displayName": "Alice Smith",
            "mail": "alice@contoso.com",
            "userPrincipalName": "alice@contoso.com",
        },
    )
    mock_graph_api.add_group_member(
        GROUP_ID,
        {
            "id": "user-002",
            "displayName": "Bob Jones",
            "mail": "bob@contoso.com",
            "userPrincipalName": "bob@contoso.com",
        },
    )

    # Add a drive item so the sync has something to process
    mock_graph_api.add_drive_item(
        USER_ID,
        {
            "id": ITEM_ID,
            "name": "doc.txt",
            "file": {"mimeType": "text/plain"},
            "size": 100,
            "webUrl": "https://contoso.com/doc.txt",
            "createdDateTime": "2026-01-01T00:00:00Z",
            "lastModifiedDateTime": "2026-01-01T00:00:00Z",
            "parentReference": {"driveId": DRIVE_ID, "path": "/drive/root:/"},
        },
    )
    mock_graph_api.set_file_content(DRIVE_ID, ITEM_ID, b"test content")

    resp = await cm_client.post(
        "/sync",
        json={"source_id": onedrive_source_id, "sync_type": "full"},
    )
    assert resp.status_code == 200, resp.text
    sync_run_id = resp.json()["sync_run_id"]

    row = await wait_for_sync(harness.db_pool, sync_run_id, timeout=60)
    assert (
        row["status"] == "completed"
    ), f"Sync ended with status={row['status']}, error={row.get('error_message')}"

    # Verify group membership sync events were emitted
    n_group_events = await count_events(
        harness.db_pool, onedrive_source_id, "group_membership_sync"
    )
    assert (
        n_group_events >= 1
    ), f"Expected at least 1 group_membership_sync event, got {n_group_events}"

    events = await get_events(harness.db_pool, onedrive_source_id)
    group_events = [e for e in events if e["event_type"] == "group_membership_sync"]
    assert len(group_events) >= 1
    payload = group_events[0]["payload"]
    assert payload["group_email"] == "engineering@contoso.com"
    assert set(payload["member_emails"]) == {"alice@contoso.com", "bob@contoso.com"}


# ---------------------------------------------------------------------------
# Teams tests
# ---------------------------------------------------------------------------

TEAM_ID = "team-eng-001"
CHANNEL_ID = "channel-general-001"
PRIVATE_CHANNEL_ID = "channel-private-001"


def _make_teams_user() -> dict:
    return {
        "id": USER_ID,
        "displayName": "Alice Smith",
        "mail": "alice@contoso.com",
        "userPrincipalName": "alice@contoso.com",
    }


def _make_team() -> dict:
    return {
        "id": TEAM_ID,
        "displayName": "Engineering",
        "mail": "engineering@contoso.com",
        "description": "Engineering team",
    }


def _make_channel(
    channel_id: str = CHANNEL_ID,
    name: str = "General",
    membership_type: str = "standard",
) -> dict:
    return {
        "id": channel_id,
        "displayName": name,
        "membershipType": membership_type,
        "description": f"{name} channel",
    }


def _make_message(
    msg_id: str,
    text: str,
    sender_name: str = "Alice Smith",
    sender_id: str = USER_ID,
    created: str = "2025-01-15T10:00:00Z",
    reply_to: str | None = None,
    attachments: list | None = None,
) -> dict:
    msg: dict = {
        "id": msg_id,
        "messageType": "message",
        "body": {"contentType": "text", "content": text},
        "from": {"user": {"id": sender_id, "displayName": sender_name}},
        "createdDateTime": created,
        "lastModifiedDateTime": created,
        "attachments": attachments or [],
        "mentions": [],
        "reactions": [],
    }
    if reply_to:
        msg["replyToId"] = reply_to
    return msg


async def test_teams_basic_sync(
    harness, seed, ms_teams_source_id, mock_graph_api, cm_client: httpx.AsyncClient
):
    """Basic Teams sync: one team, one channel, a few messages."""
    mock_graph_api.add_user(_make_teams_user())
    mock_graph_api.add_group_member(
        TEAM_ID,
        {
            "id": USER_ID,
            "displayName": "Alice Smith",
            "mail": "alice@contoso.com",
            "userPrincipalName": "alice@contoso.com",
        },
    )
    mock_graph_api.add_team(_make_team())
    mock_graph_api.add_team_channel(TEAM_ID, _make_channel())
    mock_graph_api.add_channel_message(
        TEAM_ID,
        CHANNEL_ID,
        _make_message("msg-t1", "Hello team!"),
    )
    mock_graph_api.add_channel_message(
        TEAM_ID,
        CHANNEL_ID,
        _make_message(
            "msg-t2",
            "Sprint update looks good",
            sender_name="Bob Jones",
            sender_id="user-002",
            created="2025-01-15T11:00:00Z",
        ),
    )

    resp = await cm_client.post(
        "/sync",
        json={"source_id": ms_teams_source_id, "sync_type": "full"},
    )
    assert resp.status_code == 200, resp.text
    sync_run_id = resp.json()["sync_run_id"]

    row = await wait_for_sync(harness.db_pool, sync_run_id, timeout=60)
    assert (
        row["status"] == "completed"
    ), f"Sync ended with status={row['status']}, error={row.get('error_message')}"

    n_events = await count_events(
        harness.db_pool, ms_teams_source_id, "document_created"
    )
    assert n_events >= 1, f"Expected at least 1 document_created event, got {n_events}"

    events = await get_events(harness.db_pool, ms_teams_source_id)
    doc_ids = {
        e["payload"]["document_id"]
        for e in events
        if e["event_type"] == "document_created"
    }
    assert any(
        did.startswith("teams:") for did in doc_ids
    ), f"No teams doc in {doc_ids}"

    state = await seed.get_connector_state(ms_teams_source_id)
    assert state is not None, "connector_state should be saved after sync"
    assert "delta_tokens" in state


async def test_teams_thread_sync(
    harness, seed, ms_teams_source_id, mock_graph_api, cm_client: httpx.AsyncClient
):
    """Thread messages create a separate thread document."""
    mock_graph_api.add_user(_make_teams_user())
    mock_graph_api.add_group_member(
        TEAM_ID,
        {
            "id": USER_ID,
            "displayName": "Alice Smith",
            "mail": "alice@contoso.com",
            "userPrincipalName": "alice@contoso.com",
        },
    )
    mock_graph_api.add_team(_make_team())
    mock_graph_api.add_team_channel(TEAM_ID, _make_channel())

    # Root message
    mock_graph_api.add_channel_message(
        TEAM_ID,
        CHANNEL_ID,
        _make_message("msg-root", "Who's working on the API refactor?"),
    )
    # Reply
    mock_graph_api.add_message_reply(
        TEAM_ID,
        CHANNEL_ID,
        "msg-root",
        _make_message(
            "msg-reply-1",
            "I am! Almost done with the endpoint changes.",
            sender_name="Bob Jones",
            sender_id="user-002",
            created="2025-01-15T10:05:00Z",
            reply_to="msg-root",
        ),
    )

    resp = await cm_client.post(
        "/sync",
        json={"source_id": ms_teams_source_id, "sync_type": "full"},
    )
    assert resp.status_code == 200, resp.text
    sync_run_id = resp.json()["sync_run_id"]

    row = await wait_for_sync(harness.db_pool, sync_run_id, timeout=60)
    assert (
        row["status"] == "completed"
    ), f"Sync ended with status={row['status']}, error={row.get('error_message')}"

    events = await get_events(harness.db_pool, ms_teams_source_id)
    doc_ids = {
        e["payload"]["document_id"]
        for e in events
        if e["event_type"] == "document_created"
    }
    thread_docs = [did for did in doc_ids if ":thread:" in did]
    assert len(thread_docs) >= 1, f"Expected a thread document, got {doc_ids}"


async def test_teams_private_channel_permissions(
    harness, seed, ms_teams_source_id, mock_graph_api, cm_client: httpx.AsyncClient
):
    """Private channel uses explicit member list for permissions."""
    mock_graph_api.add_user(_make_teams_user())
    mock_graph_api.add_user(
        {
            "id": "user-002",
            "displayName": "Bob Jones",
            "mail": "bob@contoso.com",
            "userPrincipalName": "bob@contoso.com",
        }
    )
    mock_graph_api.add_group_member(
        TEAM_ID,
        {
            "id": USER_ID,
            "displayName": "Alice Smith",
            "mail": "alice@contoso.com",
            "userPrincipalName": "alice@contoso.com",
        },
    )
    mock_graph_api.add_team(_make_team())
    mock_graph_api.add_team_channel(
        TEAM_ID,
        _make_channel(PRIVATE_CHANNEL_ID, "Secret Project", "private"),
    )
    mock_graph_api.add_channel_member(
        TEAM_ID,
        PRIVATE_CHANNEL_ID,
        {
            "id": "mem-1",
            "displayName": "Alice Smith",
            "email": "alice@contoso.com",
            "userId": USER_ID,
        },
    )
    mock_graph_api.add_channel_message(
        TEAM_ID,
        PRIVATE_CHANNEL_ID,
        _make_message("msg-priv-1", "Confidential discussion here"),
    )

    resp = await cm_client.post(
        "/sync",
        json={"source_id": ms_teams_source_id, "sync_type": "full"},
    )
    assert resp.status_code == 200, resp.text
    sync_run_id = resp.json()["sync_run_id"]

    row = await wait_for_sync(harness.db_pool, sync_run_id, timeout=60)
    assert (
        row["status"] == "completed"
    ), f"Sync ended with status={row['status']}, error={row.get('error_message')}"

    n_events = await count_events(
        harness.db_pool, ms_teams_source_id, "document_created"
    )
    assert n_events >= 1, f"Expected at least 1 document_created event, got {n_events}"


async def test_teams_system_messages_filtered(
    harness, seed, ms_teams_source_id, mock_graph_api, cm_client: httpx.AsyncClient
):
    """System event messages (chatEvent) should be filtered out."""
    mock_graph_api.add_user(_make_teams_user())
    mock_graph_api.add_group_member(
        TEAM_ID,
        {
            "id": USER_ID,
            "displayName": "Alice Smith",
            "mail": "alice@contoso.com",
            "userPrincipalName": "alice@contoso.com",
        },
    )
    mock_graph_api.add_team(_make_team())
    mock_graph_api.add_team_channel(TEAM_ID, _make_channel())
    # System message - should be skipped
    mock_graph_api.add_channel_message(
        TEAM_ID,
        CHANNEL_ID,
        {
            "id": "msg-sys-1",
            "messageType": "systemEventMessage",
            "body": {"contentType": "html", "content": "<systemEventMessage/>"},
            "from": None,
            "createdDateTime": "2025-01-15T09:00:00Z",
            "lastModifiedDateTime": "2025-01-15T09:00:00Z",
            "attachments": [],
            "mentions": [],
            "reactions": [],
        },
    )

    resp = await cm_client.post(
        "/sync",
        json={"source_id": ms_teams_source_id, "sync_type": "full"},
    )
    assert resp.status_code == 200, resp.text
    sync_run_id = resp.json()["sync_run_id"]

    row = await wait_for_sync(harness.db_pool, sync_run_id, timeout=60)
    assert (
        row["status"] == "completed"
    ), f"Sync ended with status={row['status']}, error={row.get('error_message')}"

    # No message documents should be created (only system message)
    n_events = await count_events(
        harness.db_pool, ms_teams_source_id, "document_created"
    )
    assert (
        n_events == 0
    ), f"Expected 0 document_created events (system msgs filtered), got {n_events}"


async def test_teams_file_attachment(
    harness, seed, ms_teams_source_id, mock_graph_api, cm_client: httpx.AsyncClient
):
    """File attachments resolve via Shares API and emit with sharepoint: external ID."""
    import base64

    mock_graph_api.add_user(_make_teams_user())
    mock_graph_api.add_group_member(
        TEAM_ID,
        {
            "id": USER_ID,
            "displayName": "Alice Smith",
            "mail": "alice@contoso.com",
            "userPrincipalName": "alice@contoso.com",
        },
    )
    mock_graph_api.add_team(_make_team())
    mock_graph_api.add_team_channel(TEAM_ID, _make_channel())

    content_url = (
        "https://contoso.sharepoint.com/sites/Engineering/Shared Documents/report.pdf"
    )
    share_token = "u!" + base64.urlsafe_b64encode(content_url.encode()).decode().rstrip(
        "="
    )

    attachment_drive_id = "sp-drive-teams"
    attachment_item_id = "sp-item-teams"
    attachment_site_id = "site-eng-001"

    mock_graph_api.set_share_drive_item(
        share_token,
        {
            "id": attachment_item_id,
            "name": "report.pdf",
            "file": {"mimeType": "application/pdf"},
            "size": 4096,
            "webUrl": content_url,
            "createdDateTime": "2025-01-10T08:00:00Z",
            "lastModifiedDateTime": "2025-01-14T12:00:00Z",
            "parentReference": {
                "driveId": attachment_drive_id,
                "siteId": attachment_site_id,
                "path": "/drive/root:/Shared Documents",
            },
        },
    )
    mock_graph_api.set_file_content(
        attachment_drive_id, attachment_item_id, b"PDF file content here"
    )

    mock_graph_api.add_channel_message(
        TEAM_ID,
        CHANNEL_ID,
        _make_message(
            "msg-attach-1",
            "Here's the quarterly report",
            attachments=[
                {
                    "id": "att-001",
                    "contentType": "reference",
                    "contentUrl": content_url,
                    "name": "report.pdf",
                }
            ],
        ),
    )

    resp = await cm_client.post(
        "/sync",
        json={"source_id": ms_teams_source_id, "sync_type": "full"},
    )
    assert resp.status_code == 200, resp.text
    sync_run_id = resp.json()["sync_run_id"]

    row = await wait_for_sync(harness.db_pool, sync_run_id, timeout=60)
    assert (
        row["status"] == "completed"
    ), f"Sync ended with status={row['status']}, error={row.get('error_message')}"

    events = await get_events(harness.db_pool, ms_teams_source_id)
    doc_ids = {
        e["payload"]["document_id"]
        for e in events
        if e["event_type"] == "document_created"
    }
    # Should have both a teams: message doc and a sharepoint: file doc
    assert any(
        did.startswith("teams:") for did in doc_ids
    ), f"No teams message doc in {doc_ids}"
    assert any(
        did.startswith("sharepoint:") for did in doc_ids
    ), f"No sharepoint file doc (attachment) in {doc_ids}"


# ---------------------------------------------------------------------------
# Max age filtering tests
# ---------------------------------------------------------------------------


async def test_mail_max_age_filters_old_messages(
    harness, seed, mock_graph_server, mock_graph_api, cm_client: httpx.AsyncClient
):
    """Messages older than MS_365_MAX_AGE_DAYS are excluded on initial sync."""
    source_id = await _create_ms_source(
        seed, mock_graph_server, mock_graph_api, "outlook"
    )
    mock_graph_api.add_user(_make_user())
    # Recent message — should be indexed
    mock_graph_api.add_mail_message(
        USER_ID,
        {
            "id": "msg-recent",
            "internetMessageId": "<recent@contoso.com>",
            "subject": "Recent Update",
            "bodyPreview": "Recent content",
            "body": {"contentType": "text", "content": "Recent content"},
            "from": {
                "emailAddress": {"name": "Bob Jones", "address": "bob@contoso.com"}
            },
            "toRecipients": [
                {
                    "emailAddress": {
                        "name": "Alice Smith",
                        "address": "alice@contoso.com",
                    }
                }
            ],
            "ccRecipients": [],
            "receivedDateTime": "2026-03-01T09:00:00Z",
            "sentDateTime": "2026-03-01T08:55:00Z",
            "webLink": "https://outlook.office365.com/mail/inbox/msg-recent",
            "hasAttachments": False,
        },
    )
    # Old message — should be filtered out
    mock_graph_api.add_mail_message(
        USER_ID,
        {
            "id": "msg-old",
            "internetMessageId": "<old@contoso.com>",
            "subject": "Ancient Update",
            "bodyPreview": "Old content",
            "body": {"contentType": "text", "content": "Old content"},
            "from": {
                "emailAddress": {"name": "Bob Jones", "address": "bob@contoso.com"}
            },
            "toRecipients": [
                {
                    "emailAddress": {
                        "name": "Alice Smith",
                        "address": "alice@contoso.com",
                    }
                }
            ],
            "ccRecipients": [],
            "receivedDateTime": "2020-01-01T09:00:00Z",
            "sentDateTime": "2020-01-01T08:55:00Z",
            "webLink": "https://outlook.office365.com/mail/inbox/msg-old",
            "hasAttachments": False,
        },
    )

    with patch.dict(os.environ, {"MS_365_MAX_AGE_DAYS": "365"}):
        import ms_connector.syncers.base as base_mod

        original = base_mod.DEFAULT_MAX_AGE_DAYS
        base_mod.DEFAULT_MAX_AGE_DAYS = 365
        try:
            resp = await cm_client.post(
                "/sync",
                json={"source_id": source_id, "sync_type": "full"},
            )
            assert resp.status_code == 200, resp.text
            sync_run_id = resp.json()["sync_run_id"]

            row = await wait_for_sync(harness.db_pool, sync_run_id, timeout=60)
            assert row["status"] == "completed", (
                f"Sync ended with status={row['status']}, "
                f"error={row.get('error_message')}"
            )

            events = await get_events(harness.db_pool, source_id)
            doc_ids = {
                e["payload"]["document_id"]
                for e in events
                if e["event_type"] == "document_created"
            }
            assert any(
                "recent@contoso.com" in did for did in doc_ids
            ), f"Recent message should be indexed, got {doc_ids}"
            assert not any(
                "old@contoso.com" in did for did in doc_ids
            ), f"Old message should be filtered out, got {doc_ids}"
        finally:
            base_mod.DEFAULT_MAX_AGE_DAYS = original


async def test_onedrive_max_age_filters_old_files(
    harness, seed, mock_graph_server, mock_graph_api, cm_client: httpx.AsyncClient
):
    """Files older than MS_365_MAX_AGE_DAYS are excluded on initial sync."""
    source_id = await _create_ms_source(
        seed, mock_graph_server, mock_graph_api, "one_drive"
    )
    mock_graph_api.add_user(_make_user())
    # Recent file — should be indexed
    mock_graph_api.add_drive_item(
        USER_ID,
        {
            "id": "item-recent",
            "name": "recent.txt",
            "file": {"mimeType": "text/plain"},
            "size": 100,
            "webUrl": "https://contoso.com/recent.txt",
            "createdDateTime": "2026-03-01T08:00:00Z",
            "lastModifiedDateTime": "2026-03-01T12:00:00Z",
            "parentReference": {"driveId": DRIVE_ID, "path": "/drive/root:/"},
        },
    )
    mock_graph_api.set_file_content(DRIVE_ID, "item-recent", b"recent content")
    # Old file — should be filtered out
    mock_graph_api.add_drive_item(
        USER_ID,
        {
            "id": "item-old",
            "name": "old.txt",
            "file": {"mimeType": "text/plain"},
            "size": 100,
            "webUrl": "https://contoso.com/old.txt",
            "createdDateTime": "2020-01-01T08:00:00Z",
            "lastModifiedDateTime": "2020-01-01T12:00:00Z",
            "parentReference": {"driveId": DRIVE_ID, "path": "/drive/root:/"},
        },
    )
    mock_graph_api.set_file_content(DRIVE_ID, "item-old", b"old content")

    import ms_connector.syncers.base as base_mod

    original = base_mod.DEFAULT_MAX_AGE_DAYS
    base_mod.DEFAULT_MAX_AGE_DAYS = 365
    try:
        resp = await cm_client.post(
            "/sync",
            json={"source_id": source_id, "sync_type": "full"},
        )
        assert resp.status_code == 200, resp.text
        sync_run_id = resp.json()["sync_run_id"]

        row = await wait_for_sync(harness.db_pool, sync_run_id, timeout=60)
        assert row["status"] == "completed", (
            f"Sync ended with status={row['status']}, "
            f"error={row.get('error_message')}"
        )

        events = await get_events(harness.db_pool, source_id)
        doc_ids = {
            e["payload"]["document_id"]
            for e in events
            if e["event_type"] == "document_created"
        }
        assert any(
            "item-recent" in did for did in doc_ids
        ), f"Recent file should be indexed, got {doc_ids}"
        assert not any(
            "item-old" in did for did in doc_ids
        ), f"Old file should be filtered out, got {doc_ids}"
    finally:
        base_mod.DEFAULT_MAX_AGE_DAYS = original

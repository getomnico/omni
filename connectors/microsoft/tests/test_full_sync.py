"""Integration tests: each Microsoft 365 source type syncs independently."""

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
            "createdDateTime": "2024-01-01T00:00:00Z",
            "lastModifiedDateTime": "2024-01-01T00:00:00Z",
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

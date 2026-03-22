"""Integration tests: workspace-level permission group and group membership sync."""

import pytest
import httpx

from omni_connector.testing import count_events, get_events, wait_for_sync

from .conftest import _block_payload

pytestmark = pytest.mark.integration

PAGE_ID = "pg-00000000-0000-0000-0000-000000000010"
DB_ID = "db-00000000-0000-0000-0000-000000000010"
ENTRY_PAGE_ID = "pg-00000000-0000-0000-0000-000000000011"


async def test_workspace_group_membership_emitted(
    harness, seed, source_id, mock_notion_api, cm_client: httpx.AsyncClient
):
    """Group membership event is emitted with person-type users, excluding bots."""
    mock_notion_api.add_user("user-001", "Alice", email="alice@example.com")
    mock_notion_api.add_user("user-002", "Bob", email="bob@example.com")
    mock_notion_api.add_user("bot-001", "My Bot", user_type="bot")

    mock_notion_api.add_page(
        PAGE_ID,
        "Test Page",
        [_block_payload("blk-100", "paragraph", "Hello")],
    )

    resp = await cm_client.post(
        "/sync", json={"source_id": source_id, "sync_type": "full"}
    )
    assert resp.status_code == 200, resp.text
    sync_run_id = resp.json()["sync_run_id"]

    row = await wait_for_sync(harness.db_pool, sync_run_id, timeout=30)
    assert (
        row["status"] == "completed"
    ), f"status={row['status']}, error={row.get('error_message')}"

    events = await get_events(harness.db_pool, source_id)
    group_events = [e for e in events if e["event_type"] == "group_membership_sync"]
    assert (
        len(group_events) == 1
    ), f"Expected 1 group_membership_sync event, got {len(group_events)}"

    payload = group_events[0]["payload"]
    assert payload["group_email"] == f"notion:workspace:{source_id}"
    assert payload["group_name"] == "Test Workspace"
    assert sorted(payload["member_emails"]) == ["alice@example.com", "bob@example.com"]


async def test_documents_have_workspace_permission_group(
    harness, seed, source_id, mock_notion_api, cm_client: httpx.AsyncClient
):
    """All emitted documents carry the workspace permission group."""
    mock_notion_api.add_user("user-001", "Alice", email="alice@example.com")

    mock_notion_api.add_page(
        PAGE_ID,
        "Standalone Page",
        [_block_payload("blk-101", "paragraph", "Content")],
    )

    db_schema = {
        "Name": {"id": "title", "name": "Name", "type": "title", "title": {}},
    }
    mock_notion_api.add_database(DB_ID, "My Database", db_schema)
    mock_notion_api.add_database_entry(
        DB_ID,
        ENTRY_PAGE_ID,
        "Entry One",
        {
            "Name": {
                "id": "title",
                "type": "title",
                "title": [
                    {
                        "type": "text",
                        "text": {"content": "Entry One"},
                        "plain_text": "Entry One",
                    }
                ],
            }
        },
        [_block_payload("blk-102", "paragraph", "Entry content")],
    )

    resp = await cm_client.post(
        "/sync", json={"source_id": source_id, "sync_type": "full"}
    )
    assert resp.status_code == 200, resp.text
    sync_run_id = resp.json()["sync_run_id"]

    row = await wait_for_sync(harness.db_pool, sync_run_id, timeout=30)
    assert row["status"] == "completed"

    events = await get_events(harness.db_pool, source_id)
    doc_events = [e for e in events if e["event_type"] == "document_created"]
    assert len(doc_events) == 3

    expected_group = f"notion:workspace:{source_id}"
    for event in doc_events:
        perms = event["payload"]["permissions"]
        assert (
            expected_group in perms["groups"]
        ), f"Document {event['payload']['document_id']} missing workspace group"
        assert perms["public"] is False


async def test_member_without_email_skipped(
    harness, seed, source_id, mock_notion_api, cm_client: httpx.AsyncClient
):
    """Members without an email are excluded from the workspace group."""
    mock_notion_api.add_user("user-001", "Alice", email="alice@example.com")
    mock_notion_api.add_user("user-003", "No Email", email=None)

    mock_notion_api.add_page(
        PAGE_ID,
        "Test Page",
        [_block_payload("blk-103", "paragraph", "Hello")],
    )

    resp = await cm_client.post(
        "/sync", json={"source_id": source_id, "sync_type": "full"}
    )
    assert resp.status_code == 200, resp.text
    sync_run_id = resp.json()["sync_run_id"]

    row = await wait_for_sync(harness.db_pool, sync_run_id, timeout=30)
    assert row["status"] == "completed"

    events = await get_events(harness.db_pool, source_id)
    group_events = [e for e in events if e["event_type"] == "group_membership_sync"]
    assert len(group_events) == 1

    payload = group_events[0]["payload"]
    assert payload["member_emails"] == ["alice@example.com"]

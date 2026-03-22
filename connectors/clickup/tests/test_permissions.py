"""Integration tests: permission inheritance and group membership sync."""

import pytest
import httpx

from omni_connector.testing import count_events, get_events, wait_for_sync

pytestmark = pytest.mark.integration


async def test_workspace_group_membership_emitted(
    harness, seed, source_id, mock_clickup_api, cm_client: httpx.AsyncClient
):
    mock_clickup_api.add_workspace(
        "team_1",
        "Test Workspace",
        members=[
            {
                "user": {"id": 1, "username": "alice", "email": "alice@test.com"},
                "role": 1,
            },
            {"user": {"id": 2, "username": "bob", "email": "bob@test.com"}, "role": 3},
            {
                "user": {"id": 3, "username": "guest", "email": "guest@test.com"},
                "role": 4,
            },
        ],
    )
    mock_clickup_api.add_space("team_1", "space_1", "Engineering")
    mock_clickup_api.add_folderless_list("space_1", "list_1", "Backlog")
    mock_clickup_api.add_task(
        "team_1", task_id="task_1", name="A task", list_id="list_1"
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

    n_group_events = await count_events(
        harness.db_pool, source_id, "group_membership_sync"
    )
    assert (
        n_group_events >= 1
    ), f"Expected >=1 group_membership_sync event, got {n_group_events}"

    events = await get_events(harness.db_pool, source_id)
    group_events = [e for e in events if e["event_type"] == "group_membership_sync"]

    workspace_events = [
        e
        for e in group_events
        if e["payload"]["group_email"] == "clickup:workspace:team_1"
    ]
    assert len(workspace_events) == 1
    payload = workspace_events[0]["payload"]
    assert set(payload["member_emails"]) == {"alice@test.com", "bob@test.com"}


async def test_private_space_group_membership_emitted(
    harness, seed, source_id, mock_clickup_api, cm_client: httpx.AsyncClient
):
    mock_clickup_api.add_workspace("team_1", "Test Workspace")
    mock_clickup_api.add_space(
        "team_1",
        "space_priv",
        "Secret Space",
        private=True,
        members=[
            {
                "user": {"id": 10, "username": "carol", "email": "carol@test.com"},
                "role": 3,
            },
        ],
    )
    mock_clickup_api.add_folderless_list("space_priv", "list_p", "Private List")
    mock_clickup_api.add_task(
        "team_1", task_id="task_p", name="Private task", list_id="list_p"
    )

    resp = await cm_client.post(
        "/sync", json={"source_id": source_id, "sync_type": "full"}
    )
    sync_run_id = resp.json()["sync_run_id"]
    row = await wait_for_sync(harness.db_pool, sync_run_id, timeout=30)
    assert (
        row["status"] == "completed"
    ), f"status={row['status']}, error={row.get('error_message')}"

    events = await get_events(harness.db_pool, source_id)
    group_events = [e for e in events if e["event_type"] == "group_membership_sync"]

    space_events = [
        e
        for e in group_events
        if e["payload"]["group_email"] == "clickup:space:space_priv"
    ]
    assert len(space_events) == 1
    assert space_events[0]["payload"]["member_emails"] == ["carol@test.com"]


async def test_task_in_public_space_gets_workspace_group(
    harness, seed, source_id, mock_clickup_api, cm_client: httpx.AsyncClient
):
    mock_clickup_api.add_workspace("team_1", "Test Workspace")
    mock_clickup_api.add_space("team_1", "space_pub", "Public Space")
    mock_clickup_api.add_folderless_list("space_pub", "list_pub", "Public List")
    mock_clickup_api.add_task(
        "team_1", task_id="task_pub", name="Public task", list_id="list_pub"
    )

    resp = await cm_client.post(
        "/sync", json={"source_id": source_id, "sync_type": "full"}
    )
    sync_run_id = resp.json()["sync_run_id"]
    row = await wait_for_sync(harness.db_pool, sync_run_id, timeout=30)
    assert row["status"] == "completed"

    events = await get_events(harness.db_pool, source_id)
    doc_events = [e for e in events if e["event_type"] == "document_created"]
    assert len(doc_events) >= 1

    payload = doc_events[0]["payload"]
    assert payload["permissions"]["groups"] == ["clickup:workspace:team_1"]


async def test_task_in_private_space_gets_space_group(
    harness, seed, source_id, mock_clickup_api, cm_client: httpx.AsyncClient
):
    mock_clickup_api.add_workspace("team_1", "Test Workspace")
    mock_clickup_api.add_space(
        "team_1",
        "space_sec",
        "Secret Space",
        private=True,
        members=[
            {
                "user": {"id": 10, "username": "carol", "email": "carol@test.com"},
                "role": 3,
            },
        ],
    )
    mock_clickup_api.add_folderless_list("space_sec", "list_sec", "Secret List")
    mock_clickup_api.add_task(
        "team_1", task_id="task_sec", name="Secret task", list_id="list_sec"
    )

    resp = await cm_client.post(
        "/sync", json={"source_id": source_id, "sync_type": "full"}
    )
    sync_run_id = resp.json()["sync_run_id"]
    row = await wait_for_sync(harness.db_pool, sync_run_id, timeout=30)
    assert row["status"] == "completed"

    events = await get_events(harness.db_pool, source_id)
    doc_events = [e for e in events if e["event_type"] == "document_created"]
    assert len(doc_events) >= 1

    payload = doc_events[0]["payload"]
    assert payload["permissions"]["groups"] == ["clickup:space:space_sec"]


async def test_member_without_email_skipped(
    harness, seed, source_id, mock_clickup_api, cm_client: httpx.AsyncClient
):
    mock_clickup_api.add_workspace(
        "team_1",
        "Test Workspace",
        members=[
            {
                "user": {"id": 1, "username": "alice", "email": "alice@test.com"},
                "role": 1,
            },
            {"user": {"id": 99, "username": "no-email-user"}, "role": 3},
        ],
    )
    mock_clickup_api.add_space("team_1", "space_1", "Engineering")
    mock_clickup_api.add_folderless_list("space_1", "list_1", "Backlog")
    mock_clickup_api.add_task(
        "team_1", task_id="task_1", name="A task", list_id="list_1"
    )

    resp = await cm_client.post(
        "/sync", json={"source_id": source_id, "sync_type": "full"}
    )
    sync_run_id = resp.json()["sync_run_id"]
    row = await wait_for_sync(harness.db_pool, sync_run_id, timeout=30)
    assert row["status"] == "completed"

    events = await get_events(harness.db_pool, source_id)
    group_events = [e for e in events if e["event_type"] == "group_membership_sync"]
    workspace_events = [
        e
        for e in group_events
        if e["payload"]["group_email"] == "clickup:workspace:team_1"
    ]
    assert len(workspace_events) == 1
    assert workspace_events[0]["payload"]["member_emails"] == ["alice@test.com"]

"""Integration tests: full sync creates task documents."""

import pytest
import httpx

from omni_connector.testing import count_events, wait_for_sync

pytestmark = pytest.mark.integration


async def test_full_sync_creates_task_documents(
    harness, seed, source_id, mock_clickup_api, cm_client: httpx.AsyncClient
):
    mock_clickup_api.add_workspace("team_1", "Test Workspace")
    mock_clickup_api.add_space("team_1", "space_1", "Engineering")
    mock_clickup_api.add_folderless_list("space_1", "list_1", "Backlog")
    mock_clickup_api.add_task("team_1", task_id="task_1", name="Fix login bug", list_id="list_1")
    mock_clickup_api.add_task(
        "team_1", task_id="task_2", name="Add search feature", list_id="list_1"
    )

    resp = await cm_client.post(
        "/sync",
        json={"source_id": source_id, "sync_type": "full"},
    )
    assert resp.status_code == 200, resp.text
    sync_run_id = resp.json()["sync_run_id"]

    row = await wait_for_sync(harness.db_pool, sync_run_id, timeout=30)
    assert row["status"] == "completed", (
        f"Sync ended with status={row['status']}, error={row.get('error_message')}"
    )

    n_events = await count_events(harness.db_pool, source_id, "document_created")
    assert n_events >= 2, f"Expected >=2 document_created events, got {n_events}"


async def test_full_sync_respects_space_filters(
    harness, seed, mock_clickup_server, mock_clickup_api, cm_client: httpx.AsyncClient
):
    mock_clickup_api.reset()
    filtered_source_id = await seed.create_source(
        source_type="clickup",
        config={
            "api_url": mock_clickup_server,
            "include_docs": False,
            "space_filters": ["space_allowed"],
        },
    )
    await seed.create_credentials(
        filtered_source_id,
        {"token": "pk_test_token_abc123"},
        provider="clickup",
        auth_type="api_key",
    )

    mock_clickup_api.add_workspace("team_1", "Test Workspace")
    mock_clickup_api.add_space("team_1", "space_allowed", "Engineering")
    mock_clickup_api.add_space("team_1", "space_blocked", "Marketing")
    mock_clickup_api.add_folderless_list("space_allowed", "list_allowed", "Backlog")
    mock_clickup_api.add_folderless_list("space_blocked", "list_blocked", "Campaigns")
    mock_clickup_api.add_task(
        "team_1", task_id="task_allowed", name="Index me", list_id="list_allowed"
    )
    mock_clickup_api.add_task(
        "team_1", task_id="task_blocked", name="Skip me", list_id="list_blocked"
    )

    resp = await cm_client.post(
        "/sync",
        json={"source_id": filtered_source_id, "sync_type": "full"},
    )
    assert resp.status_code == 200, resp.text
    sync_run_id = resp.json()["sync_run_id"]

    row = await wait_for_sync(harness.db_pool, sync_run_id, timeout=30)
    assert row["status"] == "completed"

    n_events = await count_events(harness.db_pool, filtered_source_id, "document_created")
    assert n_events == 1, f"Expected 1 selected-space document, got {n_events}"


async def test_full_sync_empty_space_filters_indexes_everything(
    harness, seed, mock_clickup_server, mock_clickup_api, cm_client: httpx.AsyncClient
):
    mock_clickup_api.reset()
    unfiltered_source_id = await seed.create_source(
        source_type="clickup",
        config={
            "api_url": mock_clickup_server,
            "include_docs": False,
            "space_filters": [],
        },
    )
    await seed.create_credentials(
        unfiltered_source_id,
        {"token": "pk_test_token_abc123"},
        provider="clickup",
        auth_type="api_key",
    )

    mock_clickup_api.add_workspace("team_1", "Test Workspace")
    mock_clickup_api.add_space("team_1", "space_1", "Engineering")
    mock_clickup_api.add_space("team_1", "space_2", "Marketing")
    mock_clickup_api.add_folderless_list("space_1", "list_1", "Backlog")
    mock_clickup_api.add_folderless_list("space_2", "list_2", "Campaigns")
    mock_clickup_api.add_task("team_1", task_id="task_1", name="Task 1", list_id="list_1")
    mock_clickup_api.add_task("team_1", task_id="task_2", name="Task 2", list_id="list_2")

    resp = await cm_client.post(
        "/sync",
        json={"source_id": unfiltered_source_id, "sync_type": "full"},
    )
    assert resp.status_code == 200, resp.text
    sync_run_id = resp.json()["sync_run_id"]

    row = await wait_for_sync(harness.db_pool, sync_run_id, timeout=30)
    assert row["status"] == "completed"

    n_events = await count_events(harness.db_pool, unfiltered_source_id, "document_created")
    assert n_events == 2, f"Expected 2 unfiltered documents, got {n_events}"


async def test_full_sync_includes_subtasks(
    harness, seed, source_id, mock_clickup_api, cm_client: httpx.AsyncClient
):
    mock_clickup_api.add_workspace("team_1", "Test Workspace")
    mock_clickup_api.add_space("team_1", "space_1", "Engineering")
    mock_clickup_api.add_folderless_list("space_1", "list_1", "Backlog")
    mock_clickup_api.add_task("team_1", task_id="task_parent", name="Parent task", list_id="list_1")
    mock_clickup_api.add_task(
        "team_1",
        task_id="task_child",
        name="Subtask",
        list_id="list_1",
        parent="task_parent",
    )

    resp = await cm_client.post(
        "/sync",
        json={"source_id": source_id, "sync_type": "full"},
    )
    sync_run_id = resp.json()["sync_run_id"]
    row = await wait_for_sync(harness.db_pool, sync_run_id, timeout=30)
    assert row["status"] == "completed"

    n_events = await count_events(harness.db_pool, source_id, "document_created")
    assert n_events >= 2, f"Expected >=2 events (parent + subtask), got {n_events}"


async def test_full_sync_includes_comments_in_content(
    harness, seed, source_id, mock_clickup_api, cm_client: httpx.AsyncClient
):
    mock_clickup_api.add_workspace("team_1", "Test Workspace")
    mock_clickup_api.add_space("team_1", "space_1", "Engineering")
    mock_clickup_api.add_folderless_list("space_1", "list_1", "Backlog")
    mock_clickup_api.add_task(
        "team_1", task_id="task_1", name="Bug with comments", list_id="list_1"
    )
    mock_clickup_api.add_comment("task_1", comment_id="c1", text="I can reproduce this")
    mock_clickup_api.add_comment("task_1", comment_id="c2", text="Fixed in latest commit")

    resp = await cm_client.post(
        "/sync",
        json={"source_id": source_id, "sync_type": "full"},
    )
    sync_run_id = resp.json()["sync_run_id"]
    row = await wait_for_sync(harness.db_pool, sync_run_id, timeout=30)
    assert row["status"] == "completed"

    n_events = await count_events(harness.db_pool, source_id, "document_created")
    assert n_events >= 1


async def test_full_sync_saves_checkpoint(
    harness, seed, source_id, mock_clickup_api, cm_client: httpx.AsyncClient
):
    mock_clickup_api.add_workspace("team_1", "Test Workspace")
    mock_clickup_api.add_space("team_1", "space_1", "Engineering")
    mock_clickup_api.add_folderless_list("space_1", "list_1", "Backlog")
    mock_clickup_api.add_task("team_1", task_id="task_1", name="Some task", list_id="list_1")

    resp = await cm_client.post(
        "/sync",
        json={"source_id": source_id, "sync_type": "full"},
    )
    sync_run_id = resp.json()["sync_run_id"]
    await wait_for_sync(harness.db_pool, sync_run_id, timeout=30)

    checkpoint = await seed.get_checkpoint(source_id)
    assert checkpoint is not None, "checkpoint should be saved after sync"
    assert "workspaces" in checkpoint
    assert "team_1" in checkpoint["workspaces"]


async def test_full_sync_scanned_count(
    harness, seed, source_id, mock_clickup_api, cm_client: httpx.AsyncClient
):
    mock_clickup_api.add_workspace("team_1", "Test Workspace")
    mock_clickup_api.add_space("team_1", "space_1", "Engineering")
    mock_clickup_api.add_folderless_list("space_1", "list_1", "Backlog")
    mock_clickup_api.add_task("team_1", task_id="t1", name="Task 1", list_id="list_1")
    mock_clickup_api.add_task("team_1", task_id="t2", name="Task 2", list_id="list_1")
    mock_clickup_api.add_task("team_1", task_id="t3", name="Task 3", list_id="list_1")

    resp = await cm_client.post(
        "/sync",
        json={"source_id": source_id, "sync_type": "full"},
    )
    sync_run_id = resp.json()["sync_run_id"]
    row = await wait_for_sync(harness.db_pool, sync_run_id, timeout=30)

    assert row["status"] == "completed"
    assert row["documents_scanned"] >= 3, (
        f"Expected >=3 documents_scanned, got {row['documents_scanned']}"
    )

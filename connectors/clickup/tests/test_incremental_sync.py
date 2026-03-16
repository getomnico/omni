"""Integration tests: incremental sync picks up only updated tasks."""

import pytest
import httpx

from omni_connector.testing import count_events, wait_for_sync

pytestmark = pytest.mark.integration


async def test_incremental_sync_after_full(
    harness, seed, source_id, mock_clickup_api, cm_client: httpx.AsyncClient
):
    mock_clickup_api.add_workspace("team_1", "Test Workspace")
    mock_clickup_api.add_space("team_1", "space_1", "Engineering")
    mock_clickup_api.add_folderless_list("space_1", "list_1", "Backlog")
    mock_clickup_api.add_task(
        "team_1",
        task_id="task_1",
        name="First task",
        list_id="list_1",
        date_updated="1709884800000",
    )

    # Full sync
    resp = await cm_client.post(
        "/sync",
        json={"source_id": source_id, "sync_type": "full"},
    )
    sync_run_id = resp.json()["sync_run_id"]
    await wait_for_sync(harness.db_pool, sync_run_id, timeout=30)
    full_event_count = await count_events(harness.db_pool, source_id)

    # Add a new task with a later date_updated
    mock_clickup_api.add_task(
        "team_1",
        task_id="task_2",
        name="Second task",
        list_id="list_1",
        date_updated="1710000000000",
    )

    # Incremental sync
    resp = await cm_client.post(
        "/sync",
        json={"source_id": source_id, "sync_type": "incremental"},
    )
    sync_run_id = resp.json()["sync_run_id"]
    row = await wait_for_sync(harness.db_pool, sync_run_id, timeout=30)
    assert row["status"] == "completed"

    total_events = await count_events(harness.db_pool, source_id)
    assert total_events > full_event_count, (
        f"Incremental sync should produce new events: "
        f"before={full_event_count}, after={total_events}"
    )

"""Integration tests: incremental sync picks up only new items."""

import pytest
import httpx

from omni_connector.testing import count_events, wait_for_sync

pytestmark = pytest.mark.integration


async def test_incremental_sync_after_full(
    harness, seed, source_id, mock_github_api, cm_client: httpx.AsyncClient
):
    mock_github_api.add_repo("octocat", "Hello-World")
    mock_github_api.add_issue("octocat", "Hello-World", 1, title="First issue")

    # Full sync
    resp = await cm_client.post(
        "/sync",
        json={"source_id": source_id, "sync_type": "full"},
    )
    sync_run_id = resp.json()["sync_run_id"]
    await wait_for_sync(harness.db_pool, sync_run_id, timeout=30)
    full_event_count = await count_events(harness.db_pool, source_id)

    # Add a new issue
    mock_github_api.add_issue("octocat", "Hello-World", 2, title="Second issue")

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

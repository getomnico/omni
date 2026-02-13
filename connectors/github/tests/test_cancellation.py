"""Integration tests: sync cancellation."""

import asyncio

import pytest
import httpx

from omni_connector.testing import wait_for_sync

pytestmark = pytest.mark.integration


async def test_cancel_running_sync(
    harness, seed, source_id, mock_github_api, cm_client: httpx.AsyncClient
):
    mock_github_api.add_repo("octocat", "Hello-World")
    for i in range(1, 51):
        mock_github_api.add_issue("octocat", "Hello-World", i, title=f"Issue {i}")

    resp = await cm_client.post(
        "/sync",
        json={"source_id": source_id, "sync_type": "full"},
    )
    assert resp.status_code == 200
    sync_run_id = resp.json()["sync_run_id"]

    # Give the sync a moment to start processing
    await asyncio.sleep(1)

    cancel_resp = await cm_client.post(f"/sync/{sync_run_id}/cancel")
    assert cancel_resp.status_code == 200

    row = await wait_for_sync(harness.db_pool, sync_run_id, timeout=30)
    assert row["status"] in (
        "cancelled",
        "failed",
    ), f"Expected cancelled or failed, got {row['status']}"

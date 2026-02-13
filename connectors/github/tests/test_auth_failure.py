"""Integration tests: authentication failures are handled correctly."""

import pytest
import httpx

from omni_connector.testing import wait_for_sync

pytestmark = pytest.mark.integration


async def test_bad_token_fails_sync(
    harness, seed, source_id, mock_github_api, cm_client: httpx.AsyncClient
):
    mock_github_api.add_repo("octocat", "Hello-World")
    mock_github_api.should_fail_auth = True

    try:
        resp = await cm_client.post(
            "/sync",
            json={"source_id": source_id, "sync_type": "full"},
        )
        assert resp.status_code == 200, resp.text
        sync_run_id = resp.json()["sync_run_id"]

        row = await wait_for_sync(harness.db_pool, sync_run_id, timeout=30)
        assert row["status"] == "failed"
        assert row["error_message"] is not None
        error_lower = row["error_message"].lower()
        assert (
            "auth" in error_lower
            or "401" in error_lower
            or "credentials" in error_lower
        ), f"Error message should mention auth: {row['error_message']}"
    finally:
        mock_github_api.should_fail_auth = False


async def test_missing_token_fails_sync(
    harness, seed, mock_github_server, cm_client: httpx.AsyncClient
):
    sid = await seed.create_source(
        source_type="github",
        config={"api_url": mock_github_server},
    )
    await seed.create_credentials(sid, {})

    resp = await cm_client.post(
        "/sync",
        json={"source_id": sid, "sync_type": "full"},
    )
    assert resp.status_code == 200, resp.text
    sync_run_id = resp.json()["sync_run_id"]

    row = await wait_for_sync(harness.db_pool, sync_run_id, timeout=30)
    assert row["status"] == "failed"
    assert row["error_message"] is not None
    assert (
        "token" in row["error_message"].lower()
    ), f"Error should mention missing token: {row['error_message']}"

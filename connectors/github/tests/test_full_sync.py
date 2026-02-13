"""Integration tests: full sync creates all document types."""

import pytest
import httpx

from omni_connector.testing import count_events, wait_for_sync

pytestmark = pytest.mark.integration


async def test_full_sync_creates_all_documents(
    harness, seed, source_id, mock_github_api, cm_client: httpx.AsyncClient
):
    mock_github_api.add_repo("octocat", "Hello-World", description="My first repo")
    mock_github_api.add_readme("octocat", "Hello-World", "# Hello World")
    mock_github_api.add_issue("octocat", "Hello-World", 1, title="Bug report")
    mock_github_api.add_pull_request("octocat", "Hello-World", 10, title="Fix bug")
    mock_github_api.add_discussion("octocat", "Hello-World", 5, title="How to deploy?")

    resp = await cm_client.post(
        "/sync",
        json={"source_id": source_id, "sync_type": "full"},
    )
    assert resp.status_code == 200, resp.text
    sync_run_id = resp.json()["sync_run_id"]

    row = await wait_for_sync(harness.db_pool, sync_run_id, timeout=30)
    assert (
        row["status"] == "completed"
    ), f"Sync ended with status={row['status']}, error={row.get('error_message')}"

    n_events = await count_events(harness.db_pool, source_id, "document_created")
    assert n_events >= 4, f"Expected >=4 document_created events, got {n_events}"


async def test_full_sync_saves_connector_state(
    harness, seed, source_id, mock_github_api, cm_client: httpx.AsyncClient
):
    mock_github_api.add_repo("octocat", "Hello-World")
    mock_github_api.add_issue("octocat", "Hello-World", 1)

    resp = await cm_client.post(
        "/sync",
        json={"source_id": source_id, "sync_type": "full"},
    )
    sync_run_id = resp.json()["sync_run_id"]
    await wait_for_sync(harness.db_pool, sync_run_id, timeout=30)

    state = await seed.get_connector_state(source_id)
    assert state is not None, "connector_state should be saved after sync"
    assert "repos" in state
    assert "octocat/Hello-World" in state["repos"]


async def test_full_sync_scanned_count(
    harness, seed, source_id, mock_github_api, cm_client: httpx.AsyncClient
):
    mock_github_api.add_repo("octocat", "Hello-World")
    mock_github_api.add_issue("octocat", "Hello-World", 1)
    mock_github_api.add_issue("octocat", "Hello-World", 2)
    mock_github_api.add_pull_request("octocat", "Hello-World", 10)

    resp = await cm_client.post(
        "/sync",
        json={"source_id": source_id, "sync_type": "full"},
    )
    sync_run_id = resp.json()["sync_run_id"]
    row = await wait_for_sync(harness.db_pool, sync_run_id, timeout=30)

    assert row["status"] == "completed"
    assert (
        row["documents_scanned"] >= 4
    ), f"Expected >=4 documents_scanned, got {row['documents_scanned']}"

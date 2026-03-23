"""Integration tests: permission inheritance and group membership sync."""

import pytest
import httpx

from omni_connector.testing import count_events, get_events, wait_for_sync

pytestmark = pytest.mark.integration


async def test_private_repo_group_membership_emitted(
    harness, seed, source_id, mock_github_api, cm_client: httpx.AsyncClient
):
    """Private repo collaborators should produce a GroupMembershipSync event."""
    mock_github_api.add_repo("acme", "secret", private=True)
    mock_github_api.add_collaborator("acme", "secret", "alice", uid=10)
    mock_github_api.add_collaborator("acme", "secret", "bob", uid=11)
    mock_github_api.set_user_email("alice", "alice@acme.com")
    mock_github_api.set_user_email("bob", "bob@acme.com")

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

    repo_events = [
        e
        for e in group_events
        if e["payload"]["group_email"] == "github:repo:acme/secret"
    ]
    assert len(repo_events) == 1
    assert set(repo_events[0]["payload"]["member_emails"]) == {
        "alice@acme.com",
        "bob@acme.com",
    }


async def test_public_repo_no_group_membership(
    harness, seed, source_id, mock_github_api, cm_client: httpx.AsyncClient
):
    """Public repos should not produce GroupMembershipSync events."""
    mock_github_api.add_repo("acme", "open-source")

    resp = await cm_client.post(
        "/sync", json={"source_id": source_id, "sync_type": "full"}
    )
    sync_run_id = resp.json()["sync_run_id"]
    row = await wait_for_sync(harness.db_pool, sync_run_id, timeout=30)
    assert row["status"] == "completed"

    n_group_events = await count_events(
        harness.db_pool, source_id, "group_membership_sync"
    )
    assert (
        n_group_events == 0
    ), f"Expected 0 group events for public repo, got {n_group_events}"


async def test_collaborator_without_email_skipped(
    harness, seed, source_id, mock_github_api, cm_client: httpx.AsyncClient
):
    """Collaborators without a public email should be excluded from group membership."""
    mock_github_api.add_repo("acme", "private-proj", private=True)
    mock_github_api.add_collaborator("acme", "private-proj", "alice", uid=10)
    mock_github_api.add_collaborator("acme", "private-proj", "no-email-user", uid=20)
    mock_github_api.set_user_email("alice", "alice@acme.com")
    mock_github_api.set_user_email("no-email-user", None)

    resp = await cm_client.post(
        "/sync", json={"source_id": source_id, "sync_type": "full"}
    )
    sync_run_id = resp.json()["sync_run_id"]
    row = await wait_for_sync(harness.db_pool, sync_run_id, timeout=30)
    assert row["status"] == "completed"

    events = await get_events(harness.db_pool, source_id)
    group_events = [e for e in events if e["event_type"] == "group_membership_sync"]

    repo_events = [
        e
        for e in group_events
        if e["payload"]["group_email"] == "github:repo:acme/private-proj"
    ]
    assert len(repo_events) == 1
    assert repo_events[0]["payload"]["member_emails"] == ["alice@acme.com"]


async def test_private_repo_documents_get_repo_group(
    harness, seed, source_id, mock_github_api, cm_client: httpx.AsyncClient
):
    """All documents in a private repo should have the repo group in permissions."""
    mock_github_api.add_repo("acme", "secret", private=True)
    mock_github_api.add_issue("acme", "secret", 1, title="Secret issue")
    mock_github_api.add_collaborator("acme", "secret", "alice", uid=10)
    mock_github_api.set_user_email("alice", "alice@acme.com")

    resp = await cm_client.post(
        "/sync", json={"source_id": source_id, "sync_type": "full"}
    )
    sync_run_id = resp.json()["sync_run_id"]
    row = await wait_for_sync(harness.db_pool, sync_run_id, timeout=30)
    assert row["status"] == "completed"

    events = await get_events(harness.db_pool, source_id)
    doc_events = [e for e in events if e["event_type"] == "document_created"]
    assert len(doc_events) >= 2  # repo + issue

    for event in doc_events:
        perms = event["payload"]["permissions"]
        assert perms["public"] is False
        assert perms["groups"] == ["github:repo:acme/secret"]

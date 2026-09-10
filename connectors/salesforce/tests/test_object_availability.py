"""Integration tests: sync skips Salesforce objects unavailable to the principal.

Salesforce editions and user licenses do not expose every standard object.
Global describe is the authoritative capability boundary: objects missing from
it must be skipped (with a warning/error) instead of aborting the sync with
INVALID_TYPE.
"""

from __future__ import annotations

import httpx
import pytest
from omni_connector.testing import get_events, wait_for_sync

from salesforce_connector.models import group_email
from tests.conftest import set_source_config

pytestmark = pytest.mark.integration


async def _run_sync(cm_client: httpx.AsyncClient, source_id: str) -> dict:
    resp = await cm_client.post("/sync", json={"source_id": source_id, "sync_type": "full"})
    assert resp.status_code == 200, resp.text
    return resp.json()


async def _sync_docs(
    harness, cm_client: httpx.AsyncClient, source_id: str
) -> tuple[dict, dict[str, dict]]:
    run = await _run_sync(cm_client, source_id)
    row = await wait_for_sync(harness.db_pool, run["sync_run_id"], timeout=40)
    assert row["status"] == "completed", f"status={row['status']} err={row.get('error_message')}"
    events = await get_events(harness.db_pool, source_id)
    docs: dict[str, dict] = {}
    for event in events:
        payload = event["payload"]
        if payload.get("type") == "document_created":
            docs[payload["document_id"]] = payload
    return row, docs


async def test_full_sync_skips_records_unavailable_to_principal(
    harness, seed, source_id, mock_salesforce_api, cm_client: httpx.AsyncClient
) -> None:
    """Objects missing from global describe are skipped, not fatal."""
    mock_salesforce_api.add_people_fixtures()
    mock_salesforce_api.add_account()
    mock_salesforce_api.add_case()
    mock_salesforce_api.add_task()
    mock_salesforce_api.hidden_objects.update({"Case", "Task"})

    row, docs = await _sync_docs(harness, cm_client, source_id)

    # Only the available object is scanned and emitted.
    assert row["documents_scanned"] == 1
    assert "Account:001000000000001" in docs
    assert not any(
        document_id.startswith(("Case:", "Task:")) for document_id in docs
    )


async def test_full_sync_without_hierarchy_tolerates_missing_user_role_field(
    harness,
    seed,
    source_id,
    mock_salesforce_api,
    mock_salesforce_server,
    cm_client: httpx.AsyncClient,
) -> None:
    """With hierarchy grants disabled, UserRoleId is not a required field.

    Organs without roles enabled do not expose UserRoleId. Once hierarchy
    grants are explicitly disabled the SELECT list is narrowed instead of
    aborting the sync, and non-role memberships still resolve.
    """
    await set_source_config(
        harness,
        source_id,
        {
            "instance_url": mock_salesforce_server,
            "grant_access_using_hierarchies": False,
        },
    )
    mock_salesforce_api.add_people_fixtures()
    mock_salesforce_api.add_account()
    mock_salesforce_api.hidden_fields.setdefault("User", set()).add("UserRoleId")

    row, docs = await _sync_docs(harness, cm_client, source_id)

    assert row["documents_scanned"] == 1
    assert "Account:001000000000001" in docs
    events = await get_events(harness.db_pool, source_id)
    person_events = [e["payload"] for e in events if e["payload"]["type"] == "person_sync"]
    active_emails = {e["person"]["email"] for e in person_events}
    assert active_emails == {"owner@example.com", "agent@example.com", "manager@example.com"}
    group_payloads = [
        e["payload"] for e in events if e["payload"]["type"] == "group_membership_sync"
    ]
    memberships = {g["group_email"]: set(g["member_emails"]) for g in group_payloads}
    # Non-role memberships still resolve without UserRoleId.
    assert memberships.get(group_email("00G000000000001")) == {"agent@example.com"}


async def test_full_sync_fails_when_share_state_is_unavailable(
    harness, seed, source_id, mock_salesforce_api, cm_client: httpx.AsyncClient
) -> None:
    """A share object that cannot be read must not commit under-granted docs."""
    mock_salesforce_api.add_people_fixtures()
    mock_salesforce_api.add_account()
    mock_salesforce_api.hidden_objects.update({"AccountShare"})

    run = await _run_sync(cm_client, source_id)
    row = await wait_for_sync(harness.db_pool, run["sync_run_id"], timeout=40)
    assert row["status"] == "failed"
    events = await get_events(harness.db_pool, source_id)
    assert not any(
        event["payload"].get("type") == "document_created" for event in events
    )

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


async def test_full_sync_skips_unavailable_share_object(
    harness, seed, source_id, mock_salesforce_api, cm_client: httpx.AsyncClient
) -> None:
    """A hidden share object drops only sharing rows, not the records."""
    mock_salesforce_api.add_people_fixtures()
    mock_salesforce_api.add_account()
    mock_salesforce_api.hidden_objects.update({"AccountShare"})

    row, docs = await _sync_docs(harness, cm_client, source_id)

    assert row["documents_scanned"] == 1
    account = docs["Account:001000000000001"]
    assert account["permissions"]["public"] is True
    assert "owner@example.com" in account["permissions"]["users"]

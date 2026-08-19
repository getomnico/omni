"""Integration tests: incremental (delta) sync and tombstones."""

from __future__ import annotations

import httpx
import pytest
from omni_connector.testing import get_events, wait_for_sync

from tests.conftest import _now_modstamp

pytestmark = pytest.mark.integration


async def test_incremental_sync_picks_up_changes_and_tombstones(
    harness, seed, source_id, mock_salesforce_api, cm_client: httpx.AsyncClient
) -> None:
    mock_salesforce_api.add_people_fixtures()
    mock_salesforce_api.add_account()

    # Baseline full sync.
    resp = await cm_client.post("/sync", json={"source_id": source_id, "sync_type": "full"})
    full_row = await wait_for_sync(harness.db_pool, resp.json()["sync_run_id"], timeout=40)
    assert full_row["status"] == "completed"

    # A new account created after the baseline, and the original one deleted.
    mock_salesforce_api.add_account(
        "001000000000002", name="NewCo", system_modstamp=_now_modstamp()
    )
    mock_salesforce_api.mark_deleted("Account", "001000000000001")

    resp = await cm_client.post("/sync", json={"source_id": source_id, "sync_type": "incremental"})
    run_id = resp.json()["sync_run_id"]
    row = await wait_for_sync(harness.db_pool, run_id, timeout=40)
    assert row["status"] == "completed", f"err={row.get('error_message')}"

    # Only the changed record is rescanned.
    assert row["documents_scanned"] == 1, row["documents_scanned"]

    events = await get_events(harness.db_pool, source_id)
    # New record is emitted as an update (indexer upserts).
    updated = [e["payload"] for e in events if e["payload"]["type"] == "document_updated"]
    assert any(p["document_id"] == "Account:001000000000002" for p in updated)
    # The baseline record is tombstoned.
    deleted = [e["payload"] for e in events if e["payload"]["type"] == "document_deleted"]
    assert any(p["document_id"] == "Account:001000000000001" for p in deleted)
    # The untouched baseline record is not re-emitted.
    assert not any(p["document_id"] == "Account:001000000000001" for p in updated)


async def test_incremental_sync_without_changes_is_cheap(
    harness, seed, source_id, mock_salesforce_api, cm_client: httpx.AsyncClient
) -> None:
    mock_salesforce_api.add_people_fixtures()
    mock_salesforce_api.add_account()

    resp = await cm_client.post("/sync", json={"source_id": source_id, "sync_type": "full"})
    await wait_for_sync(harness.db_pool, resp.json()["sync_run_id"], timeout=40)

    resp = await cm_client.post("/sync", json={"source_id": source_id, "sync_type": "incremental"})
    row = await wait_for_sync(harness.db_pool, resp.json()["sync_run_id"], timeout=40)
    assert row["status"] == "completed"

    # Watermarks prevent a second full pass: nothing to scan, nothing emitted.
    assert row["documents_scanned"] == 0
    events = await get_events(harness.db_pool, source_id)
    # Person/group events are re-emitted (idempotent), but no document updates.
    assert not any(e["payload"]["type"] in ("document_created", "document_updated") for e in events)

"""Integration tests: checkpoint persistence and watermark invalidation."""

from __future__ import annotations

import json

import httpx
import pytest
from omni_connector.testing import wait_for_sync

pytestmark = pytest.mark.integration


async def _run_sync(cm_client, source_id, sync_type) -> dict:
    resp = await cm_client.post("/sync", json={"source_id": source_id, "sync_type": sync_type})
    assert resp.status_code == 200, resp.text
    return resp.json()


async def test_checkpoint_persisted_after_full_sync(
    harness, seed, source_id, mock_salesforce_api, cm_client: httpx.AsyncClient
) -> None:
    mock_salesforce_api.add_people_fixtures()
    mock_salesforce_api.add_account()

    run = await _run_sync(cm_client, source_id, "full")
    row = await wait_for_sync(harness.db_pool, run["sync_run_id"], timeout=40)
    assert row["status"] == "completed"

    checkpoint_row = await harness.db_pool.fetchrow(
        "SELECT checkpoint FROM sources WHERE id = $1::char(26)", source_id
    )
    checkpoint = checkpoint_row["checkpoint"]
    assert checkpoint is not None
    assert checkpoint["version"] == 1
    assert checkpoint["records_synced"]["Account"] is True
    assert "Account" in checkpoint["watermarks"]

    # connector_state carries the schema fingerprint for invalidation checks.
    state_row = await harness.db_pool.fetchrow(
        "SELECT connector_state FROM sources WHERE id = $1::char(26)", source_id
    )
    assert state_row["connector_state"]["schema_fingerprint"]


async def test_mid_sync_checkpoint_is_granular(
    harness, seed, source_id, mock_salesforce_api, cm_client: httpx.AsyncClient
) -> None:
    """A run over many pages persists record_cursors, so a resume can pick
    up mid-object rather than redoing the object."""
    mock_salesforce_api.add_people_fixtures()
    # 2500 accounts: a full page (2000) plus a partial page.
    for i in range(2500):
        mock_salesforce_api.add_account(f"0010000000{i:04d}", name=f"Acme {i}")

    run = await _run_sync(cm_client, source_id, "full")
    row = await wait_for_sync(harness.db_pool, run["sync_run_id"], timeout=120)
    assert row["status"] == "completed"
    assert row["documents_scanned"] == 2500

    checkpoint_row = await harness.db_pool.fetchrow(
        "SELECT checkpoint FROM sources WHERE id = $1::char(26)", source_id
    )
    checkpoint = checkpoint_row["checkpoint"]
    cursor = checkpoint["record_cursors"]["Account"]
    # The final keyset cursor matches the last synced record.
    assert cursor["last_id"] == "00100000002499"


async def test_schema_change_invalidates_watermarks(
    harness, seed, source_id, mock_salesforce_api, mock_salesforce_server, cm_client
) -> None:
    """Changing the object set forces a full rescan even for an incremental run."""
    mock_salesforce_api.add_people_fixtures()
    mock_salesforce_api.add_account()

    run = await _run_sync(cm_client, source_id, "full")
    row = await wait_for_sync(harness.db_pool, run["sync_run_id"], timeout=40)
    assert row["status"] == "completed"

    # Incremental with no schema change: watermarks honored, nothing scanned.
    run = await _run_sync(cm_client, source_id, "incremental")
    row = await wait_for_sync(harness.db_pool, run["sync_run_id"], timeout=40)
    assert row["status"] == "completed"
    assert row["documents_scanned"] == 0

    # Change the enabled object set -> schema fingerprint changes.
    await harness.db_pool.execute(
        "UPDATE sources SET config = $2::jsonb WHERE id = $1::char(26)",
        source_id,
        json.dumps(
            {
                "instance_url": mock_salesforce_server,
                "enabled_objects": ["Account", "Contact"],
            }
        ),
    )

    run = await _run_sync(cm_client, source_id, "incremental")
    row = await wait_for_sync(harness.db_pool, run["sync_run_id"], timeout=40)
    assert row["status"] == "completed"
    # Watermarks were dropped: the Account set is rescanned in full.
    assert row["documents_scanned"] == 1

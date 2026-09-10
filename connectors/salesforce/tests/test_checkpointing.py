"""Integration tests: checkpoint persistence and watermark invalidation."""

from __future__ import annotations

import json

import httpx
import pytest
from omni_connector.testing import wait_for_sync

from tests.conftest import set_source_config

pytestmark = pytest.mark.integration


def _as_object(value: object) -> dict[str, object] | None:
    """asyncpg returns jsonb columns as strings unless a codec is registered."""
    if isinstance(value, str):
        return json.loads(value)
    return value if isinstance(value, dict) else None


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
    checkpoint = _as_object(checkpoint_row["checkpoint"])
    assert checkpoint is not None
    assert checkpoint["version"] == 2
    assert checkpoint["progress"] is None
    assert checkpoint["objects"]["Account"]["watermark"] is not None
    assert checkpoint["objects"]["Account"]["deletion_through"] is not None

    # Correctness fingerprints live on the promoted checkpoint (not
    # connector_state, which is shared with the realtime slot).
    assert checkpoint["schema_fingerprint"]
    assert checkpoint["resolved_fingerprint"]
    assert checkpoint["enabled_objects"]


async def test_completed_checkpoint_has_no_run_progress(
    harness, seed, source_id, mock_salesforce_api, cm_client: httpx.AsyncClient
) -> None:
    """A run over many pages commits per-object coverage but never leaks
    in-progress run state into the published source checkpoint."""
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
    checkpoint = _as_object(checkpoint_row["checkpoint"])
    assert checkpoint is not None
    # Published source checkpoints are committed-state-only; run cursors are
    # discarded on completion so a later fresh run cannot resume an old pass.
    assert checkpoint["progress"] is None
    assert checkpoint["objects"]["Account"]["watermark"] is not None
    assert checkpoint["objects"]["Account"]["deletion_through"] is not None


async def test_schema_change_invalidates_watermarks(
    harness, seed, source_id, mock_salesforce_api, mock_salesforce_server, cm_client
) -> None:
    """Adding an object forces a full re-emission even for an incremental run."""
    await set_source_config(
        harness,
        source_id,
        {"instance_url": mock_salesforce_server, "enabled_objects": ["Account"]},
    )
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

    # Add Contact (adding never leaves stale documents). The schema fingerprint
    # changes, so the enabled objects are fully re-emitted.
    await set_source_config(
        harness,
        source_id,
        {"instance_url": mock_salesforce_server, "enabled_objects": ["Account", "Contact"]},
    )
    mock_salesforce_api.add_contact()

    run = await _run_sync(cm_client, source_id, "incremental")
    row = await wait_for_sync(harness.db_pool, run["sync_run_id"], timeout=40)
    assert row["status"] == "completed"
    assert row["documents_scanned"] == 2

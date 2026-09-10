"""Integration tests: authentication failure surfaces a failed sync run."""

from __future__ import annotations

import httpx
import pytest
from omni_connector.testing import wait_for_sync

pytestmark = pytest.mark.integration


async def test_auth_failure_marks_sync_failed(
    harness, seed, source_id, mock_salesforce_api, cm_client: httpx.AsyncClient
) -> None:
    mock_salesforce_api.add_people_fixtures()
    mock_salesforce_api.add_account()
    mock_salesforce_api.should_fail_auth = True

    resp = await cm_client.post("/sync", json={"source_id": source_id, "sync_type": "full"})
    assert resp.status_code == 200
    row = await wait_for_sync(harness.db_pool, resp.json()["sync_run_id"], timeout=40)
    assert row["status"] == "failed"
    assert "Authentication" in (row.get("error_message") or "")

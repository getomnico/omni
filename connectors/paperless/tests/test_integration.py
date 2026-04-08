"""Integration tests: full sync indexes paperless-ngx documents."""

import pytest
import httpx

from omni_connector.testing import count_events, get_events, wait_for_sync

pytestmark = pytest.mark.integration


async def test_full_sync_indexes_documents(
    harness, seed, source_id, mock_paperless_api, cm_client: httpx.AsyncClient
):
    """Sync two documents and verify they appear as document_created events."""
    mock_paperless_api.add_document(
        doc_id=1,
        title="Invoice from ACME",
        content="Total: $1,234.56",
        correspondent=1,
        document_type=1,
        tags=[1],
        custom_fields=[{"field": 1, "value": "Project Alpha"}],
    )
    mock_paperless_api.add_document(
        doc_id=2,
        title="Receipt for office supplies",
        content="Staples order #12345",
        tags=[2],
    )

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
    assert n_events == 2, f"Expected 2 document_created events, got {n_events}"

    events = await get_events(harness.db_pool, source_id)
    doc_ids = {
        e["payload"]["document_id"]
        for e in events
        if e["event_type"] == "document_created"
    }
    assert f"paperless:{source_id}:1" in doc_ids
    assert f"paperless:{source_id}:2" in doc_ids

    state = await seed.get_connector_state(source_id)
    assert state is not None, "connector_state should be saved after sync"
    assert "last_sync_at" in state

    assert (
        row["documents_scanned"] >= 2
    ), f"Expected >=2 documents_scanned, got {row['documents_scanned']}"

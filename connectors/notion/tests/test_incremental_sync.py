"""Integration tests: incremental sync only re-emits items modified after the cutoff."""

import httpx
import pytest
from omni_connector.testing import count_events, get_events, wait_for_sync

from .conftest import _block_payload

pytestmark = pytest.mark.integration

OLD_PAGE_ID = "pg-00000000-0000-0000-0000-000000000aaa"
NEW_PAGE_ID = "pg-00000000-0000-0000-0000-000000000bbb"
NEWER_PAGE_ID = "pg-00000000-0000-0000-0000-000000000ccc"


async def test_incremental_sync_only_emits_modified_items(
    harness, seed, source_id, mock_notion_api, cm_client: httpx.AsyncClient
):
    """Pages older than last_sync_at are skipped; newer ones are re-emitted.

    Pages are returned by /v1/search in arbitrary order from the mock's dict —
    we explicitly sort the response server-side. The connector relies on a
    desc-by-last_edited_time sort param; this test asserts the early-break
    logic stops after seeing the cutoff.
    """
    # Page edited well before the cutoff — should not be emitted.
    mock_notion_api.add_page(
        OLD_PAGE_ID,
        "Stale Page",
        [_block_payload("blk-old", "paragraph", "old content")],
        last_edited_time="2024-01-01T00:00:00.000Z",
    )

    # Two pages edited after the cutoff — both should be emitted.
    mock_notion_api.add_page(
        NEW_PAGE_ID,
        "Recently Edited",
        [_block_payload("blk-new", "paragraph", "new content")],
        last_edited_time="2024-07-01T12:00:00.000Z",
    )
    mock_notion_api.add_page(
        NEWER_PAGE_ID,
        "Most Recently Edited",
        [_block_payload("blk-newer", "paragraph", "newer content")],
        last_edited_time="2024-08-01T12:00:00.000Z",
    )

    # Seed connector_state to simulate a prior successful sync. The cutoff
    # falls between the stale page and the two recent ones.
    cutoff = "2024-06-01T00:00:00.000Z"
    await seed.set_connector_state(source_id, {"last_sync_at": cutoff})

    resp = await cm_client.post(
        "/sync",
        json={"source_id": source_id, "sync_type": "incremental"},
    )
    assert resp.status_code == 200, resp.text
    sync_run_id = resp.json()["sync_run_id"]

    row = await wait_for_sync(harness.db_pool, sync_run_id, timeout=30)
    assert (
        row["status"] == "completed"
    ), f"status={row['status']}, error={row.get('error_message')}"

    n_events = await count_events(harness.db_pool, source_id, "document_created")
    assert n_events == 2, (
        f"Expected exactly 2 document_created events (the two post-cutoff pages); "
        f"got {n_events}. The stale page should be filtered out by the early-break."
    )

    events = await get_events(harness.db_pool, source_id)
    doc_ids = {
        e["payload"]["document_id"]
        for e in events
        if e["event_type"] == "document_created"
    }
    assert f"notion:page:{NEW_PAGE_ID}" in doc_ids
    assert f"notion:page:{NEWER_PAGE_ID}" in doc_ids
    assert f"notion:page:{OLD_PAGE_ID}" not in doc_ids

    state = await seed.get_connector_state(source_id)
    assert state is not None
    assert (
        state["last_sync_at"] > cutoff
    ), "last_sync_at should advance to the new run's timestamp"

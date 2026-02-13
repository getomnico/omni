"""Polling helpers and assertion utilities for integration tests."""

from __future__ import annotations

import asyncio
import json
from typing import Any

import asyncpg


async def wait_for_sync(
    pool: asyncpg.Pool,
    sync_run_id: str,
    timeout: float = 30,
) -> asyncpg.Record:
    """Poll sync_runs until the given run is no longer 'running'. Returns the final row."""
    elapsed = 0.0
    interval = 0.5
    while elapsed < timeout:
        row = await pool.fetchrow(
            "SELECT * FROM sync_runs WHERE id = $1::char(26)", sync_run_id
        )
        if row is None:
            raise ValueError(f"sync_run {sync_run_id} not found")
        if row["status"] != "running":
            return row
        await asyncio.sleep(interval)
        elapsed += interval
    raise TimeoutError(f"sync_run {sync_run_id} still running after {timeout}s")


async def count_events(
    pool: asyncpg.Pool,
    source_id: str,
    event_type: str | None = None,
) -> int:
    if event_type:
        row = await pool.fetchrow(
            "SELECT count(*) AS cnt FROM connector_events_queue "
            "WHERE source_id = $1::char(26) AND event_type = $2",
            source_id,
            event_type,
        )
    else:
        row = await pool.fetchrow(
            "SELECT count(*) AS cnt FROM connector_events_queue "
            "WHERE source_id = $1::char(26)",
            source_id,
        )
    return row["cnt"] if row else 0


async def get_events(
    pool: asyncpg.Pool,
    source_id: str,
) -> list[dict[str, Any]]:
    rows = await pool.fetch(
        "SELECT * FROM connector_events_queue "
        "WHERE source_id = $1::char(26) ORDER BY created_at",
        source_id,
    )
    results = []
    for row in rows:
        d = dict(row)
        if "payload" in d and isinstance(d["payload"], str):
            d["payload"] = json.loads(d["payload"])
        results.append(d)
    return results

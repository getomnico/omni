"""SOQL query construction and pagination helpers."""

from __future__ import annotations

from collections.abc import AsyncIterator, Mapping
from datetime import UTC, datetime

from .client import QueryResult, SalesforceClient
from .config import PAGE_SIZE
from .models import RecordCursor


async def iter_query_pages(client: SalesforceClient, soql: str) -> AsyncIterator[QueryResult]:
    """Yield every page of a query, following nextRecordsUrl tokens."""
    response = await client.query(soql)
    while True:
        yield response
        if response.done or response.next_records_url is None:
            return
        response = await client.query_more(response.next_records_url)


def soql_datetime(value: datetime) -> str:
    """Render a datetime as a SOQL UTC timestamp literal."""
    return value.astimezone(UTC).strftime("%Y-%m-%dT%H:%M:%SZ")


def full_scan_soql(
    object_type: str,
    fields: tuple[str, ...],
    cursor: RecordCursor | None,
    page_size: int = PAGE_SIZE,
) -> str:
    """SOQL for a resumable full scan ordered by Id (keyset pagination)."""
    clauses = []
    if cursor is not None and cursor.last_id is not None:
        clauses.append(f"Id > '{cursor.last_id}'")
    where = f"WHERE {' AND '.join(clauses)} " if clauses else ""
    return f"SELECT {', '.join(fields)} FROM {object_type} {where}ORDER BY Id LIMIT {page_size}"


def delta_scan_soql(
    object_type: str,
    fields: tuple[str, ...],
    cursor: RecordCursor | None,
    window_start: datetime,
    window_end: datetime,
    page_size: int = PAGE_SIZE,
) -> str:
    """SOQL for a resumable bounded incremental scan.

    Every page of a pass is constrained to the same fixed ``[window_start,
    window_end]`` window and keyset-ordered on ``(SystemModstamp, Id)``. A
    cursor that does not carry both keyset components is ignored so a
    malformed or partial resume restarts the window instead of skipping
    records.
    """
    start_literal = soql_datetime(window_start)
    end_literal = soql_datetime(window_end)
    clauses = [f"SystemModstamp >= {start_literal}", f"SystemModstamp <= {end_literal}"]
    if cursor is not None and cursor.is_delta_ready:
        assert cursor.last_system_modstamp is not None and cursor.last_id is not None
        clauses.append(
            f"(SystemModstamp > {cursor.last_system_modstamp}"
            f" OR (SystemModstamp = {cursor.last_system_modstamp}"
            f" AND Id > '{cursor.last_id}'))"
        )
    where = f"WHERE {' AND '.join(clauses)} "
    return (
        f"SELECT {', '.join(fields)} FROM {object_type} {where}"
        f"ORDER BY SystemModstamp ASC, Id ASC LIMIT {page_size}"
    )


def cursor_from_record(record: Mapping[str, object]) -> RecordCursor:
    """Extract the keyset position of the last record of a page."""
    record_id = record.get("Id")
    modstamp = record.get("SystemModstamp")
    return RecordCursor(
        last_id=str(record_id) if record_id is not None else None,
        last_system_modstamp=str(modstamp) if modstamp is not None else None,
    )

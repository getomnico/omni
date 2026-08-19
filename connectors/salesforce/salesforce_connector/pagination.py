"""SOQL query construction and pagination helpers."""

from __future__ import annotations

from collections.abc import AsyncIterator, Mapping

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
    watermark: str,
    page_size: int = PAGE_SIZE,
) -> str:
    """SOQL for a resumable incremental scan.

    Keyset on (SystemModstamp, Id): the cursor's last_system_modstamp/last_id
    continue the scan where it left off, otherwise everything at or after the
    watermark is returned (so a resume never skips records updated while the
    previous run was in flight).
    """
    clauses = []
    if cursor is not None and cursor.last_system_modstamp is not None:
        clauses.append(
            f"(SystemModstamp > {cursor.last_system_modstamp}"
            f" OR (SystemModstamp = {cursor.last_system_modstamp}"
            f" AND Id > '{cursor.last_id}'))"
        )
    else:
        clauses.append(f"SystemModstamp >= {watermark}")
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

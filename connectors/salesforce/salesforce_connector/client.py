"""Salesforce REST API client with typed responses and retry logic."""

from __future__ import annotations

import asyncio
import json
import logging
from collections.abc import Awaitable, Callable, Mapping
from dataclasses import dataclass
from datetime import datetime
from functools import wraps
from typing import TypeVar

from simple_salesforce import (  # type: ignore[attr-defined]
    Salesforce,
    SalesforceAuthenticationFailed,
    SalesforceError,
)

from .config import API_VERSION

logger = logging.getLogger(__name__)

T = TypeVar("T")


class SalesforceClientError(Exception):
    """Base exception for Salesforce API errors."""

    pass


class AuthenticationError(SalesforceClientError):
    """Invalid or expired token (401)."""

    pass


class ForbiddenError(SalesforceClientError):
    """Insufficient permissions (403)."""

    pass


class NotFoundError(SalesforceClientError):
    """Record not found (404)."""

    pass


def with_retry(
    max_retries: int = 3,
    base_delay: float = 1.0,
) -> Callable[[Callable[..., Awaitable[T]]], Callable[..., Awaitable[T]]]:
    """Retry Salesforce API calls with exponential backoff.

    - 401: re-raised as AuthenticationError (non-retryable)
    - 403: re-raised as ForbiddenError (non-retryable)
    - 404: re-raised as NotFoundError (non-retryable)
    - 429: wait then retry (unbounded)
    - 5xx: exponential backoff, bounded by max_retries
    """

    def decorator(func: Callable[..., Awaitable[T]]) -> Callable[..., Awaitable[T]]:
        @wraps(func)
        async def wrapper(*args: object, **kwargs: object) -> T:
            last_exception: SalesforceError | None = None
            error_retries = 0

            while True:
                try:
                    return await func(*args, **kwargs)
                except SalesforceAuthenticationFailed as e:
                    raise AuthenticationError("Invalid or expired access token") from e
                except SalesforceError as e:
                    last_exception = e
                    status = getattr(e, "status", 0)

                    if status == 401:
                        raise AuthenticationError("Invalid or expired access token") from e
                    if status == 403:
                        raise ForbiddenError(f"Insufficient permissions: {e}") from e
                    if status == 404:
                        raise NotFoundError(str(e)) from e
                    if status == 429:
                        retry_after = 10
                        logger.warning("Rate limited. Waiting %ds", retry_after)
                        await asyncio.sleep(retry_after)
                        continue
                    if status >= 500:
                        error_retries += 1
                        if error_retries > max_retries:
                            break
                        delay = base_delay * (2 ** (error_retries - 1))
                        logger.warning(
                            "Server error %d. Retrying in %.1fs (attempt %d/%d)",
                            status,
                            delay,
                            error_retries,
                            max_retries,
                        )
                        await asyncio.sleep(delay)
                        continue

                    raise SalesforceClientError(f"API error: {e}") from e

            raise SalesforceClientError(
                f"Max retries exceeded: {last_exception}"
            ) from last_exception

        return wrapper

    return decorator


def _as_str(value: object, field: str) -> str:
    if not isinstance(value, str):
        raise SalesforceClientError(
            f"malformed API response: {field} expected string, got {type(value).__name__}"
        )
    return value


def _as_int(value: object, field: str) -> int:
    if isinstance(value, bool) or not isinstance(value, (int, float)):
        raise SalesforceClientError(
            f"malformed API response: {field} expected number, got {type(value).__name__}"
        )
    return int(value)


def _as_bool(value: object, field: str) -> bool:
    if not isinstance(value, bool):
        raise SalesforceClientError(
            f"malformed API response: {field} expected boolean, got {type(value).__name__}"
        )
    return value


@dataclass(frozen=True)
class QueryResult:
    """Typed envelope of a SOQL query response."""

    done: bool
    total_size: int
    records: tuple[Mapping[str, object], ...]
    next_records_url: str | None

    @classmethod
    def from_response(cls, raw: Mapping[str, object]) -> QueryResult:
        records_value = raw.get("records")
        if not isinstance(records_value, list):
            raise SalesforceClientError(
                "malformed query response: records expected list, "
                f"got {type(records_value).__name__}"
            )
        records: list[Mapping[str, object]] = []
        for record in records_value:
            if not isinstance(record, Mapping):
                raise SalesforceClientError(
                    f"malformed query response: record expected object, got {type(record).__name__}"
                )
            records.append(record)
        next_url_value = raw.get("nextRecordsUrl")
        next_url = _as_str(next_url_value, "nextRecordsUrl") if next_url_value is not None else None
        return cls(
            done=_as_bool(raw.get("done"), "done"),
            total_size=_as_int(raw.get("totalSize"), "totalSize"),
            records=tuple(records),
            next_records_url=next_url,
        )


@dataclass(frozen=True)
class UpdatedResult:
    """Typed envelope of the /updated endpoint response."""

    ids: tuple[str, ...]
    latest_date_covered: datetime | None

    @classmethod
    def from_response(cls, raw: Mapping[str, object]) -> UpdatedResult:
        ids_value = raw.get("ids")
        if not isinstance(ids_value, list):
            raise SalesforceClientError(
                f"malformed updated response: ids expected list, got {type(ids_value).__name__}"
            )
        ids = tuple(_as_str(item, "ids[]") for item in ids_value)
        latest = raw.get("latestDateCovered")
        return cls(
            ids=ids,
            latest_date_covered=_parse_iso(latest) if latest is not None else None,
        )


@dataclass(frozen=True)
class DeletedRecord:
    id: str
    deleted_date: datetime | None


@dataclass(frozen=True)
class DeletedResult:
    """Typed envelope of the /deleted endpoint response."""

    deleted_records: tuple[DeletedRecord, ...]
    earliest_date_available: datetime | None
    latest_date_covered: datetime | None
    next_records_url: str | None

    @classmethod
    def from_response(cls, raw: Mapping[str, object]) -> DeletedResult:
        records_value = raw.get("deletedRecords")
        if not isinstance(records_value, list):
            raise SalesforceClientError(
                "malformed deleted response: deletedRecords expected list, "
                f"got {type(records_value).__name__}"
            )
        records: list[DeletedRecord] = []
        for item in records_value:
            if not isinstance(item, Mapping):
                raise SalesforceClientError(
                    f"malformed deleted response: entry expected object, got {type(item).__name__}"
                )
            deleted_date_value = item.get("deletedDate")
            records.append(
                DeletedRecord(
                    id=_as_str(item.get("id"), "id"),
                    deleted_date=(
                        _parse_iso(deleted_date_value) if deleted_date_value is not None else None
                    ),
                )
            )
        next_url_value = raw.get("nextRecordsUrl")
        earliest = raw.get("earliestDateAvailable")
        latest = raw.get("latestDateCovered")
        return cls(
            deleted_records=tuple(records),
            earliest_date_available=(_parse_iso(earliest) if earliest is not None else None),
            latest_date_covered=_parse_iso(latest) if latest is not None else None,
            next_records_url=_as_str(next_url_value, "nextRecordsUrl")
            if next_url_value is not None
            else None,
        )


def _parse_iso(value: object) -> datetime:
    if not isinstance(value, str):
        raise SalesforceClientError(
            f"malformed timestamp in API response: expected string, got {type(value).__name__}"
        )
    try:
        return datetime.fromisoformat(value.replace("Z", "+00:00"))
    except ValueError as e:
        raise SalesforceClientError(f"malformed timestamp in API response: {value!r}") from e


class SalesforceClient:
    """Async wrapper around simple-salesforce with typed responses."""

    def __init__(self, access_token: str, instance_url: str):
        version = API_VERSION.lstrip("v")
        self._sf = Salesforce(
            instance_url=instance_url,
            session_id=access_token,
            version=version,
        )
        # simple-salesforce always builds an https base_url; honor the exact
        # instance_url the operator provided (https in production, but also
        # http for self-hosted/dev instances and test mocks).
        self._sf.base_url = f"{instance_url.rstrip('/')}/services/data/v{version}/"
        self._instance_url = instance_url

    @property
    def instance_url(self) -> str:
        return self._instance_url

    @with_retry(max_retries=3)
    async def query(self, soql: str) -> QueryResult:
        """Execute a SOQL query and return a typed result."""
        raw = await asyncio.to_thread(self._sf.query, soql)
        return QueryResult.from_response(_require_mapping(raw, "query"))

    @with_retry(max_retries=3)
    async def query_more(self, next_records_url: str) -> QueryResult:
        """Fetch the next page of a query result."""
        raw = await asyncio.to_thread(self._sf.query_more, next_records_url, identifier_is_url=True)
        return QueryResult.from_response(_require_mapping(raw, "query page"))

    @with_retry(max_retries=3)
    async def get_updated(self, object_type: str, start: datetime, end: datetime) -> UpdatedResult:
        """List ids of records updated within [start, end]."""
        raw = await asyncio.to_thread(
            self._sf.restful,
            f"sobjects/{object_type}/updated",
            params={
                "start": _format_api_datetime(start),
                "end": _format_api_datetime(end),
            },
        )
        return UpdatedResult.from_response(_require_mapping(raw, "updated"))

    @with_retry(max_retries=3)
    async def get_deleted(self, object_type: str, start: datetime, end: datetime) -> DeletedResult:
        """List records deleted within [start, end]."""
        raw = await asyncio.to_thread(
            self._sf.restful,
            f"sobjects/{object_type}/deleted",
            params={
                "start": _format_api_datetime(start),
                "end": _format_api_datetime(end),
            },
        )
        return DeletedResult.from_response(_require_mapping(raw, "deleted"))

    @with_retry(max_retries=3)
    async def get_deleted_more(self, next_records_url: str) -> DeletedResult:
        """Fetch the next page of a deleted-records result."""
        raw = await asyncio.to_thread(self._sf.restful, next_records_url.lstrip("/"))
        return DeletedResult.from_response(_require_mapping(raw, "deleted page"))

    @with_retry(max_retries=3)
    async def create(self, object_type: str, data: Mapping[str, object]) -> str:
        """Create a record and return its id."""
        raw = await asyncio.to_thread(
            self._sf.restful,
            f"sobjects/{object_type}/",
            method="POST",
            data=json.dumps({k: v for k, v in data.items() if v is not None}),
        )
        record_id = raw.get("id") if isinstance(raw, Mapping) else None
        if not isinstance(record_id, str):
            raise SalesforceClientError(f"malformed create response for {object_type}: missing id")
        return record_id

    @with_retry(max_retries=3)
    async def update(self, object_type: str, record_id: str, data: Mapping[str, object]) -> None:
        """Update a record in place."""
        await asyncio.to_thread(
            self._sf.restful,
            f"sobjects/{object_type}/{record_id}",
            method="PATCH",
            data=json.dumps({k: v for k, v in data.items() if v is not None}),
        )

    @with_retry(max_retries=3)
    async def get_record(
        self, object_type: str, record_id: str, fields: tuple[str, ...]
    ) -> Mapping[str, object]:
        """Fetch a single record by id with the given fields."""
        raw = await asyncio.to_thread(
            self._sf.restful,
            f"sobjects/{object_type}/{record_id}",
            params={"fields": ",".join(fields)},
        )
        if not isinstance(raw, Mapping):
            raise SalesforceClientError(f"malformed record response for {object_type} {record_id}")
        return raw

    async def test_connection(self) -> None:
        """Verify the token works by querying a single Account."""
        await self.query("SELECT Id FROM Account LIMIT 1")


def _format_api_datetime(value: datetime) -> str:
    return value.strftime("%Y-%m-%dT%H:%M:%SZ")


def _require_mapping(raw: object, what: str) -> Mapping[str, object]:
    if not isinstance(raw, Mapping):
        raise SalesforceClientError(f"malformed {what} response: expected object")
    return raw

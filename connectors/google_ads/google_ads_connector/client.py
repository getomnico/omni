"""Google Ads API client wrapper."""

from __future__ import annotations

import asyncio
import logging
from collections.abc import AsyncIterator
from typing import Any, cast

from .config import GoogleAdsCredentials

logger = logging.getLogger(__name__)


class GoogleAdsConnectorError(Exception):
    """Base connector error."""


class GoogleAdsAuthenticationError(GoogleAdsConnectorError):
    """Authentication/authorization failed."""


class GoogleAdsRateLimitError(GoogleAdsConnectorError):
    """Rate limit or quota failure."""


class GoogleAdsApiError(GoogleAdsConnectorError):
    """Normalized Google Ads API error."""


def _proto_to_dict(value: Any) -> dict[str, Any]:
    if isinstance(value, dict):
        return value
    if hasattr(value, "to_dict") and callable(value.to_dict):
        maybe = value.to_dict()
        if isinstance(maybe, dict):
            return maybe
    try:
        from google.protobuf.json_format import MessageToDict  # type: ignore[import-untyped]

        pb = getattr(value, "_pb", value)
        result = MessageToDict(
            pb, preserving_proto_field_name=True, use_integers_for_enums=False
        )
        return cast(dict[str, Any], result)
    except Exception:
        pass
    return _object_to_dict(value)


def _object_to_dict(value: Any) -> dict[str, Any]:
    if isinstance(value, dict):
        return value
    out: dict[str, Any] = {}
    for name in dir(value):
        if name.startswith("_"):
            continue
        try:
            attr = getattr(value, name)
        except Exception:
            continue
        if callable(attr):
            continue
        if isinstance(attr, (str, int, float, bool)) or attr is None:
            out[name] = attr
    return out


def _normalize_exception(exc: Exception) -> GoogleAdsConnectorError:
    name = exc.__class__.__name__.lower()
    message = str(exc)
    if "authentication" in name or "permission" in name or "unauth" in message.lower():
        return GoogleAdsAuthenticationError(message)
    if (
        "quota" in message.lower()
        or "rate" in message.lower()
        or "resource_exhausted" in message.lower()
    ):
        return GoogleAdsRateLimitError(message)
    return GoogleAdsApiError(message)


class GoogleAdsClient:
    """Small async wrapper around Google's official synchronous Python client."""

    def __init__(
        self, credentials: GoogleAdsCredentials, login_customer_id: str | None = None
    ):
        self.credentials = credentials
        self.login_customer_id = login_customer_id
        self._client: Any | None = None

    @property
    def client(self) -> Any:
        if self._client is None:
            try:
                from google.ads.googleads.client import (  # type: ignore[import-untyped]
                    GoogleAdsClient as OfficialGoogleAdsClient,
                )

                self._client = OfficialGoogleAdsClient.load_from_dict(
                    self.credentials.to_google_ads_dict(self.login_customer_id)
                )
            except Exception as exc:  # pragma: no cover - exercised in integration env
                raise _normalize_exception(exc) from exc
        return self._client

    async def list_accessible_customers(self) -> list[str]:
        def call() -> list[str]:
            try:
                customer_service = self.client.get_service("CustomerService")
                response = customer_service.list_accessible_customers()
                return [str(r).split("/")[-1] for r in response.resource_names]
            except Exception as exc:
                raise _normalize_exception(exc) from exc

        return await asyncio.to_thread(call)

    async def search(
        self,
        customer_id: str,
        query: str,
        *,
        page_size: int = 1000,
    ) -> AsyncIterator[dict[str, Any]]:
        rows = await asyncio.to_thread(self._search_sync, customer_id, query, page_size)
        for row in rows:
            yield row

    def _search_sync(
        self, customer_id: str, query: str, page_size: int
    ) -> list[dict[str, Any]]:
        try:
            service = self.client.get_service("GoogleAdsService")
            request = self.client.get_type("SearchGoogleAdsRequest")
            request.customer_id = customer_id
            request.query = query
            request.page_size = page_size
            response = service.search(request=request)
            return [_proto_to_dict(row) for row in response]
        except Exception as exc:
            raise _normalize_exception(exc) from exc

    async def search_stream(
        self, customer_id: str, query: str
    ) -> AsyncIterator[dict[str, Any]]:
        batches = await asyncio.to_thread(self._search_stream_sync, customer_id, query)
        for row in batches:
            yield row

    def _search_stream_sync(self, customer_id: str, query: str) -> list[dict[str, Any]]:
        try:
            service = self.client.get_service("GoogleAdsService")
            response = service.search_stream(customer_id=customer_id, query=query)
            rows: list[dict[str, Any]] = []
            for batch in response:
                for row in batch.results:
                    rows.append(_proto_to_dict(row))
            return rows
        except Exception as exc:
            raise _normalize_exception(exc) from exc

    async def run_gaql(
        self, customer_id: str, query: str, *, limit: int = 1000
    ) -> list[dict[str, Any]]:
        rows: list[dict[str, Any]] = []
        async for row in self.search(
            customer_id, query, page_size=min(max(limit, 1), 10000)
        ):
            rows.append(row)
            if len(rows) >= limit:
                break
        return rows


class InMemoryGoogleAdsClient(GoogleAdsClient):
    """Test/mock client using configured query results."""

    def __init__(self, data: dict[str, Any]):
        self.data = data

    async def list_accessible_customers(self) -> list[str]:
        return [str(c) for c in self.data.get("accessible_customers", [])]

    async def search(
        self, customer_id: str, query: str, *, page_size: int = 1000
    ) -> AsyncIterator[dict[str, Any]]:
        rows = self._rows_for_query(customer_id, query)
        for row in rows:
            yield row

    async def search_stream(
        self, customer_id: str, query: str
    ) -> AsyncIterator[dict[str, Any]]:
        rows = self._rows_for_query(customer_id, query)
        for row in rows:
            yield row

    async def run_gaql(
        self, customer_id: str, query: str, *, limit: int = 1000
    ) -> list[dict[str, Any]]:
        return self._rows_for_query(customer_id, query)[:limit]

    def _rows_for_query(self, customer_id: str, query: str) -> list[dict[str, Any]]:
        by_customer = self.data.get("customers", {}).get(str(customer_id), {})
        normalized = " ".join(query.lower().split())
        for key, rows in by_customer.get("queries", {}).items():
            if key.lower() in normalized:
                return list(rows)
        from_resource = _extract_from_resource(normalized)
        if from_resource and from_resource in by_customer:
            return list(by_customer[from_resource])
        return []


def _extract_from_resource(query: str) -> str | None:
    parts = query.split(" from ", 1)
    if len(parts) != 2:
        return None
    resource = parts[1].split()[0].strip()
    return resource or None

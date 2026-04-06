"""Async HTTP client for the paperless-ngx API."""

import asyncio
import logging
from datetime import datetime, timezone
from collections.abc import AsyncIterator
from typing import Any

import httpx

from .config import INITIAL_BACKOFF_SECONDS, MAX_RETRIES, PAGE_SIZE
from .models import (
    PaperlessCustomField,
    PaperlessDocument,
)

logger = logging.getLogger(__name__)


class PaperlessError(Exception):
    """Base exception for paperless-ngx API errors."""


class AuthenticationError(PaperlessError):
    """Invalid or missing API token (401/403)."""


class PaperlessClient:
    """Thin async wrapper around the paperless-ngx REST API."""

    def __init__(self, base_url: str, api_key: str) -> None:
        # Normalise: strip trailing slash so path concatenation is predictable.
        self._base_url = base_url.rstrip("/")
        self._client = httpx.AsyncClient(
            base_url=self._base_url,
            headers={"Authorization": f"Token {api_key}"},
            timeout=30.0,
        )
        # In-memory caches for tag/correspondent/document-type lookups so we
        # don't re-fetch them for every document during a single sync run.
        self._tags: dict[int, str] | None = None
        self._correspondents: dict[int, str] | None = None
        self._document_types: dict[int, str] | None = None

    # ── Internal helpers ────────────────────────────────────────────

    async def _request(self, method: str, path: str, **kwargs: Any) -> Any:
        """Send a request with retry on rate-limit and transient server errors."""
        backoff = INITIAL_BACKOFF_SECONDS
        for attempt in range(MAX_RETRIES + 1):
            resp = await self._client.request(method, path, **kwargs)

            if resp.status_code == 429:
                try:
                    wait = float(resp.headers.get("Retry-After", backoff))
                except (ValueError, TypeError):
                    wait = backoff
                logger.warning("Rate limited, waiting %.1fs (attempt %d)", wait, attempt + 1)
                await asyncio.sleep(wait)
                backoff *= 2
                continue

            if resp.status_code in (401, 403):
                raise AuthenticationError(
                    f"Authentication failed (HTTP {resp.status_code}): check your API key"
                )

            if resp.status_code >= 500:
                if attempt < MAX_RETRIES:
                    logger.warning(
                        "Server error %d, retrying in %.1fs", resp.status_code, backoff
                    )
                    await asyncio.sleep(backoff)
                    backoff *= 2
                    continue
                raise PaperlessError(
                    f"Server error (HTTP {resp.status_code}) after {MAX_RETRIES + 1} attempts"
                )

            try:
                resp.raise_for_status()
            except httpx.HTTPStatusError as exc:
                raise PaperlessError(str(exc)) from exc
            return resp.json()

        raise PaperlessError("Max retries exceeded")

    async def _iter_pages(
        self, path: str, params: dict[str, Any] | None = None
    ) -> AsyncIterator[dict[str, Any]]:
        """Yield individual items from a paginated endpoint, one page at a time."""
        page = 1
        query = dict(params) if params else {}
        query.setdefault("page_size", PAGE_SIZE)
        while True:
            query["page"] = page
            data = await self._request("GET", path, params=query)
            for item in data.get("results", []):
                yield item
            if not data.get("next"):
                break
            page += 1

    async def _list_all(
        self, path: str, params: dict[str, Any] | None = None
    ) -> list[dict[str, Any]]:
        """Collect all results from a paginated endpoint (for small lookup tables)."""
        return [item async for item in self._iter_pages(path, params)]

    # ── Caching lookups ─────────────────────────────────────────────

    async def get_tags(self) -> dict[int, str]:
        if self._tags is None:
            items = await self._list_all("/api/tags/")
            self._tags = {item["id"]: item["name"] for item in items}
        return self._tags

    async def get_correspondents(self) -> dict[int, str]:
        if self._correspondents is None:
            items = await self._list_all("/api/correspondents/")
            self._correspondents = {item["id"]: item["name"] for item in items}
        return self._correspondents

    async def get_document_types(self) -> dict[int, str]:
        if self._document_types is None:
            items = await self._list_all("/api/document_types/")
            self._document_types = {item["id"]: item["name"] for item in items}
        return self._document_types

    # ── Document listing ────────────────────────────────────────────

    async def list_documents(
        self,
        *,
        modified_after: datetime | None = None,
    ) -> AsyncIterator[dict[str, Any]]:
        """Yield raw document dicts, optionally filtered by modification date.

        Documents are streamed page-by-page so that only one page is held in
        memory at a time — safe for instances with tens of thousands of documents.
        """
        params: dict[str, Any] = {"ordering": "modified"}
        if modified_after is not None:
            params["modified__gt"] = modified_after.isoformat()
        async for item in self._iter_pages("/api/documents/", params):
            yield item

    # ── Document parsing ────────────────────────────────────────────

    async def parse_document(self, raw: dict[str, Any]) -> PaperlessDocument:
        """Convert a raw API dict to a PaperlessDocument with resolved names."""
        tags_map = await self.get_tags()
        correspondents_map = await self.get_correspondents()
        doc_types_map = await self.get_document_types()

        tag_ids: list[int] = raw.get("tags", [])
        correspondent_id: int | None = raw.get("correspondent")
        document_type_id: int | None = raw.get("document_type")

        # Parse custom fields
        custom_fields: list[PaperlessCustomField] = []
        for cf in raw.get("custom_fields", []):
            value = cf.get("value")
            custom_fields.append(
                PaperlessCustomField(
                    name=str(cf.get("field", "")),
                    value=str(value) if value is not None else None,
                )
            )

        return PaperlessDocument(
            id=raw["id"],
            title=raw.get("title", "Untitled"),
            content=raw.get("content", ""),
            created=_parse_dt(raw.get("created")),
            added=_parse_dt(raw.get("added")),
            modified=_parse_dt(raw.get("modified")),
            original_file_name=raw.get("original_file_name"),
            custom_fields=custom_fields,
            correspondent_name=correspondents_map.get(correspondent_id) if correspondent_id else None,
            document_type_name=doc_types_map.get(document_type_id) if document_type_id else None,
            tag_names=[tags_map[t] for t in tag_ids if t in tags_map],
        )

    # ── Lifecycle ───────────────────────────────────────────────────

    async def close(self) -> None:
        await self._client.aclose()

    async def validate(self) -> None:
        """Validate connectivity and credentials. Raises AuthenticationError on failure."""
        await self._request("GET", "/api/")


# ── Helpers ──────────────────────────────────────────────────────────

def _parse_dt(value: str | None) -> datetime | None:
    if not value:
        return None
    # Python 3.11+ fromisoformat handles 'Z', fractional seconds, offsets, and date-only strings.
    try:
        dt = datetime.fromisoformat(value)
        if dt.tzinfo is None:
            dt = dt.replace(tzinfo=timezone.utc)
        return dt
    except ValueError:
        logger.debug("Could not parse datetime: %r", value)
        return None

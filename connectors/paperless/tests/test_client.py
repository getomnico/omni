"""Unit tests for the paperless-ngx API client."""

from datetime import datetime, timezone
from typing import Any
from unittest.mock import AsyncMock, MagicMock, patch

import pytest

from paperless_connector.client import AuthenticationError, PaperlessClient, PaperlessError


def _mock_response(json_data: Any, status_code: int = 200) -> MagicMock:
    resp = MagicMock()
    resp.status_code = status_code
    resp.json.return_value = json_data
    resp.raise_for_status = MagicMock()
    resp.headers = {}
    return resp


def _paginated(results: list[Any]) -> dict[str, Any]:
    return {"count": len(results), "next": None, "previous": None, "results": results}


@pytest.fixture
def client() -> PaperlessClient:
    return PaperlessClient(base_url="http://paperless.local", api_key="test-token-abc")


class TestPaperlessClientAuth:
    async def test_request_raises_on_401(self, client: PaperlessClient) -> None:
        with patch.object(client._client, "request", new_callable=AsyncMock) as mock_req:
            mock_req.return_value = _mock_response({}, status_code=401)
            with pytest.raises(AuthenticationError):
                await client._request("GET", "/api/")

    async def test_request_raises_on_403(self, client: PaperlessClient) -> None:
        with patch.object(client._client, "request", new_callable=AsyncMock) as mock_req:
            mock_req.return_value = _mock_response({}, status_code=403)
            with pytest.raises(AuthenticationError):
                await client._request("GET", "/api/")

    async def test_validate_success(self, client: PaperlessClient) -> None:
        with patch.object(client._client, "request", new_callable=AsyncMock) as mock_req:
            mock_req.return_value = _mock_response({"version": "2.0"})
            await client.validate()  # Should not raise

    async def test_validate_failure(self, client: PaperlessClient) -> None:
        with patch.object(client._client, "request", new_callable=AsyncMock) as mock_req:
            mock_req.return_value = _mock_response({}, status_code=401)
            with pytest.raises(AuthenticationError):
                await client.validate()


class TestPaperlessClientCaching:
    async def test_tags_cached_after_first_call(self, client: PaperlessClient) -> None:
        tags_response = _paginated([{"id": 1, "name": "finance"}, {"id": 2, "name": "2024"}])
        with patch.object(client._client, "request", new_callable=AsyncMock) as mock_req:
            mock_req.return_value = _mock_response(tags_response)
            tags1 = await client.get_tags()
            tags2 = await client.get_tags()
        assert tags1 == tags2 == {1: "finance", 2: "2024"}
        # Only one actual HTTP call
        assert mock_req.call_count == 1

    async def test_correspondents_cached(self, client: PaperlessClient) -> None:
        corr_response = _paginated([{"id": 5, "name": "ACME Corp"}])
        with patch.object(client._client, "request", new_callable=AsyncMock) as mock_req:
            mock_req.return_value = _mock_response(corr_response)
            c1 = await client.get_correspondents()
            c2 = await client.get_correspondents()
        assert c1 == {5: "ACME Corp"}
        assert c1 == c2
        assert mock_req.call_count == 1

    async def test_document_types_cached(self, client: PaperlessClient) -> None:
        dt_response = _paginated([{"id": 3, "name": "Invoice"}])
        with patch.object(client._client, "request", new_callable=AsyncMock) as mock_req:
            mock_req.return_value = _mock_response(dt_response)
            dt1 = await client.get_document_types()
            dt2 = await client.get_document_types()
        assert dt1 == {3: "Invoice"}
        assert dt1 == dt2
        assert mock_req.call_count == 1


class TestListDocuments:
    async def test_returns_empty_list_when_no_documents(self, client: PaperlessClient) -> None:
        with patch.object(client._client, "request", new_callable=AsyncMock) as mock_req:
            mock_req.return_value = _mock_response(_paginated([]))
            result = [doc async for doc in client.list_documents()]
        assert result == []

    async def test_passes_modified_after_param(self, client: PaperlessClient) -> None:
        dt = datetime(2024, 6, 1, tzinfo=timezone.utc)
        with patch.object(client._client, "request", new_callable=AsyncMock) as mock_req:
            mock_req.return_value = _mock_response(_paginated([]))
            _ = [doc async for doc in client.list_documents(modified_after=dt)]
        call_kwargs = mock_req.call_args
        params = call_kwargs.kwargs.get("params", {})
        assert "modified__gt" in params
        # Should be full ISO 8601, not date-only
        assert "T" in params["modified__gt"]
        # Should contain proper colon separator in timezone offset (e.g. +00:00)
        assert params["modified__gt"] == "2024-06-01T00:00:00+00:00"

    async def test_paginates_multiple_pages(self, client: PaperlessClient) -> None:
        page1 = {"count": 2, "next": "?page=2", "results": [{"id": 1}, {"id": 2}]}
        page2 = {"count": 2, "next": None, "results": [{"id": 3}]}

        responses = [_mock_response(page1), _mock_response(page2)]
        with patch.object(
            client._client, "request", new_callable=AsyncMock, side_effect=responses
        ):
            result = [doc async for doc in client.list_documents()]
        assert len(result) == 3
        assert [r["id"] for r in result] == [1, 2, 3]


class TestParseDocument:
    async def _setup_caches(self, client: PaperlessClient) -> None:
        """Pre-populate caches so parse_document doesn't make network calls."""
        client._tags = {1: "finance", 2: "2024"}
        client._correspondents = {10: "ACME Corp"}
        client._document_types = {20: "Invoice"}

    async def test_basic_fields(self, client: PaperlessClient) -> None:
        await self._setup_caches(client)
        raw = {
            "id": 42,
            "title": "Invoice Jan 2024",
            "content": "Total: 1000 EUR",
            "created": "2024-01-15T10:00:00Z",
            "added": "2024-01-16T08:00:00Z",
            "modified": "2024-01-17T09:00:00Z",
            "original_file_name": "invoice.pdf",
            "archived_file_name": "invoice.pdf",
            "correspondent": 10,
            "document_type": 20,
            "tags": [1, 2],
            "custom_fields": [],
        }
        doc = await client.parse_document(raw)

        assert doc.id == 42
        assert doc.title == "Invoice Jan 2024"
        assert doc.content == "Total: 1000 EUR"
        assert doc.correspondent_name == "ACME Corp"
        assert doc.document_type_name == "Invoice"
        assert sorted(doc.tag_names) == ["2024", "finance"]
        assert doc.original_file_name == "invoice.pdf"

    async def test_missing_correspondent_resolves_to_none(self, client: PaperlessClient) -> None:
        await self._setup_caches(client)
        raw = {
            "id": 1,
            "title": "Doc",
            "content": "",
            "created": None,
            "added": None,
            "modified": None,
            "original_file_name": None,
            "archived_file_name": None,
            "correspondent": None,
            "document_type": None,
            "tags": [],
            "custom_fields": [],
        }
        doc = await client.parse_document(raw)
        assert doc.correspondent_name is None
        assert doc.document_type_name is None
        assert doc.tag_names == []

    async def test_custom_fields_parsed(self, client: PaperlessClient) -> None:
        await self._setup_caches(client)
        raw = {
            "id": 5,
            "title": "Doc",
            "content": "",
            "created": None,
            "added": None,
            "modified": None,
            "original_file_name": None,
            "archived_file_name": None,
            "correspondent": None,
            "document_type": None,
            "tags": [],
            "custom_fields": [
                {"field": 99, "value": "INV-001"},
            ],
        }
        doc = await client.parse_document(raw)
        assert len(doc.custom_fields) == 1
        assert doc.custom_fields[0].value == "INV-001"


class TestRetryBehavior:
    async def test_raises_on_server_error_after_retries(self, client: PaperlessClient) -> None:
        resp = _mock_response({}, status_code=500)
        resp.raise_for_status.side_effect = Exception("Internal Server Error")
        with patch.object(
            client._client, "request", new_callable=AsyncMock, return_value=resp
        ):
            with patch("asyncio.sleep", new_callable=AsyncMock):
                with pytest.raises(Exception):
                    await client._request("GET", "/api/documents/")

    async def test_raises_paperless_error_on_max_retries(self, client: PaperlessClient) -> None:
        """After MAX_RETRIES rate-limit responses, PaperlessError is raised."""
        rate_limited = _mock_response({}, status_code=429)
        rate_limited.headers = {"Retry-After": "0.01"}
        with patch.object(
            client._client, "request", new_callable=AsyncMock, return_value=rate_limited
        ):
            with patch("asyncio.sleep", new_callable=AsyncMock):
                with pytest.raises(PaperlessError, match="Max retries exceeded"):
                    await client._request("GET", "/api/documents/")


class TestParseDt:
    """Tests for the _parse_dt helper function."""

    def test_iso_with_timezone(self) -> None:
        from paperless_connector.client import _parse_dt

        result = _parse_dt("2024-01-15T10:30:00+00:00")
        assert result is not None
        assert result.year == 2024
        assert result.month == 1
        assert result.day == 15
        assert result.hour == 10
        assert result.minute == 30
        assert result.tzinfo is not None

    def test_iso_with_z_suffix(self) -> None:
        from paperless_connector.client import _parse_dt

        result = _parse_dt("2024-01-15T10:30:00Z")
        assert result is not None
        assert result.tzinfo is not None

    def test_iso_with_fractional_seconds(self) -> None:
        from paperless_connector.client import _parse_dt

        result = _parse_dt("2024-01-15T10:30:00.123456+00:00")
        assert result is not None
        assert result.year == 2024
        assert result.microsecond == 123456

    def test_iso_with_fractional_seconds_z(self) -> None:
        from paperless_connector.client import _parse_dt

        result = _parse_dt("2024-06-15T14:23:45.999999Z")
        assert result is not None
        assert result.tzinfo is not None

    def test_date_only(self) -> None:
        from paperless_connector.client import _parse_dt

        result = _parse_dt("2024-01-15")
        assert result is not None
        assert result.year == 2024
        assert result.tzinfo is not None

    def test_naive_datetime_gets_utc(self) -> None:
        from paperless_connector.client import _parse_dt

        result = _parse_dt("2024-01-15T10:30:00")
        assert result is not None
        assert result.tzinfo is not None

    def test_none_returns_none(self) -> None:
        from paperless_connector.client import _parse_dt

        assert _parse_dt(None) is None

    def test_empty_string_returns_none(self) -> None:
        from paperless_connector.client import _parse_dt

        assert _parse_dt("") is None

    def test_garbage_returns_none(self) -> None:
        from paperless_connector.client import _parse_dt

        assert _parse_dt("not-a-date") is None

from __future__ import annotations

from unittest.mock import AsyncMock

import pytest

from tools.registry import ToolContext
from tools.web_handler import WebAccessPolicy, WebToolHandler, validate_public_web_url
from web_providers import FetchedWebPage, WebSearchResponse, WebSearchResult
from web_providers.base import WebProviderError

pytestmark = pytest.mark.unit


class FakeSearchProvider:
    provider_type = "fake"

    async def search(self, request):
        return WebSearchResponse(
            query=request.query,
            provider="fake",
            results=[
                WebSearchResult(
                    title="Example result",
                    url="https://example.com/result",
                    snippet="Relevant public snippet",
                )
            ],
        )

    async def health_check(self):
        return True


class FailingSearchProvider:
    provider_type = "fake"

    async def search(self, request):
        raise WebProviderError("search failed")

    async def health_check(self):
        return False


class FakeFetchProvider:
    provider_type = "fake"

    async def fetch(self, url: str):
        return FetchedWebPage(url=url, title="Fetched", content="Page body")

    async def health_check(self):
        return True


def _context() -> ToolContext:
    return ToolContext(chat_id="chat", user_id="user")


def test_web_tool_schema_exposes_fetch_only_when_configured():
    search_only = WebToolHandler(search_provider=FakeSearchProvider())
    assert [tool["name"] for tool in search_only.get_tools()] == ["web_search"]

    with_fetch = WebToolHandler(
        search_provider=FakeSearchProvider(), fetch_provider=FakeFetchProvider()
    )
    assert [tool["name"] for tool in with_fetch.get_tools()] == [
        "web_search",
        "fetch_web_page",
    ]


@pytest.mark.asyncio
async def test_web_search_returns_citation_friendly_result_block():
    handler = WebToolHandler(search_provider=FakeSearchProvider())

    result = await handler.execute("web_search", {"query": "docs", "limit": 1}, _context())

    assert result.is_error is False
    assert result.content[0]["type"] == "search_result"
    assert result.content[0]["source"] == "https://example.com/result"


@pytest.mark.asyncio
async def test_web_search_provider_failure_is_clear_error():
    handler = WebToolHandler(search_provider=FailingSearchProvider())

    result = await handler.execute("web_search", {"query": "docs"}, _context())

    assert result.is_error is True
    assert result.content[0]["text"] == "search failed"


@pytest.mark.asyncio
async def test_fetch_web_page_wraps_untrusted_content(monkeypatch):
    monkeypatch.setattr("tools.web_handler.load_web_access_policy", AsyncMock(return_value=WebAccessPolicy([])))
    monkeypatch.setattr("tools.web_handler._resolve_host", AsyncMock(return_value=["93.184.216.34"]))
    handler = WebToolHandler(
        search_provider=FakeSearchProvider(), fetch_provider=FakeFetchProvider()
    )

    result = await handler.execute("fetch_web_page", {"url": "https://example.com"}, _context())

    assert result.is_error is False
    text = result.content[0]["text"]
    assert "<untrusted-web-page" in text
    assert "Page body" in text


@pytest.mark.asyncio
async def test_fetch_web_page_rejects_private_urls(monkeypatch):
    monkeypatch.setattr("tools.web_handler.load_web_access_policy", AsyncMock(return_value=WebAccessPolicy([])))
    handler = WebToolHandler(
        search_provider=FakeSearchProvider(), fetch_provider=FakeFetchProvider()
    )

    result = await handler.execute("fetch_web_page", {"url": "http://127.0.0.1/admin"}, _context())

    assert result.is_error is True
    assert "Private/internal" in result.content[0]["text"]


@pytest.mark.asyncio
async def test_validate_public_web_url_enforces_blocklist(monkeypatch):
    monkeypatch.setattr("tools.web_handler._resolve_host", AsyncMock(return_value=["93.184.216.34"]))

    with pytest.raises(Exception, match="blocked by admin policy"):
        await validate_public_web_url(
            "https://docs.example.com/private",
            WebAccessPolicy(blocklist=["*.example.com"]),
        )

from __future__ import annotations

import json

import pytest
import respx
from httpx import Response

from web_providers import WebProviderError, WebSearchRequest, create_web_search_provider
from web_providers.brave import BraveSearchProvider
from web_providers.exa import ExaFetchProvider, ExaSearchProvider
from web_providers.firecrawl import FirecrawlFetchProvider
from web_providers.keenable import KeenableSearchProvider
from web_providers.serper import SerperSearchProvider

pytestmark = pytest.mark.unit


@pytest.mark.asyncio
@respx.mock
async def test_serper_search_success():
    respx.post("https://google.serper.dev/search").mock(
        return_value=Response(
            200,
            json={
                "organic": [
                    {
                        "title": "Result",
                        "link": "https://example.com",
                        "snippet": "Snippet",
                        "source": "Example",
                    }
                ]
            },
        )
    )
    provider = SerperSearchProvider(api_key="key")

    response = await provider.search(WebSearchRequest(query="query", limit=1))

    assert response.results[0].url == "https://example.com"


@pytest.mark.asyncio
@respx.mock
async def test_brave_search_success():
    respx.get("https://api.search.brave.com/res/v1/web/search").mock(
        return_value=Response(
            200,
            json={
                "web": {
                    "results": [
                        {
                            "title": "Result",
                            "url": "https://example.com",
                            "description": "Snippet",
                        }
                    ]
                }
            },
        )
    )
    provider = BraveSearchProvider(api_key="key")

    response = await provider.search(WebSearchRequest(query="query", limit=1))

    assert response.results[0].title == "Result"


@pytest.mark.asyncio
@respx.mock
async def test_exa_search_success():
    respx.post("https://api.exa.ai/search").mock(
        return_value=Response(
            200,
            json={"results": [{"title": "Result", "url": "https://example.com", "text": "Text"}]},
        )
    )
    provider = ExaSearchProvider(api_key="key")

    response = await provider.search(WebSearchRequest(query="query", limit=1))

    assert response.results[0].snippet == "Text"


@pytest.mark.asyncio
@respx.mock
async def test_firecrawl_fetch_success():
    respx.post("https://api.firecrawl.dev/v1/scrape").mock(
        return_value=Response(
            200,
            json={
                "success": True,
                "data": {
                    "markdown": "# Page",
                    "metadata": {"title": "Page", "sourceURL": "https://example.com"},
                },
            },
        )
    )
    provider = FirecrawlFetchProvider(api_key="key")

    page = await provider.fetch("https://example.com")

    assert page.title == "Page"
    assert page.content == "# Page"


@pytest.mark.asyncio
@respx.mock
async def test_exa_fetch_success():
    respx.post("https://api.exa.ai/contents").mock(
        return_value=Response(
            200,
            json={"results": [{"url": "https://example.com", "title": "Page", "text": "Body"}]},
        )
    )
    provider = ExaFetchProvider(api_key="key")

    page = await provider.fetch("https://example.com")

    assert page.content == "Body"


@pytest.mark.asyncio
@respx.mock
async def test_keenable_search_success():
    route = respx.post("https://api.keenable.ai/v1/search").mock(
        return_value=Response(
            200,
            json={
                "results": [
                    {
                        "title": "Result",
                        "url": "https://example.com",
                        "description": "",
                        "snippet": "x" * 800,
                    },
                    {"title": "", "url": "https://example.org"},
                    {"title": "Second", "url": "https://example.net", "description": "Desc"},
                ]
            },
        )
    )
    provider = KeenableSearchProvider(api_key="key")

    response = await provider.search(WebSearchRequest(query="query", limit=3))

    request = route.calls.last.request
    assert request.headers["X-API-Key"] == "key"
    assert json.loads(request.content) == {"query": "query", "max_results": 3}
    assert [r.url for r in response.results] == ["https://example.com", "https://example.net"]
    assert len(response.results[0].snippet) == 500
    assert response.results[1].snippet == "Desc"
    assert response.provider == "keenable"


@pytest.mark.asyncio
@respx.mock
async def test_keenable_search_http_error():
    respx.post("https://api.keenable.ai/v1/search").mock(
        return_value=Response(401, json={"error": "bad key"})
    )
    provider = KeenableSearchProvider(api_key="key")

    with pytest.raises(WebProviderError) as exc:
        await provider.search(WebSearchRequest(query="query", limit=1))

    assert exc.value.status_code == 401


def test_factory_creates_keenable_and_requires_key():
    assert isinstance(create_web_search_provider("keenable", api_key="key"), KeenableSearchProvider)
    with pytest.raises(ValueError):
        create_web_search_provider("keenable")

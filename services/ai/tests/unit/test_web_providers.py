from __future__ import annotations

import pytest
import respx
from httpx import Response

from web_providers import WebSearchRequest
from web_providers.brave import BraveSearchProvider
from web_providers.exa import ExaFetchProvider, ExaSearchProvider
from web_providers.firecrawl import FirecrawlFetchProvider
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

from __future__ import annotations

import httpx

from .base import (
    FetchedWebPage,
    WebFetchProvider,
    WebProviderError,
    WebSearchProvider,
    WebSearchRequest,
    WebSearchResponse,
    WebSearchResult,
)


class ExaSearchProvider(WebSearchProvider):
    provider_type = "exa"

    def __init__(self, api_key: str, base_url: str | None = None) -> None:
        self.api_key = api_key
        self.base_url = (base_url or "https://api.exa.ai").rstrip("/")
        self.client = httpx.AsyncClient(timeout=httpx.Timeout(20.0, connect=5.0))

    async def search(self, request: WebSearchRequest) -> WebSearchResponse:
        try:
            response = await self.client.post(
                f"{self.base_url}/search",
                headers={"x-api-key": self.api_key, "Content-Type": "application/json"},
                json={"query": request.query, "numResults": request.limit},
            )
            response.raise_for_status()
            data = response.json()
        except httpx.HTTPStatusError as e:
            raise WebProviderError(
                f"Exa search failed: HTTP {e.response.status_code}",
                status_code=e.response.status_code,
            ) from e
        except httpx.RequestError as e:
            raise WebProviderError(f"Exa search failed: {e}") from e

        results: list[WebSearchResult] = []
        for item in data.get("results", [])[: request.limit]:
            url = item.get("url")
            title = item.get("title") or url
            if not url or not title:
                continue
            highlights = item.get("highlights")
            snippet = item.get("text") or ("\n".join(highlights) if isinstance(highlights, list) else None)
            results.append(
                WebSearchResult(
                    title=title,
                    url=url,
                    snippet=snippet,
                    published_date=item.get("publishedDate"),
                    source=item.get("author"),
                    metadata={"score": item.get("score")},
                )
            )
        return WebSearchResponse(query=request.query, results=results, provider=self.provider_type)

    async def health_check(self) -> bool:
        try:
            response = await self.search(WebSearchRequest(query="connection test", limit=1))
            return bool(response.results)
        except Exception:
            return False


class ExaFetchProvider(WebFetchProvider):
    provider_type = "exa"

    def __init__(self, api_key: str, base_url: str | None = None) -> None:
        self.api_key = api_key
        self.base_url = (base_url or "https://api.exa.ai").rstrip("/")
        self.client = httpx.AsyncClient(timeout=httpx.Timeout(30.0, connect=5.0))

    async def fetch(self, url: str) -> FetchedWebPage:
        try:
            response = await self.client.post(
                f"{self.base_url}/contents",
                headers={"x-api-key": self.api_key, "Content-Type": "application/json"},
                json={"urls": [url], "text": True},
            )
            response.raise_for_status()
            data = response.json()
        except httpx.HTTPStatusError as e:
            raise WebProviderError(
                f"Exa fetch failed: HTTP {e.response.status_code}",
                status_code=e.response.status_code,
            ) from e
        except httpx.RequestError as e:
            raise WebProviderError(f"Exa fetch failed: {e}") from e

        results = data.get("results", [])
        if not results:
            raise WebProviderError("Exa returned no content for URL")
        item = results[0]
        content = item.get("text")
        if not content:
            raise WebProviderError("Exa returned empty content for URL")
        return FetchedWebPage(
            url=item.get("url") or url,
            title=item.get("title"),
            content=content,
            description=item.get("summary"),
            metadata={"author": item.get("author"), "published_date": item.get("publishedDate")},
        )

    async def health_check(self) -> bool:
        return True

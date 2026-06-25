from __future__ import annotations

import httpx

from .base import WebProviderError, WebSearchProvider, WebSearchRequest, WebSearchResponse, WebSearchResult


class SerperSearchProvider(WebSearchProvider):
    provider_type = "serper"

    def __init__(self, api_key: str, base_url: str | None = None) -> None:
        self.api_key = api_key
        self.base_url = (base_url or "https://google.serper.dev").rstrip("/")
        self.client = httpx.AsyncClient(timeout=httpx.Timeout(20.0, connect=5.0))

    async def search(self, request: WebSearchRequest) -> WebSearchResponse:
        try:
            response = await self.client.post(
                f"{self.base_url}/search",
                headers={"X-API-KEY": self.api_key, "Content-Type": "application/json"},
                json={"q": request.query, "num": request.limit},
            )
            response.raise_for_status()
            data = response.json()
        except httpx.HTTPStatusError as e:
            raise WebProviderError(
                f"Serper search failed: HTTP {e.response.status_code}",
                status_code=e.response.status_code,
            ) from e
        except httpx.RequestError as e:
            raise WebProviderError(f"Serper search failed: {e}") from e

        results: list[WebSearchResult] = []
        for item in data.get("organic", [])[: request.limit]:
            url = item.get("link")
            title = item.get("title")
            if not url or not title:
                continue
            results.append(
                WebSearchResult(
                    title=title,
                    url=url,
                    snippet=item.get("snippet"),
                    published_date=item.get("date"),
                    source=item.get("source"),
                )
            )
        return WebSearchResponse(query=request.query, results=results, provider=self.provider_type)

    async def health_check(self) -> bool:
        try:
            response = await self.search(WebSearchRequest(query="connection test", limit=1))
            return bool(response.results)
        except Exception:
            return False

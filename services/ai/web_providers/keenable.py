from __future__ import annotations

import httpx

from .base import WebProviderError, WebSearchProvider, WebSearchRequest, WebSearchResponse, WebSearchResult


class KeenableSearchProvider(WebSearchProvider):
    provider_type = "keenable"

    def __init__(self, api_key: str, base_url: str | None = None) -> None:
        self.api_key = api_key
        self.base_url = (base_url or "https://api.keenable.ai").rstrip("/")
        self.client = httpx.AsyncClient(timeout=httpx.Timeout(20.0, connect=5.0))

    async def search(self, request: WebSearchRequest) -> WebSearchResponse:
        try:
            response = await self.client.post(
                f"{self.base_url}/v1/search",
                headers={"X-API-Key": self.api_key, "Content-Type": "application/json"},
                json={"query": request.query, "max_results": request.limit},
            )
            response.raise_for_status()
            data = response.json()
        except httpx.HTTPStatusError as e:
            raise WebProviderError(
                f"Keenable search failed: HTTP {e.response.status_code}",
                status_code=e.response.status_code,
            ) from e
        except httpx.RequestError as e:
            raise WebProviderError(f"Keenable search failed: {e}") from e

        results: list[WebSearchResult] = []
        for item in (data.get("results") or [])[: request.limit]:
            url = item.get("url")
            title = item.get("title")
            if not url or not title:
                continue
            snippet = item.get("description") or item.get("snippet")
            results.append(
                WebSearchResult(
                    title=title,
                    url=url,
                    snippet=snippet[:500] if snippet else None,
                )
            )
        return WebSearchResponse(query=request.query, results=results, provider=self.provider_type)

    async def health_check(self) -> bool:
        try:
            response = await self.search(WebSearchRequest(query="connection test", limit=1))
            return bool(response.results)
        except Exception:
            return False

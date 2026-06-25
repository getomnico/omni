from __future__ import annotations

import httpx

from .base import WebProviderError, WebSearchProvider, WebSearchRequest, WebSearchResponse, WebSearchResult


class BraveSearchProvider(WebSearchProvider):
    provider_type = "brave"

    def __init__(self, api_key: str, base_url: str | None = None) -> None:
        self.api_key = api_key
        self.base_url = (base_url or "https://api.search.brave.com/res/v1").rstrip("/")
        self.client = httpx.AsyncClient(timeout=httpx.Timeout(20.0, connect=5.0))

    async def search(self, request: WebSearchRequest) -> WebSearchResponse:
        try:
            response = await self.client.get(
                f"{self.base_url}/web/search",
                headers={"X-Subscription-Token": self.api_key, "Accept": "application/json"},
                params={"q": request.query, "count": request.limit},
            )
            response.raise_for_status()
            data = response.json()
        except httpx.HTTPStatusError as e:
            raise WebProviderError(
                f"Brave search failed: HTTP {e.response.status_code}",
                status_code=e.response.status_code,
            ) from e
        except httpx.RequestError as e:
            raise WebProviderError(f"Brave search failed: {e}") from e

        results: list[WebSearchResult] = []
        for item in data.get("web", {}).get("results", [])[: request.limit]:
            url = item.get("url")
            title = item.get("title")
            if not url or not title:
                continue
            results.append(
                WebSearchResult(
                    title=title,
                    url=url,
                    snippet=item.get("description"),
                    published_date=item.get("page_age") or item.get("age"),
                    source=item.get("profile", {}).get("name") if isinstance(item.get("profile"), dict) else None,
                )
            )
        return WebSearchResponse(query=request.query, results=results, provider=self.provider_type)

    async def health_check(self) -> bool:
        try:
            response = await self.search(WebSearchRequest(query="connection test", limit=1))
            return bool(response.results)
        except Exception:
            return False

from __future__ import annotations

import httpx

from .base import FetchedWebPage, WebFetchProvider, WebProviderError


class FirecrawlFetchProvider(WebFetchProvider):
    provider_type = "firecrawl"

    def __init__(self, api_key: str, base_url: str | None = None) -> None:
        self.api_key = api_key
        root = (base_url or "https://api.firecrawl.dev").rstrip("/")
        self.base_url = root if root.endswith("/v1") else f"{root}/v1"
        self.client = httpx.AsyncClient(timeout=httpx.Timeout(40.0, connect=5.0))

    async def fetch(self, url: str) -> FetchedWebPage:
        try:
            response = await self.client.post(
                f"{self.base_url}/scrape",
                headers={"Authorization": f"Bearer {self.api_key}", "Content-Type": "application/json"},
                json={"url": url, "formats": ["markdown"]},
            )
            response.raise_for_status()
            payload = response.json()
        except httpx.HTTPStatusError as e:
            raise WebProviderError(
                f"Firecrawl fetch failed: HTTP {e.response.status_code}",
                status_code=e.response.status_code,
            ) from e
        except httpx.RequestError as e:
            raise WebProviderError(f"Firecrawl fetch failed: {e}") from e

        if payload.get("success") is False:
            raise WebProviderError(str(payload.get("error") or "Firecrawl returned an error"))

        data = payload.get("data") or payload
        content = data.get("markdown") or data.get("content")
        if not content:
            raise WebProviderError("Firecrawl returned empty content for URL")
        metadata = data.get("metadata") or {}
        title = metadata.get("title") if isinstance(metadata, dict) else None
        description = metadata.get("description") if isinstance(metadata, dict) else None
        source_url = metadata.get("sourceURL") if isinstance(metadata, dict) else None
        status_code = metadata.get("statusCode") if isinstance(metadata, dict) else None
        return FetchedWebPage(
            url=source_url or url,
            title=title,
            content=content,
            description=description,
            status_code=status_code if isinstance(status_code, int) else None,
            content_type="text/markdown",
        )

    async def health_check(self) -> bool:
        return True

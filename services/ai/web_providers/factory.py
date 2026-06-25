from __future__ import annotations

from .base import WebFetchProvider, WebSearchProvider
from .brave import BraveSearchProvider
from .exa import ExaFetchProvider, ExaSearchProvider
from .firecrawl import FirecrawlFetchProvider
from .serper import SerperSearchProvider


def create_web_search_provider(provider_type: str, **kwargs) -> WebSearchProvider:
    if provider_type == "exa":
        api_key = kwargs.get("api_key")
        if not api_key:
            raise ValueError("api_key is required for Exa search provider")
        return ExaSearchProvider(api_key=api_key, base_url=kwargs.get("base_url"))
    if provider_type == "serper":
        api_key = kwargs.get("api_key")
        if not api_key:
            raise ValueError("api_key is required for Serper search provider")
        return SerperSearchProvider(api_key=api_key, base_url=kwargs.get("base_url"))
    if provider_type == "brave":
        api_key = kwargs.get("api_key")
        if not api_key:
            raise ValueError("api_key is required for Brave search provider")
        return BraveSearchProvider(api_key=api_key, base_url=kwargs.get("base_url"))
    raise ValueError(f"Unknown web search provider type: {provider_type}")


def create_web_fetch_provider(provider_type: str, **kwargs) -> WebFetchProvider:
    if provider_type == "exa":
        api_key = kwargs.get("api_key")
        if not api_key:
            raise ValueError("api_key is required for Exa fetch provider")
        return ExaFetchProvider(api_key=api_key, base_url=kwargs.get("base_url"))
    if provider_type == "firecrawl":
        api_key = kwargs.get("api_key")
        if not api_key:
            raise ValueError("api_key is required for Firecrawl fetch provider")
        return FirecrawlFetchProvider(api_key=api_key, base_url=kwargs.get("base_url"))
    raise ValueError(f"Unknown web fetch provider type: {provider_type}")

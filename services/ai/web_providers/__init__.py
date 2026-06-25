"""Web search and web page fetch provider abstractions."""

from .base import (
    FetchedWebPage,
    WebFetchProvider,
    WebProviderError,
    WebSearchProvider,
    WebSearchRequest,
    WebSearchResponse,
    WebSearchResult,
)
from .factory import create_web_fetch_provider, create_web_search_provider

__all__ = [
    "FetchedWebPage",
    "WebFetchProvider",
    "WebProviderError",
    "WebSearchProvider",
    "WebSearchRequest",
    "WebSearchResponse",
    "WebSearchResult",
    "create_web_fetch_provider",
    "create_web_search_provider",
]

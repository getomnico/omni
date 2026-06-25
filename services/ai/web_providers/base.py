from __future__ import annotations

from abc import ABC, abstractmethod
from dataclasses import dataclass, field


class WebProviderError(Exception):
    """Raised when a web search/fetch provider fails."""

    def __init__(self, message: str, *, status_code: int | None = None):
        super().__init__(message)
        self.message = message
        self.status_code = status_code


@dataclass
class WebSearchRequest:
    query: str
    limit: int = 10


@dataclass
class WebSearchResult:
    title: str
    url: str
    snippet: str | None = None
    published_date: str | None = None
    source: str | None = None
    metadata: dict[str, str | int | float | bool | None] = field(default_factory=dict)


@dataclass
class WebSearchResponse:
    query: str
    results: list[WebSearchResult]
    provider: str


@dataclass
class FetchedWebPage:
    url: str
    title: str | None
    content: str
    description: str | None = None
    status_code: int | None = None
    content_type: str | None = None
    metadata: dict[str, str | int | float | bool | None] = field(default_factory=dict)


class WebSearchProvider(ABC):
    provider_type: str

    @abstractmethod
    async def search(self, request: WebSearchRequest) -> WebSearchResponse:
        pass

    @abstractmethod
    async def health_check(self) -> bool:
        pass


class WebFetchProvider(ABC):
    provider_type: str

    @abstractmethod
    async def fetch(self, url: str) -> FetchedWebPage:
        pass

    @abstractmethod
    async def health_check(self) -> bool:
        pass

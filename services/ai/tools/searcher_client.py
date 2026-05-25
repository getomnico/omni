"""
Client for communicating with the omni-searcher service.
"""

from __future__ import annotations

import logging
import os
import sys

import httpx
from pydantic import BaseModel

from db.models import UserConfiguration

logger = logging.getLogger(__name__)

JsonObject = dict[str, object]


class SearchRequest(BaseModel):
    query: str
    source_types: list[str] | None = None
    content_types: list[str] | None = None
    limit: int = 20
    offset: int = 0
    mode: str = "hybrid"
    user_id: str | None = None
    user_email: str | None = None
    user_configuration: UserConfiguration | None = None
    is_generated_query: bool | None = None
    original_user_query: str | None = None
    document_id: str | None = None
    document_content_start_line: int | None = None
    document_content_end_line: int | None = None
    include_facets: bool | None = None
    ignore_typos: bool | None = None
    attribute_filters: dict | None = None


class Document(BaseModel):
    id: str
    title: str
    content_type: str | None
    url: str | None
    source_type: str | None = None
    attributes: dict | None = None
    metadata: dict | None = None


class SearchResult(BaseModel):
    document: Document
    highlights: list[str]
    source_type: str | None = None


class SearchResponse(BaseModel):
    results: list[SearchResult]
    total_count: int
    query_time_ms: int


class SearcherError(httpx.HTTPStatusError):
    """Custom error for searcher API call failures."""

    pass


class PeopleSearchRequest(BaseModel):
    query: str
    limit: int = 10


class PersonResult(BaseModel):
    id: str
    email: str
    display_name: str | None = None
    given_name: str | None = None
    surname: str | None = None
    job_title: str | None = None
    department: str | None = None
    score: float


class PeopleSearchResponse(BaseModel):
    people: list[PersonResult]


class CapabilityUpsert(BaseModel):
    id: str
    capability_type: str
    search_text: str
    data: JsonObject
    user_id: str | None = None


class CapabilitiesUpsertRequest(BaseModel):
    capabilities: list[CapabilityUpsert]


class CapabilitiesUpsertResponse(BaseModel):
    upserted: int


class CapabilitySearchRequest(BaseModel):
    capability_type: str
    query: str
    limit: int = 10
    allowed_ids: list[str] | None = None
    allowed_source_ids: list[str] | None = None


class CapabilitySearchResult(BaseModel):
    id: str
    capability_type: str
    search_text: str
    data: JsonObject
    score: float
    user_id: str | None = None


class CapabilitySearchResponse(BaseModel):
    results: list[CapabilitySearchResult]


class SearcherClient:
    """Client for calling omni-searcher service"""

    def __init__(self):
        searcher_url = os.getenv("SEARCHER_URL")
        if not searcher_url:
            print(
                "ERROR: SEARCHER_URL environment variable is not set", file=sys.stderr
            )
            print(
                "Please set this variable to point to your searcher service",
                file=sys.stderr,
            )
            sys.exit(1)

        self.searcher_url = searcher_url.rstrip("/")
        self.client = httpx.AsyncClient(timeout=30.0)

    async def search_documents(self, request: SearchRequest) -> SearchResponse:
        """
        Search documents using omni-searcher service

        Returns:
            dict: Search results with 'success' boolean and either 'results'/'total_count' or 'error'
        """
        try:
            search_payload = request.model_dump(mode="json")

            logger.info(f"Calling searcher service with query: {request.query}...")

            response = await self.client.post(
                f"{self.searcher_url}/search", json=search_payload
            )

            if response.status_code == 200:
                search_results = SearchResponse.model_validate(response.json())
                logger.info(f"Search completed: {search_results.total_count} results")
                return search_results
            else:
                logger.error(
                    f"Search service error: {response.status_code} - {response.text}"
                )
                raise SearcherError(
                    message=f"Searcher API call failed: {response.status_code} {response.text}",
                    request=response.request,
                    response=response,
                )
        except Exception as e:
            logger.error(f"Call to searcher service failed: {e}")
            raise

    async def search_people(self, request: PeopleSearchRequest) -> PeopleSearchResponse:
        """Search the people directory using omni-searcher service."""
        try:
            logger.info(f"People search with query: {request.query}...")
            response = await self.client.get(
                f"{self.searcher_url}/people/search",
                params={"q": request.query, "limit": request.limit},
            )

            if response.status_code == 200:
                result = PeopleSearchResponse.model_validate(response.json())
                logger.info(f"People search completed: {len(result.people)} results")
                return result
            else:
                logger.error(
                    f"People search error: {response.status_code} - {response.text}"
                )
                raise SearcherError(
                    message=f"People search failed: {response.status_code} {response.text}",
                    request=response.request,
                    response=response,
                )
        except SearcherError:
            raise
        except Exception as e:
            logger.error(f"People search failed: {e}")
            raise

    async def upsert_capabilities(
        self, request: CapabilitiesUpsertRequest
    ) -> CapabilitiesUpsertResponse:
        """Publish searchable agent capability projections to omni-searcher."""
        response = await self.client.post(
            f"{self.searcher_url}/capabilities/upsert",
            json=request.model_dump(),
        )
        if response.status_code == 200:
            return CapabilitiesUpsertResponse.model_validate(response.json())
        logger.error(
            f"Capability upsert error: {response.status_code} - {response.text}"
        )
        raise SearcherError(
            message=f"Capability upsert failed: {response.status_code} {response.text}",
            request=response.request,
            response=response,
        )

    async def search_capabilities(
        self, request: CapabilitySearchRequest
    ) -> CapabilitySearchResponse:
        """Search agent capabilities using omni-searcher's ParadeDB index."""
        response = await self.client.post(
            f"{self.searcher_url}/capabilities/search",
            json=request.model_dump(),
        )
        if response.status_code == 200:
            return CapabilitySearchResponse.model_validate(response.json())
        logger.error(
            f"Capability search error: {response.status_code} - {response.text}"
        )
        raise SearcherError(
            message=f"Capability search failed: {response.status_code} {response.text}",
            request=response.request,
            response=response,
        )

    async def get_attribute_values(
        self, keys: list[str], limit: int = 25
    ) -> dict[str, list[str]]:
        """Fetch distinct values for the given attribute keys from the index."""
        try:
            response = await self.client.get(
                f"{self.searcher_url}/attributes/values",
                params={"keys": ",".join(keys), "limit": limit},
            )
            if response.status_code == 200:
                data = response.json()
                return data.get("attributes", {})
            else:
                logger.error(
                    f"Attribute values fetch error: {response.status_code} - {response.text}"
                )
                return {}
        except Exception as e:
            logger.error(f"Failed to fetch attribute values: {e}")
            return {}

    async def close(self):
        """Close the HTTP client"""
        await self.client.aclose()

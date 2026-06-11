"""
Client for communicating with the omni-searcher service.
"""

import os
import sys
import logging
from typing import Optional, List
from pydantic import BaseModel
import httpx

logger = logging.getLogger(__name__)


class SearchRequest(BaseModel):
    query: str
    source_types: Optional[List[str]] = None
    content_types: Optional[List[str]] = None
    limit: int = 20
    offset: int = 0
    mode: str = "hybrid"
    user_id: Optional[str] = None
    user_email: Optional[str] = None
    user_timezone: Optional[str] = None
    is_generated_query: Optional[bool] = None
    original_user_query: Optional[str] = None
    document_id: Optional[str] = None
    document_content_start_line: Optional[int] = None
    document_content_end_line: Optional[int] = None
    include_facets: Optional[bool] = None
    ignore_typos: Optional[bool] = None
    attribute_filters: Optional[dict] = None


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
    display_name: Optional[str] = None
    given_name: Optional[str] = None
    surname: Optional[str] = None
    job_title: Optional[str] = None
    department: Optional[str] = None
    score: float


class PeopleSearchResponse(BaseModel):
    people: list[PersonResult]


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
            search_payload = {
                "query": request.query,
                "source_types": request.source_types,
                "content_types": request.content_types,
                "limit": request.limit,
                "offset": request.offset,
                "mode": request.mode,
                "user_id": request.user_id,
                "user_email": request.user_email,
                "user_timezone": request.user_timezone,
                "is_generated_query": request.is_generated_query,
                "original_user_query": request.original_user_query,
                "document_id": request.document_id,
                "document_content_start_line": request.document_content_start_line,
                "document_content_end_line": request.document_content_end_line,
                "include_facets": request.include_facets,
                "ignore_typos": request.ignore_typos,
                "attribute_filters": request.attribute_filters,
            }

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

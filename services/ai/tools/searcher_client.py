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
    sources: Optional[List[str]] = None
    content_types: Optional[List[str]] = None
    limit: int = 20
    offset: int = 0
    mode: str = "hybrid"
    user_id: Optional[str] = None
    user_email: Optional[str] = None
    is_generated_query: Optional[bool] = None
    original_user_query: Optional[str] = None

class Document(BaseModel):
    title: str
    content_type: str | None
    url: str | None

class SearchResult(BaseModel):
    document: Document
    highlights: list[str]

class SearchResponse(BaseModel):
    results: list[SearchResult]
    total_count: int
    query_time_ms: int

class SearcherError(httpx.HTTPStatusError):
    """Custom error for searcher API call failures."""
    pass

class SearcherClient:
    """Client for calling omni-searcher service"""

    def __init__(self):
        searcher_url = os.getenv("SEARCHER_URL")
        if not searcher_url:
            print("ERROR: SEARCHER_URL environment variable is not set", file=sys.stderr)
            print("Please set this variable to point to your searcher service", file=sys.stderr)
            sys.exit(1)

        self.searcher_url = searcher_url.rstrip('/')
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
                "sources": request.sources,
                "content_types": request.content_types,
                "limit": request.limit,
                "offset": request.offset,
                "mode": request.mode,
                "user_id": request.user_id,
                "user_email": request.user_email,
                "is_generated_query": request.is_generated_query,
                "original_user_query": request.original_user_query,
            }

            logger.info(f"Calling searcher service with query: {request.query}...")

            response = await self.client.post(
                f"{self.searcher_url}/search",
                json=search_payload
            )

            if response.status_code == 200:
                search_results = SearchResponse.model_validate(response.json())
                logger.info(f"Search completed: {search_results.total_count} results")
                return search_results
            else:
                logger.error(f"Search service error: {response.status_code} - {response.text}")
                raise SearcherError(
                    message=f"Searcher API call failed: {response.status_code} {response.text}",
                    request=response.request,
                    response=response,
                ) 
        except Exception as e:
            logger.error(f"Call to searcher service failed: {e}")
            raise

    async def close(self):
        """Close the HTTP client"""
        await self.client.aclose()
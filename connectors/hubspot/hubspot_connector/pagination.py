"""Pagination utilities for HubSpot API."""

import logging
from collections.abc import AsyncIterator
from typing import Any

from .client import HubSpotClient

logger = logging.getLogger(__name__)


async def paginate_all(
    client: HubSpotClient,
    object_type: str,
) -> AsyncIterator[dict[str, Any]]:
    """
    Async generator that yields all objects of a type with pagination.

    HubSpot API uses cursor-based pagination:
    - Request includes `limit` (max 100) and optional `after` cursor
    - Response includes `paging.next.after` if more pages exist

    Args:
        client: HubSpot API client
        object_type: Type of object to fetch (contacts, companies, etc.)

    Yields:
        Individual HubSpot objects as dictionaries
    """
    after: str | None = None
    page_count = 0

    while True:
        page_count += 1
        logger.debug("Fetching %s page %d (after=%s)", object_type, page_count, after)

        response = await client.get_objects(
            object_type=object_type,
            after=after,
        )

        # Yield each object in the page
        for obj in response.results:
            # Convert SDK object to dict if needed
            if hasattr(obj, "to_dict"):
                yield obj.to_dict()
            else:
                yield obj

        # Check for next page
        if response.paging and response.paging.next and response.paging.next.after:
            after = response.paging.next.after
        else:
            logger.debug(
                "Finished paginating %s: %d pages total",
                object_type,
                page_count,
            )
            break

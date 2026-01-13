"""HubSpot API client wrapper with async support and retry logic."""

import asyncio
import logging
from functools import wraps
from typing import Any

from hubspot import HubSpot
from hubspot.crm.contacts import ApiException

from .config import BATCH_SIZE, HUBSPOT_OBJECT_CONFIGS

logger = logging.getLogger(__name__)


class HubSpotError(Exception):
    """Base exception for HubSpot API errors."""

    pass


class AuthenticationError(HubSpotError):
    """Invalid or expired token (401)."""

    pass


class NotFoundError(HubSpotError):
    """Resource not found (404)."""

    pass


def with_retry(max_retries: int = 3, base_delay: float = 1.0):
    """
    Decorator for retrying HubSpot API calls with exponential backoff.

    Handles:
    - 429 Rate Limit: Wait for Retry-After header (unlimited retries)
    - 500/502/503/504: Exponential backoff (limited retries)
    - 401: Re-raise immediately (auth issue)
    """

    def decorator(func):
        @wraps(func)
        async def wrapper(*args, **kwargs):
            last_exception = None
            error_retries = 0

            while True:
                try:
                    return await func(*args, **kwargs)
                except ApiException as e:
                    last_exception = e

                    if e.status == 401:
                        raise AuthenticationError(
                            "Invalid or expired access token"
                        ) from e

                    if e.status == 404:
                        raise NotFoundError(f"Resource not found: {e.body}") from e

                    if e.status == 429:
                        # Rate limits don't count against max_retries
                        retry_after = 10
                        if e.headers:
                            retry_after = int(e.headers.get("Retry-After", 10))
                        logger.warning(
                            "Rate limited. Waiting %ds",
                            retry_after,
                        )
                        await asyncio.sleep(retry_after)
                        continue

                    if e.status >= 500:
                        error_retries += 1
                        if error_retries > max_retries:
                            break
                        delay = base_delay * (2 ** (error_retries - 1))
                        logger.warning(
                            "Server error %d. Retrying in %.1fs (attempt %d/%d)",
                            e.status,
                            delay,
                            error_retries,
                            max_retries,
                        )
                        await asyncio.sleep(delay)
                        continue

                    # Other errors: re-raise immediately
                    raise HubSpotError(f"API error {e.status}: {e.body}") from e

            raise HubSpotError(
                f"Max retries exceeded: {last_exception}"
            ) from last_exception

        return wrapper

    return decorator


class HubSpotClient:
    """Wrapper around HubSpot API client with async support."""

    def __init__(self, access_token: str):
        """
        Initialize with OAuth access token or private app token.

        The token should have these scopes:
        - crm.objects.contacts.read
        - crm.objects.companies.read
        - crm.objects.deals.read
        - tickets
        - e-commerce (for engagements)
        """
        self._client = HubSpot(access_token=access_token)
        self._access_token = access_token

    @with_retry(max_retries=3)
    async def get_objects(
        self,
        object_type: str,
        after: str | None = None,
    ) -> Any:
        """
        Get a page of objects of the specified type.

        Args:
            object_type: Type of object (contacts, companies, deals, etc.)
            after: Cursor for pagination

        Returns:
            Response with results and paging info
        """
        config = HUBSPOT_OBJECT_CONFIGS.get(object_type, {})
        properties = config.get("properties", [])

        # Use asyncio.to_thread for sync HubSpot SDK calls
        api = self._get_api_for_type(object_type)

        return await asyncio.to_thread(
            api.get_page,
            limit=BATCH_SIZE,
            properties=properties,
            after=after,
        )

    def _get_api_for_type(self, object_type: str) -> Any:
        """Get the appropriate API client for the object type."""
        api_map = {
            "contacts": self._client.crm.contacts.basic_api,
            "companies": self._client.crm.companies.basic_api,
            "deals": self._client.crm.deals.basic_api,
            "tickets": self._client.crm.tickets.basic_api,
            "calls": self._client.crm.objects.calls.basic_api,
            "emails": self._client.crm.objects.emails.basic_api,
            "meetings": self._client.crm.objects.meetings.basic_api,
            "notes": self._client.crm.objects.notes.basic_api,
            "tasks": self._client.crm.objects.tasks.basic_api,
        }

        api = api_map.get(object_type)
        if not api:
            raise HubSpotError(f"Unsupported object type: {object_type}")

        return api

    async def test_connection(self) -> bool:
        """Test the connection by fetching a single contact."""
        # Reuse get_objects which has retry logic
        await self.get_objects("contacts", after=None)
        return True

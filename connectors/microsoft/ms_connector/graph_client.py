"""Thin async wrapper over Microsoft Graph REST API with retry logic."""

import asyncio
import logging
from collections.abc import AsyncIterator
from functools import wraps
from typing import Any

import httpx

from .auth import MSGraphAuth

logger = logging.getLogger(__name__)

GRAPH_BASE_URL = "https://graph.microsoft.com/v1.0"


class GraphAPIError(Exception):
    """Base exception for Graph API errors."""

    def __init__(self, message: str, status_code: int | None = None):
        super().__init__(message)
        self.status_code = status_code


class AuthenticationError(GraphAPIError):
    """Invalid or expired credentials (401)."""

    pass


def with_retry(max_retries: int = 3, base_delay: float = 1.0):
    """Decorator for retrying Graph API calls with exponential backoff.

    Handles:
    - 429 Rate Limit: Wait for Retry-After header (unlimited retries)
    - 5xx Server Error: Exponential backoff (limited retries)
    - 401 Unauthorized: Re-authenticate once, then fail
    """

    def decorator(func):
        @wraps(func)
        async def wrapper(self, *args, **kwargs):
            last_exception = None
            error_retries = 0
            auth_retried = False

            while True:
                try:
                    return await func(self, *args, **kwargs)
                except httpx.HTTPStatusError as e:
                    last_exception = e
                    status = e.response.status_code

                    if status == 401:
                        if auth_retried:
                            raise AuthenticationError(
                                "Authentication failed after token refresh",
                                status_code=401,
                            ) from e
                        auth_retried = True
                        logger.warning("Got 401, refreshing token and retrying")
                        self._refresh_token()
                        continue

                    if status == 404:
                        raise GraphAPIError(
                            f"Resource not found: {e.response.text}",
                            status_code=404,
                        ) from e

                    if status == 429:
                        retry_after = int(e.response.headers.get("Retry-After", "10"))
                        logger.warning("Rate limited. Waiting %ds", retry_after)
                        await asyncio.sleep(retry_after)
                        continue

                    if status >= 500:
                        error_retries += 1
                        if error_retries > max_retries:
                            break
                        delay = base_delay * (2 ** (error_retries - 1))
                        logger.warning(
                            "Server error %d. Retrying in %.1fs (%d/%d)",
                            status,
                            delay,
                            error_retries,
                            max_retries,
                        )
                        await asyncio.sleep(delay)
                        continue

                    raise GraphAPIError(
                        f"API error {status}: {e.response.text}",
                        status_code=status,
                    ) from e

            raise GraphAPIError(
                f"Max retries exceeded: {last_exception}"
            ) from last_exception

        return wrapper

    return decorator


class GraphClient:
    """Async client for Microsoft Graph API v1.0."""

    def __init__(
        self,
        auth: MSGraphAuth,
        http_client: httpx.AsyncClient | None = None,
        base_url: str = GRAPH_BASE_URL,
    ):
        self._auth = auth
        self._client = http_client or httpx.AsyncClient(
            base_url=base_url,
            timeout=httpx.Timeout(30.0, connect=10.0),
        )
        self._refresh_token()

    def _refresh_token(self) -> None:
        token = self._auth.get_token()
        self._client.headers["Authorization"] = f"Bearer {token}"

    async def close(self) -> None:
        await self._client.aclose()

    @with_retry(max_retries=3)
    async def get(
        self, url: str, params: dict[str, Any] | None = None
    ) -> dict[str, Any]:
        """Execute a GET request against the Graph API."""
        response = await self._client.get(url, params=params)
        response.raise_for_status()
        return response.json()

    @with_retry(max_retries=3)
    async def get_binary(self, url: str) -> bytes:
        """Execute a GET request that returns binary content (file downloads)."""
        response = await self._client.get(url, follow_redirects=True)
        response.raise_for_status()
        return response.content

    async def get_paginated(
        self,
        url: str,
        params: dict[str, Any] | None = None,
    ) -> AsyncIterator[dict[str, Any]]:
        """Follow @odata.nextLink pagination, yielding individual items."""
        next_url: str | None = url
        next_params = params

        while next_url:
            data = await self.get(next_url, params=next_params)
            for item in data.get("value", []):
                yield item

            next_link = data.get("@odata.nextLink")
            if next_link:
                # nextLink is an absolute URL with params baked in
                next_url = next_link
                next_params = None
            else:
                next_url = None

    async def get_delta(
        self,
        url: str,
        delta_token: str | None = None,
        params: dict[str, Any] | None = None,
    ) -> tuple[list[dict[str, Any]], str | None]:
        """Execute a delta query, following all pages.

        Returns (items, new_delta_token). On first call, pass delta_token=None
        for a full snapshot. On subsequent calls, pass the previously returned
        delta_token for incremental changes.
        """
        if delta_token:
            # Delta link is an absolute URL — use it directly
            next_url: str | None = delta_token
            next_params = None
        else:
            next_url = url
            next_params = params

        items: list[dict[str, Any]] = []
        new_delta_token: str | None = None

        while next_url:
            data = await self.get(next_url, params=next_params)
            items.extend(data.get("value", []))

            next_link = data.get("@odata.nextLink")
            if next_link:
                next_url = next_link
                next_params = None
            else:
                new_delta_token = data.get("@odata.deltaLink")
                next_url = None

        return items, new_delta_token

    async def list_users(self) -> list[dict[str, Any]]:
        """Enumerate all users in the tenant."""
        users: list[dict[str, Any]] = []
        async for user in self.get_paginated(
            "/users",
            params={"$select": "id,displayName,mail,userPrincipalName"},
        ):
            users.append(user)
        return users

    async def search_users(self, query: str, limit: int = 20) -> list[dict[str, Any]]:
        """Search users by displayName or mail using $filter with startsWith."""
        filter_expr = f"startswith(displayName,'{query}') or startswith(mail,'{query}')"
        data = await self.get(
            "/users",
            params={
                "$filter": filter_expr,
                "$select": "id,displayName,mail,userPrincipalName",
                "$top": str(limit),
            },
        )
        return data.get("value", [])

    async def list_groups(self) -> list[dict[str, Any]]:
        """Enumerate all groups in the tenant."""
        groups: list[dict[str, Any]] = []
        async for group in self.get_paginated(
            "/groups",
            params={
                "$select": "id,displayName,mail,mailEnabled,securityEnabled",
            },
        ):
            groups.append(group)
        return groups

    async def list_group_members(self, group_id: str) -> list[dict[str, Any]]:
        """Enumerate all members of a group."""
        members: list[dict[str, Any]] = []
        async for member in self.get_paginated(
            f"/groups/{group_id}/members",
            params={"$select": "id,displayName,mail,userPrincipalName"},
        ):
            members.append(member)
        return members

    async def list_item_permissions(
        self, drive_id: str, item_id: str
    ) -> list[dict[str, Any]]:
        """List sharing permissions on a driveItem."""
        permissions: list[dict[str, Any]] = []
        async for perm in self.get_paginated(
            f"/drives/{drive_id}/items/{item_id}/permissions",
        ):
            permissions.append(perm)
        return permissions

    async def list_teams(self) -> list[dict[str, Any]]:
        """List all teams in the tenant (M365 groups with Teams provisioned)."""
        teams: list[dict[str, Any]] = []
        async for team in self.get_paginated(
            "/groups",
            params={
                "$filter": "resourceProvisionedPlans/any(p: p/providingService eq 'MCO')",
                "$select": "id,displayName,mail,description",
            },
        ):
            teams.append(team)
        return teams

    async def list_team_channels(self, team_id: str) -> list[dict[str, Any]]:
        """List all channels in a team."""
        channels: list[dict[str, Any]] = []
        async for channel in self.get_paginated(
            f"/teams/{team_id}/channels",
            params={"$select": "id,displayName,membershipType,description"},
        ):
            channels.append(channel)
        return channels

    async def list_channel_members(
        self, team_id: str, channel_id: str
    ) -> list[dict[str, Any]]:
        """List members of a channel (for private/shared channels)."""
        members: list[dict[str, Any]] = []
        async for member in self.get_paginated(
            f"/teams/{team_id}/channels/{channel_id}/members",
            params={"$select": "id,displayName,email,userId"},
        ):
            members.append(member)
        return members

    async def list_message_attachments(
        self, user_id: str, message_id: str
    ) -> list[dict[str, Any]]:
        """Fetch file attachments for an Outlook message.

        Returns only file attachments (not item or reference attachments),
        excluding inline attachments (embedded images).
        """
        attachments: list[dict[str, Any]] = []
        async for att in self.get_paginated(
            f"/users/{user_id}/messages/{message_id}/attachments",
            params={"$select": "id,name,contentType,size,contentBytes,isInline"},
        ):
            if att.get("@odata.type") != "#microsoft.graph.fileAttachment":
                continue
            if att.get("isInline", False):
                continue
            attachments.append(att)
        return attachments

    async def get_channel_messages_delta(
        self,
        team_id: str,
        channel_id: str,
        delta_token: str | None = None,
        filter_from: str | None = None,
    ) -> tuple[list[dict[str, Any]], str | None]:
        """Fetch channel messages using delta query for incremental sync.

        If filter_from is set (ISO datetime) and delta_token is None,
        adds $filter=lastModifiedDateTime gt <date> to scope the query.
        """
        params: dict[str, str] = {
            "$select": "id,body,from,createdDateTime,lastModifiedDateTime,"
            "replyToId,attachments,mentions,reactions,messageType",
        }
        if filter_from and delta_token is None:
            params["$filter"] = f"lastModifiedDateTime gt {filter_from}"
        return await self.get_delta(
            f"/teams/{team_id}/channels/{channel_id}/messages/delta",
            delta_token=delta_token,
            params=params,
        )

    async def list_message_replies(
        self, team_id: str, channel_id: str, message_id: str
    ) -> list[dict[str, Any]]:
        """List all replies to a channel message."""
        replies: list[dict[str, Any]] = []
        async for reply in self.get_paginated(
            f"/teams/{team_id}/channels/{channel_id}/messages/{message_id}/replies",
            params={
                "$select": "id,body,from,createdDateTime,lastModifiedDateTime,"
                "attachments,mentions,reactions,messageType",
            },
        ):
            replies.append(reply)
        return replies

    async def resolve_share(self, share_url: str) -> dict[str, Any]:
        """Resolve a SharePoint sharing URL to its underlying driveItem.

        Encodes the URL as a share token and calls the Shares API.
        Used to resolve Teams file attachments to their SharePoint driveItem
        for content extraction and dedup.
        """
        import base64

        encoded = base64.urlsafe_b64encode(share_url.encode()).decode().rstrip("=")
        share_token = f"u!{encoded}"
        return await self.get(f"/shares/{share_token}/driveItem")

    async def test_connection(self) -> None:
        """Validate credentials by calling /organization."""
        await self.get("/organization", params={"$select": "id,displayName"})

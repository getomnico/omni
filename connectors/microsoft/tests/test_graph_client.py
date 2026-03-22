"""Tests for GraphClient retry, auth, and delta pagination."""

from unittest.mock import MagicMock

import httpx
import pytest
import respx

from ms_connector.graph_client import (
    AuthenticationError,
    GraphAPIError,
    GraphClient,
    GRAPH_BASE_URL,
)


@pytest.fixture
def mock_auth():
    auth = MagicMock()
    auth.get_token.return_value = "fake-token"
    return auth


@pytest.fixture
def mock_router():
    with respx.mock(assert_all_called=False) as router:
        yield router


@pytest.fixture
def graph_client(mock_auth, mock_router):
    http_client = httpx.AsyncClient(
        base_url=GRAPH_BASE_URL,
        timeout=httpx.Timeout(30.0, connect=10.0),
    )
    return GraphClient(mock_auth, http_client=http_client)


async def test_retry_on_429_and_500(graph_client, mock_router):
    route = mock_router.get(url__eq=f"{GRAPH_BASE_URL}/users")
    route.side_effect = [
        httpx.Response(429, headers={"Retry-After": "0"}),
        httpx.Response(200, json={"value": []}),
    ]
    result = await graph_client.get("/users")
    assert result == {"value": []}
    assert route.call_count == 2

    route_fail = mock_router.get(url__eq=f"{GRAPH_BASE_URL}/me")
    route_fail.mock(return_value=httpx.Response(500, text="Internal Server Error"))
    with pytest.raises(GraphAPIError, match="Max retries exceeded"):
        await graph_client.get("/me")


async def test_401_refreshes_token_then_fails(graph_client, mock_auth, mock_router):
    mock_router.get(url__eq=f"{GRAPH_BASE_URL}/organization").mock(
        return_value=httpx.Response(401, json={"error": {"message": "Unauthorized"}})
    )
    with pytest.raises(AuthenticationError):
        await graph_client.get("/organization")
    assert mock_auth.get_token.call_count >= 2


async def test_list_groups(graph_client, mock_router):
    mock_router.get(url__regex=r".*/groups(\?.*)?$").mock(
        return_value=httpx.Response(
            200,
            json={
                "value": [
                    {
                        "id": "grp-1",
                        "displayName": "Engineering",
                        "mail": "engineering@contoso.com",
                        "mailEnabled": True,
                        "securityEnabled": True,
                    },
                    {
                        "id": "grp-2",
                        "displayName": "Sales",
                        "mail": "sales@contoso.com",
                        "mailEnabled": True,
                        "securityEnabled": False,
                    },
                ]
            },
        )
    )
    groups = await graph_client.list_groups()
    assert len(groups) == 2
    assert groups[0]["displayName"] == "Engineering"
    assert groups[1]["mail"] == "sales@contoso.com"


async def test_list_group_members(graph_client, mock_router):
    mock_router.get(url__regex=r".*/groups/grp-1/members(\?.*)?$").mock(
        return_value=httpx.Response(
            200,
            json={
                "value": [
                    {
                        "id": "u1",
                        "displayName": "Alice",
                        "mail": "alice@contoso.com",
                        "userPrincipalName": "alice@contoso.com",
                    },
                    {
                        "id": "u2",
                        "displayName": "Bob",
                        "mail": "bob@contoso.com",
                        "userPrincipalName": "bob@contoso.com",
                    },
                ]
            },
        )
    )
    members = await graph_client.list_group_members("grp-1")
    assert len(members) == 2
    assert members[0]["mail"] == "alice@contoso.com"


async def test_delta_query_with_pagination(graph_client, mock_router):
    page2_url = f"{GRAPH_BASE_URL}/delta?page=2"

    async def delta_handler(request):
        url = str(request.url)
        if "page=2" in url:
            return httpx.Response(
                200,
                json={
                    "value": [{"id": "item-2"}],
                    "@odata.deltaLink": f"{GRAPH_BASE_URL}/delta?token=xyz",
                },
            )
        return httpx.Response(
            200,
            json={
                "value": [{"id": "item-1"}],
                "@odata.nextLink": page2_url,
            },
        )

    mock_router.get(url__regex=r".*/users/u1/drive/root/delta.*").mock(
        side_effect=delta_handler
    )
    mock_router.get(url__regex=r".*/delta\?page=2").mock(side_effect=delta_handler)

    items, token = await graph_client.get_delta("/users/u1/drive/root/delta")
    assert len(items) == 2
    assert token is not None

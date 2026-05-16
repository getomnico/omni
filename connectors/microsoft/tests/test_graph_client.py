"""Tests for GraphClient retry, auth, and delta pagination."""

from unittest.mock import MagicMock

import httpx
import pytest
import respx

from ms_connector.graph_client import (
    GRAPH_BASE_URL,
    AuthenticationError,
    GraphAPIError,
    GraphClient,
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


async def test_list_item_permissions(graph_client, mock_router):
    mock_router.get(url__regex=r".*/drives/d1/items/i1/permissions(\?.*)?$").mock(
        return_value=httpx.Response(
            200,
            json={
                "value": [
                    {
                        "id": "perm-1",
                        "roles": ["write"],
                        "grantedToV2": {
                            "user": {"id": "u1", "displayName": "Alice"},
                        },
                    },
                    {
                        "id": "perm-2",
                        "roles": ["read"],
                        "link": {"scope": "organization", "type": "view"},
                    },
                ]
            },
        )
    )
    perms = await graph_client.list_item_permissions("d1", "i1")
    assert len(perms) == 2
    assert perms[0]["grantedToV2"]["user"]["id"] == "u1"
    assert perms[1]["link"]["scope"] == "organization"


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


async def test_delta_pages_yields_resume_links(graph_client, mock_router):
    page2_url = f"{GRAPH_BASE_URL}/delta?page=2"
    delta_url = f"{GRAPH_BASE_URL}/delta?token=xyz"

    mock_router.get(url__eq=f"{GRAPH_BASE_URL}/users/u1/drive/root/delta").mock(
        return_value=httpx.Response(
            200,
            json={
                "value": [{"id": "item-1"}],
                "@odata.nextLink": page2_url,
            },
        )
    )
    mock_router.get(url__eq=page2_url).mock(
        return_value=httpx.Response(
            200,
            json={
                "value": [{"id": "item-2"}],
                "@odata.deltaLink": delta_url,
            },
        )
    )

    pages = [
        page
        async for page in graph_client.get_delta_pages("/users/u1/drive/root/delta")
    ]

    assert pages == [
        ([{"id": "item-1"}], page2_url, None),
        ([{"id": "item-2"}], None, delta_url),
    ]


async def test_list_teams(graph_client, mock_router):
    mock_router.get(url__regex=r".*/groups(\?.*)?$").mock(
        return_value=httpx.Response(
            200,
            json={
                "value": [
                    {
                        "id": "team-1",
                        "displayName": "Engineering",
                        "mail": "eng@contoso.com",
                        "description": "Eng team",
                    },
                ]
            },
        )
    )
    teams = await graph_client.list_teams()
    assert len(teams) == 1
    assert teams[0]["displayName"] == "Engineering"


async def test_list_team_channels(graph_client, mock_router):
    mock_router.get(url__regex=r".*/teams/team-1/channels(\?.*)?$").mock(
        return_value=httpx.Response(
            200,
            json={
                "value": [
                    {
                        "id": "ch-1",
                        "displayName": "General",
                        "membershipType": "standard",
                    },
                    {
                        "id": "ch-2",
                        "displayName": "Secret",
                        "membershipType": "private",
                    },
                ]
            },
        )
    )
    channels = await graph_client.list_team_channels("team-1")
    assert len(channels) == 2
    assert channels[1]["membershipType"] == "private"


async def test_channel_messages_delta(graph_client, mock_router):
    mock_router.get(url__regex=r".*/teams/t1/channels/c1/messages/delta(\?.*)?$").mock(
        return_value=httpx.Response(
            200,
            json={
                "value": [
                    {
                        "id": "msg-1",
                        "messageType": "message",
                        "body": {"contentType": "text", "content": "Hello"},
                    }
                ],
                "@odata.deltaLink": (
                    f"{GRAPH_BASE_URL}/teams/t1/channels/c1/messages/delta?token=abc"
                ),
            },
        )
    )
    messages, token = await graph_client.get_channel_messages_delta("t1", "c1")
    assert len(messages) == 1
    assert token is not None


async def test_list_message_replies(graph_client, mock_router):
    mock_router.get(
        url__regex=r".*/teams/t1/channels/c1/messages/m1/replies(\?.*)?$"
    ).mock(
        return_value=httpx.Response(
            200,
            json={
                "value": [
                    {
                        "id": "r1",
                        "messageType": "message",
                        "body": {"contentType": "text", "content": "Reply!"},
                    }
                ]
            },
        )
    )
    replies = await graph_client.list_message_replies("t1", "c1", "m1")
    assert len(replies) == 1
    assert replies[0]["id"] == "r1"


async def test_resolve_share(graph_client, mock_router):
    mock_router.get(url__regex=r".*/shares/.*/driveItem(\?.*)?$").mock(
        return_value=httpx.Response(
            200,
            json={
                "id": "item-123",
                "name": "report.pdf",
                "file": {"mimeType": "application/pdf"},
                "parentReference": {
                    "driveId": "drv-1",
                    "siteId": "site-1",
                },
            },
        )
    )
    item = await graph_client.resolve_share(
        "https://contoso.sharepoint.com/sites/Eng/Shared Documents/report.pdf"
    )
    assert item["id"] == "item-123"
    assert item["parentReference"]["siteId"] == "site-1"

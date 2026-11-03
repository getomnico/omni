"""Unit tests for GitHub Enterprise Server support (no Docker, no network)."""

from datetime import datetime
from typing import Any

import httpx
import pytest
import respx
from omni_connector import ActionDefinition, Source
from pydantic import ValidationError

from github_connector.client import GitHubClient
from github_connector.connector import GitHubConnector
from github_connector.models import GitHubSourceConfig
from tests.conftest import _issue_payload, _pr_payload, _repo_payload, _user_payload

GHES_API = "https://ghe.example.com/api/v3"
# githubkit's current latest REST API version; update when githubkit is upgraded.
GITHUBKIT_LATEST_VERSION = "2026-03-10"
GHES_VERSION = "2022-11-28"


def _pages(item: dict[str, Any]) -> list[httpx.Response]:
    return [httpx.Response(200, json=[item]), httpx.Response(200, json=[])]


def _versions(router: respx.MockRouter) -> set[str]:
    return {call.request.headers["X-GitHub-Api-Version"] for call in router.calls}


async def test_github_com_uses_githubkit_default_version() -> None:
    with respx.mock(assert_all_called=False) as router:
        route = router.get("https://api.github.com/user").respond(
            200, json=_user_payload("octocat")
        )
        await GitHubClient("t").validate_token()
        assert route.called
        assert _versions(router) == {GITHUBKIT_LATEST_VERSION}


async def test_ghes_requests_use_ghes_api_version() -> None:
    with respx.mock(assert_all_called=False) as router:
        user = router.get(f"{GHES_API}/user").respond(200, json=_user_payload("octocat"))
        repos = router.get(f"{GHES_API}/user/repos").mock(
            side_effect=_pages(_repo_payload("octocat", "hello"))
        )
        issues = router.get(f"{GHES_API}/repos/octocat/hello/issues").mock(
            side_effect=_pages(_issue_payload("octocat", "hello", 1))
        )
        pulls = router.get(f"{GHES_API}/repos/octocat/hello/pulls").mock(
            side_effect=_pages(_pr_payload("octocat", "hello", 2))
        )

        client = GitHubClient("t", base_url=GHES_API)
        await client.validate_token()
        assert [r async for r in client.list_repos_for_authenticated_user()]
        assert [i async for i in client.list_issues("octocat", "hello")]
        assert [p async for p in client.list_pull_requests("octocat", "hello")]

        assert user.called and repos.called and issues.called and pulls.called
        assert _versions(router) == {GHES_VERSION}


async def test_ghes_api_version_override() -> None:
    with respx.mock(assert_all_called=False) as router:
        router.get(f"{GHES_API}/user").respond(200, json=_user_payload("octocat"))
        await GitHubClient("t", base_url=GHES_API, api_version="2030-01-01").validate_token()
        assert _versions(router) == {"2030-01-01"}


@pytest.mark.parametrize(
    ("base_url", "graphql_url"),
    [
        (GHES_API, "https://ghe.example.com/api/graphql"),
        (None, "https://api.github.com/graphql"),
    ],
)
async def test_graphql_endpoint(base_url: str | None, graphql_url: str) -> None:
    empty = {
        "data": {
            "repository": {
                "discussions": {
                    "pageInfo": {"hasNextPage": False, "endCursor": None},
                    "nodes": [],
                }
            }
        }
    }
    with respx.mock(assert_all_called=False) as router:
        route = router.post(graphql_url).respond(200, json=empty)
        client = GitHubClient("t", base_url=base_url)
        assert [d async for d in client.list_discussions("octocat", "hello")] == []
    assert route.called


@pytest.mark.parametrize(
    "api_url",
    [
        "https://ghe.example.com",
        "https://ghe.example.com/",
        "https://ghe.example.com/api/v3",
        "https://ghe.example.com/api/v3/",
    ],
)
def test_api_url_ghes_forms_normalise(api_url: str) -> None:
    config = GitHubSourceConfig(api_url=api_url)
    assert config.api_url == GHES_API
    assert config.host == "https://ghe.example.com"


@pytest.mark.parametrize(
    "api_url",
    [None, "", "  ", "https://api.github.com", "https://api.github.com/"],
)
def test_api_url_github_com_forms_normalise_to_none(api_url: str | None) -> None:
    config = GitHubSourceConfig(api_url=api_url)
    assert config.api_url is None
    assert config.host is None


def test_api_url_ghe_com_is_kept_as_typed() -> None:
    config = GitHubSourceConfig(api_url="https://api.sub.ghe.com")
    assert config.api_url == "https://api.sub.ghe.com"
    assert config.host == "https://api.sub.ghe.com"


def test_api_url_custom_path_is_kept() -> None:
    config = GitHubSourceConfig(api_url="https://ghe.example.com/custom")
    assert config.api_url == "https://ghe.example.com/custom"


def test_api_url_without_scheme_is_rejected() -> None:
    with pytest.raises(ValidationError):
        GitHubSourceConfig(api_url="ghe.example.com")


def test_api_version_must_be_a_date() -> None:
    assert GitHubSourceConfig(api_version="2026-03-10").api_version == "2026-03-10"
    with pytest.raises(ValidationError):
        GitHubSourceConfig(api_version="nope")


def test_oauth_config_is_github_com_only() -> None:
    """Per-user OAuth targets github.com only; GHES sources authenticate with the PAT.

    The connector manifest is global and cannot declare per-source OAuth endpoints.
    """
    oauth = GitHubConnector().oauth_config()
    assert oauth is not None
    assert oauth.auth_endpoint == "https://github.com/login/oauth/authorize"
    assert oauth.token_endpoint == "https://github.com/login/oauth/access_token"
    assert oauth.userinfo_endpoint == "https://api.github.com/user/emails"


def _source(config: dict[str, Any]) -> Source:
    now = datetime(2026, 1, 1)
    return Source(
        id="src",
        name="GitHub",
        source_type="github",
        config=config,
        is_active=True,
        is_deleted=False,
        scope="org",
        created_at=now,
        updated_at=now,
        created_by="user",
    )


def test_mcp_server_gets_ghes_host() -> None:
    connector = GitHubConnector()
    server = connector.mcp_server_for_source(_source({"api_url": "https://ghe.example.com"}))
    assert server.env == {"GITHUB_HOST": "https://ghe.example.com"}
    assert server.command == connector.mcp_server.command
    assert server.args == connector.mcp_server.args


def test_mcp_server_for_github_com_and_no_source_is_default() -> None:
    connector = GitHubConnector()
    assert connector.mcp_server_for_source(_source({})) == connector.mcp_server
    assert connector.mcp_server_for_source(None) == connector.mcp_server
    assert connector.mcp_server.env is None


def test_prepare_mcp_env_sets_ghes_host_from_source_config_hint() -> None:
    connector = GitHubConnector()
    assert connector.prepare_mcp_env(
        {"token": "t", "_omni_source_config": {"api_url": "https://ghe.example.com"}}
    ) == {"GITHUB_PERSONAL_ACCESS_TOKEN": "t", "GITHUB_HOST": "https://ghe.example.com"}
    assert connector.prepare_mcp_env({"token": "t", "_omni_source_config": {}}) == {
        "GITHUB_PERSONAL_ACCESS_TOKEN": "t"
    }


async def test_mcp_action_names_reuse_default_catalog() -> None:
    connector = GitHubConnector()
    assert connector.mcp_adapter is not None
    # Seeds the adapter's private cache on purpose: avoids spawning github-mcp-server.
    connector.mcp_adapter._cached_actions = [
        ActionDefinition(name="list_issues", description="List issues")
    ]
    ghes = await connector.mcp_action_names_for_source(
        _source({"api_url": "https://ghe.example.com"})
    )
    assert ghes == {"list_issues"}
    assert ghes == await connector.mcp_action_names_for_source(_source({}))


def test_prepare_mcp_env_only_carries_token() -> None:
    assert GitHubConnector().prepare_mcp_env({"token": "t"}) == {
        "GITHUB_PERSONAL_ACCESS_TOKEN": "t"
    }

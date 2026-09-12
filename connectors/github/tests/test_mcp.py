"""Unit tests for the GitHub connector MCP integration and OAuth manifest."""

import json
from datetime import UTC, datetime
from typing import Any

import pytest
from omni_connector import (
    ActionDefinition,
    ActionResponse,
    OAuthCredentialReadyRequest,
    Source,
    StdioMcpServer,
)

from github_connector import GitHubConnector
from github_connector.config import GITHUB_MCP_COMMAND, MCP_TOOLSETS


class FakeMcpAdapter:
    def __init__(self) -> None:
        self.env_seen: dict[str, str] | None = None
        self.discovered = False
        self.executed: tuple[str, dict[str, Any]] | None = None
        self.rejected: str | None = None

    async def discover(self, **auth: Any) -> None:
        self.env_seen = auth.get("env")
        self.discovered = True

    def _export_catalog(self) -> dict[str, Any]:
        return {
            "actions": [
                ActionDefinition(
                    name="issue_write",
                    description="Create or update an issue",
                    input_schema={"type": "object", "properties": {}},
                    mode="write",
                ).model_dump()
            ],
            "resources": [],
            "prompts": [],
        }

    def _save_catalog_cache(self, path: Any) -> None:
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_text(
            json.dumps(
                {"version": 1, "cached_at": 1, "catalog": self._export_catalog()}
            )
        )

    def _catalog_cache_expired(self, ttl_seconds: int) -> bool:
        return False

    def _clear_catalog_cache_if_expired(self, ttl_seconds: int) -> bool:
        return False

    async def get_action_definitions(self, **auth: Any) -> list[ActionDefinition]:
        self.env_seen = auth.get("env")
        return [
            ActionDefinition(
                name="issue_write",
                description="Create or update an issue",
                input_schema={"type": "object", "properties": {}},
                mode="write",
            )
        ]

    async def execute_tool(
        self, name: str, arguments: dict[str, Any], **auth: Any
    ) -> ActionResponse:
        self.env_seen = auth.get("env")
        self.executed = (name, arguments)
        return ActionResponse.success({"name": name, "arguments": arguments})

    async def get_resource_definitions(self, **auth: Any) -> list[Any]:
        return []

    async def get_prompt_definitions(self, **auth: Any) -> list[Any]:
        return []


def make_source(config: dict[str, Any]) -> Source:
    return Source(
        id="src_1",
        name="GitHub",
        source_type="github",
        config=config,
        is_active=True,
        is_deleted=False,
        scope="org",
        created_at=datetime.now(tz=UTC),
        updated_at=datetime.now(tz=UTC),
        created_by="user_1",
    )


# ---------------------------------------------------------------------------
# MCP server configuration
# ---------------------------------------------------------------------------


def test_mcp_server_uses_curated_toolsets() -> None:
    connector = GitHubConnector()
    server = connector.mcp_server

    assert isinstance(server, StdioMcpServer)
    assert server.command == GITHUB_MCP_COMMAND
    assert server.args == ["stdio", "--toolsets", ",".join(MCP_TOOLSETS)]
    # The curated list must never silently widen back to "all".
    assert "all" not in server.args
    # High-risk toolsets are excluded from the default surface.
    for excluded in ("gists", "notifications", "projects", "secret_protection"):
        assert excluded not in MCP_TOOLSETS


# ---------------------------------------------------------------------------
# Credential → env bridging
# ---------------------------------------------------------------------------


@pytest.mark.parametrize(
    "credentials,expected",
    [
        ({"token": "ghp_pat"}, "ghp_pat"),
        ({"access_token": "gho_oauth"}, "gho_oauth"),
        ({"credentials": {"token": "ghp_pat"}}, "ghp_pat"),
        ({"credentials": {"access_token": "gho_oauth"}}, "gho_oauth"),
        # ServiceCredential envelope merged with org setup config.
        (
            {
                "credentials": {"access_token": "gho_oauth", "refresh_token": None},
                "config": {},
                "auth_type": "oauth",
            },
            "gho_oauth",
        ),
    ],
)
def test_prepare_mcp_env_uses_resolved_token(
    credentials: dict[str, Any], expected: str
) -> None:
    connector = GitHubConnector()

    assert connector.prepare_mcp_env(credentials) == {
        "GITHUB_PERSONAL_ACCESS_TOKEN": expected
    }


@pytest.mark.parametrize(
    "credentials",
    [
        {},
        {"credentials": {}},
        {"credentials": {"client_id": "x", "refresh_token": "y"}},
        {"token": "", "access_token": ""},
    ],
)
def test_prepare_mcp_env_rejects_missing_token(credentials: dict[str, Any]) -> None:
    connector = GitHubConnector()

    with pytest.raises(ValueError, match="token"):
        connector.prepare_mcp_env(credentials)


# ---------------------------------------------------------------------------
# OAuth manifest
# ---------------------------------------------------------------------------


def test_oauth_config_declares_github_flow() -> None:
    connector = GitHubConnector()
    oauth = connector.oauth_config()

    assert oauth is not None
    assert oauth.provider == "github"
    assert oauth.auth_endpoint == "https://github.com/login/oauth/authorize"
    assert oauth.token_endpoint == "https://github.com/login/oauth/access_token"
    assert oauth.userinfo_endpoint == "https://api.github.com/user/emails"
    assert oauth.identity_scopes == ["read:user", "user:email"]
    assert oauth.scopes["github"].read == ["repo", "read:org"]
    assert oauth.scopes["github"].write == ["repo"]
    assert oauth.scope_separator == " "


# ---------------------------------------------------------------------------
# Bootstrap + per-user credential hook
# ---------------------------------------------------------------------------


async def test_manifest_includes_connector_skills(tmp_path, monkeypatch) -> None:
    monkeypatch.setenv("CATALOG_CACHE_DIR", str(tmp_path))
    connector = GitHubConnector()

    manifest = await connector.get_manifest("http://connector")

    assert manifest.mcp_enabled is True
    skill_ids = [skill.id for skill in manifest.skills]
    for expected in (
        "github-investigate-issue",
        "github-summarize-pr",
        "github-comment",
        "github-manage-issues",
    ):
        assert expected in skill_ids


async def test_authenticated_bootstrap_discovers_and_persists_catalog(
    tmp_path, monkeypatch
) -> None:
    cache_path = tmp_path / "github.mcp-catalog.json"
    monkeypatch.setenv("CATALOG_CACHE_DIR", str(tmp_path))
    connector = GitHubConnector()
    fake = FakeMcpAdapter()
    connector._mcp_adapter = fake  # noqa: SLF001 - deliberate test seam
    connector._mcp_catalog_cache_loaded = True  # noqa: SLF001

    await connector.bootstrap_mcp({"credentials": {"token": "ghp_boot"}})

    assert fake.discovered is True
    assert fake.env_seen == {"GITHUB_PERSONAL_ACCESS_TOKEN": "ghp_boot"}
    assert (
        json.loads(cache_path.read_text())["catalog"]["actions"][0]["name"]
        == "issue_write"
    )


async def test_bootstrap_skipped_without_token(tmp_path, monkeypatch) -> None:
    monkeypatch.setenv("CATALOG_CACHE_DIR", str(tmp_path))
    connector = GitHubConnector()
    fake = FakeMcpAdapter()
    connector._mcp_adapter = fake  # noqa: SLF001 - deliberate test seam
    connector._mcp_catalog_cache_loaded = True  # noqa: SLF001

    await connector.bootstrap_mcp({"credentials": {}})

    assert fake.discovered is False


async def test_oauth_credential_ready_bootstraps_with_forwarded_credentials(
    tmp_path, monkeypatch
) -> None:
    cache_path = tmp_path / "github.mcp-catalog.json"
    monkeypatch.setenv("CATALOG_CACHE_DIR", str(tmp_path))
    connector = GitHubConnector()
    fake = FakeMcpAdapter()
    connector._mcp_adapter = fake  # noqa: SLF001 - deliberate test seam
    connector._mcp_catalog_cache_loaded = True  # noqa: SLF001

    changed = await connector.oauth_credential_ready(
        OAuthCredentialReadyRequest(
            source_id="src_github",
            user_id="user_1",
            provider="github",
            flow="user_write",
            credentials={
                "credentials": {"access_token": "gho_ready"},
                "config": {"granted_scopes": ["repo", "read:user", "user:email"]},
                "auth_type": "oauth",
                "principal_email": "user@example.com",
            },
        )
    )

    assert changed is True
    assert fake.discovered is True
    assert fake.env_seen == {"GITHUB_PERSONAL_ACCESS_TOKEN": "gho_ready"}
    assert cache_path.exists()


async def test_oauth_credential_ready_ignores_payload_without_token(
    tmp_path, monkeypatch
) -> None:
    monkeypatch.setenv("CATALOG_CACHE_DIR", str(tmp_path))
    connector = GitHubConnector()
    fake = FakeMcpAdapter()
    connector._mcp_adapter = fake  # noqa: SLF001 - deliberate test seam
    connector._mcp_catalog_cache_loaded = True  # noqa: SLF001

    changed = await connector.oauth_credential_ready(
        OAuthCredentialReadyRequest(
            source_id="src_github",
            provider="github",
            flow="user_write",
            credentials={"credentials": {}},
        )
    )

    assert changed is False
    assert fake.discovered is False


# ---------------------------------------------------------------------------
# MCP dispatch
# ---------------------------------------------------------------------------


async def test_execute_action_delegates_to_mcp_with_resolved_token() -> None:
    connector = GitHubConnector()
    fake = FakeMcpAdapter()
    connector._mcp_adapter = fake  # noqa: SLF001 - deliberate test seam
    connector._mcp_catalog_cache_loaded = True  # noqa: SLF001

    response = await connector.execute_action(
        "issue_write",
        {"owner": "octo", "repo": "api", "title": "Hi", "method": "create"},
        {"credentials": {"access_token": "gho_user"}},
    )

    assert response.status_code == 200
    assert json.loads(response.body)["status"] == "success"
    assert fake.env_seen == {"GITHUB_PERSONAL_ACCESS_TOKEN": "gho_user"}
    assert fake.executed is not None


# ---------------------------------------------------------------------------
# Repository scoping (validate_mcp_action)
# ---------------------------------------------------------------------------


def test_scoping_allows_when_no_scope_configured() -> None:
    connector = GitHubConnector()
    source = make_source({})

    connector.validate_mcp_action(
        "issue_write", {"owner": "anyone", "repo": "anything"}, source
    )


def test_scoping_allows_in_scope_repo() -> None:
    connector = GitHubConnector()
    source = make_source({"repos": ["octo/api", "octo/web"]})

    connector.validate_mcp_action(
        "issue_write", {"owner": "Octo", "repo": "API"}, source
    )


def test_scoping_rejects_out_of_scope_repo() -> None:
    connector = GitHubConnector()
    source = make_source({"repos": ["octo/api"]})

    with pytest.raises(ValueError, match="outside the source"):
        connector.validate_mcp_action(
            "issue_write", {"owner": "octo", "repo": "private-repo"}, source
        )


def test_scoping_rejects_out_of_scope_owner() -> None:
    connector = GitHubConnector()
    source = make_source({"orgs": ["acme"]})

    with pytest.raises(ValueError, match="outside the source"):
        connector.validate_mcp_action(
            "issue_write", {"owner": "competitor", "repo": "api"}, source
        )


def test_scoping_allows_any_repo_under_configured_org() -> None:
    connector = GitHubConnector()
    source = make_source({"orgs": ["acme"]})

    connector.validate_mcp_action(
        "issue_write", {"owner": "acme", "repo": "whatever"}, source
    )


def test_scoping_allows_any_repo_under_configured_user() -> None:
    connector = GitHubConnector()
    source = make_source({"users": ["octo"]})

    connector.validate_mcp_action("issue_read", {"owner": "octo", "repo": "x"}, source)


def test_scoping_ignores_calls_without_owner() -> None:
    connector = GitHubConnector()
    source = make_source({"repos": ["octo/api"]})

    # Context-style tools (e.g. get_me) carry no owner; nothing to pin.
    connector.validate_mcp_action("get_me", {}, source)

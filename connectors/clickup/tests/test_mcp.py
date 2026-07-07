"""Unit tests for ClickUp remote MCP integration."""

import json
from typing import Any

import pytest

from clickup_connector import ClickUpConnector
from clickup_connector.config import CLICKUP_MCP_URL, CLICKUP_OAUTH_RESOURCE
from omni_connector import (
    ActionDefinition,
    ActionResponse,
    HttpMcpServer,
    OAuthCredentialReadyRequest,
)


class FakeMcpAdapter:
    def __init__(self) -> None:
        self.headers_seen: dict[str, str] | None = None
        self.discovered = False

    async def discover(self, **auth: Any) -> None:
        self.headers_seen = auth.get("headers")
        self.discovered = True

    def _export_catalog(self) -> dict[str, Any]:
        return {
            "actions": [
                ActionDefinition(
                    name="create_task",
                    description="Create a ClickUp task",
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
        self.headers_seen = auth.get("headers")
        return [
            ActionDefinition(
                name="create_task",
                description="Create a ClickUp task",
                input_schema={"type": "object", "properties": {}},
                mode="write",
            )
        ]

    async def execute_tool(
        self, name: str, arguments: dict[str, Any], **auth: Any
    ) -> ActionResponse:
        self.headers_seen = auth.get("headers")
        return ActionResponse.success({"name": name, "arguments": arguments})

    async def get_resource_definitions(self, **auth: Any) -> list[Any]:
        return []

    async def get_prompt_definitions(self, **auth: Any) -> list[Any]:
        return []


def test_mcp_server_points_at_clickup_remote_http() -> None:
    connector = ClickUpConnector()
    server = connector.mcp_server

    assert isinstance(server, HttpMcpServer)
    assert server.url == CLICKUP_MCP_URL


@pytest.mark.parametrize(
    "credentials",
    [
        {"access_token": "tok_123"},
        {"credentials": {"access_token": "tok_123"}},
        {
            "credentials": {"access_token": "tok_123"},
            "config": {},
            "auth_type": "oauth",
        },
    ],
)
def test_prepare_mcp_headers_uses_access_token(credentials: dict[str, Any]) -> None:
    connector = ClickUpConnector()

    assert connector.prepare_mcp_headers(credentials) == {
        "Authorization": "Bearer tok_123"
    }


def test_prepare_mcp_headers_rejects_missing_access_token() -> None:
    connector = ClickUpConnector()

    with pytest.raises(ValueError, match="access_token"):
        connector.prepare_mcp_headers({"credentials": {"token": "pk_legacy"}})


def test_oauth_config_declares_clickup_mcp_public_pkce() -> None:
    connector = ClickUpConnector()
    oauth = connector.oauth_config()

    assert oauth is not None
    assert oauth.provider == "clickup"
    assert oauth.scopes["clickup"].read == ["read"]
    assert oauth.scopes["clickup"].write == ["read", "write"]
    assert oauth.extra_auth_params["resource"] == CLICKUP_OAUTH_RESOURCE
    assert oauth.registration_endpoint == "https://mcp.clickup.com/oauth/register"
    assert oauth.token_endpoint_auth_method == "none"


async def test_manifest_does_not_discover_without_authenticated_bootstrap(
    tmp_path, monkeypatch
) -> None:
    monkeypatch.setenv("CATALOG_CACHE_DIR", str(tmp_path))
    connector = ClickUpConnector()

    manifest = await connector.get_manifest("http://connector")

    assert manifest.mcp_enabled is True
    assert [action.name for action in manifest.actions] == ["search_spaces"]
    assert manifest.resources == []
    assert manifest.prompts == []


async def test_authenticated_bootstrap_discovers_and_persists_catalog(
    tmp_path, monkeypatch
) -> None:
    cache_path = tmp_path / "clickup.mcp-catalog.json"
    monkeypatch.setenv("CATALOG_CACHE_DIR", str(tmp_path))
    connector = ClickUpConnector()
    fake = FakeMcpAdapter()
    connector._mcp_adapter = fake  # noqa: SLF001 - deliberate test seam
    connector._mcp_catalog_cache_loaded = True  # noqa: SLF001

    await connector.bootstrap_mcp({"credentials": {"access_token": "tok_boot"}})

    assert fake.discovered is True
    assert fake.headers_seen == {"Authorization": "Bearer tok_boot"}
    assert (
        json.loads(cache_path.read_text())["catalog"]["actions"][0]["name"]
        == "create_task"
    )


async def test_oauth_credential_ready_bootstraps_with_forwarded_credentials(
    tmp_path, monkeypatch
) -> None:
    cache_path = tmp_path / "clickup.mcp-catalog.json"
    monkeypatch.setenv("CATALOG_CACHE_DIR", str(tmp_path))
    connector = ClickUpConnector()
    fake = FakeMcpAdapter()
    connector._mcp_adapter = fake  # noqa: SLF001 - deliberate test seam
    connector._mcp_catalog_cache_loaded = True  # noqa: SLF001

    changed = await connector.oauth_credential_ready(
        OAuthCredentialReadyRequest(
            source_id="src_clickup",
            user_id="user_1",
            provider="clickup",
            flow="user_write",
            credentials={
                "credentials": {"access_token": "tok_ready"},
                "config": {"granted_scopes": ["read", "write"]},
                "auth_type": "oauth",
                "principal_email": "user@example.com",
            },
        )
    )

    assert changed is True
    assert fake.discovered is True
    assert fake.headers_seen == {"Authorization": "Bearer tok_ready"}
    assert (
        json.loads(cache_path.read_text())["catalog"]["actions"][0]["name"]
        == "create_task"
    )


async def test_execute_action_delegates_to_mcp_with_bearer_header(monkeypatch) -> None:
    connector = ClickUpConnector()
    fake = FakeMcpAdapter()
    connector._mcp_adapter = fake  # noqa: SLF001 - deliberate test seam
    connector._mcp_catalog_cache_loaded = True  # noqa: SLF001

    response = await connector.execute_action(
        "create_task",
        {"name": "Ship it"},
        {"credentials": {"access_token": "tok_action"}},
    )

    assert response.status_code == 200
    assert json.loads(response.body)["status"] == "success"
    assert fake.headers_seen == {"Authorization": "Bearer tok_action"}


async def test_search_spaces_action_returns_spaces(monkeypatch) -> None:
    class FakeClickUpClient:
        def __init__(self, token: str, base_url: str | None = None) -> None:
            assert token == "pk_test"
            assert base_url == "http://clickup.test"

        async def get_workspaces(self) -> list[dict[str, Any]]:
            return [{"id": "team_1", "name": "Workspace"}]

        async def list_spaces(self, team_id: str) -> list[dict[str, Any]]:
            assert team_id == "team_1"
            return [
                {"id": "space_1", "name": "Engineering"},
                {"id": "space_2", "name": "Marketing"},
            ]

        async def close(self) -> None:
            pass

    monkeypatch.setattr(
        "clickup_connector.connector.ClickUpClient",
        FakeClickUpClient,
    )

    connector = ClickUpConnector()
    response = await connector.execute_action(
        "search_spaces",
        {"api_url": "http://clickup.test", "query": "eng"},
        {"credentials": {"token": "pk_test"}},
    )

    body = json.loads(response.body)
    assert response.status_code == 200
    assert body["status"] == "success"
    assert body["result"]["spaces"] == [
        {
            "id": "space_1",
            "name": "Engineering",
            "workspace_id": "team_1",
            "workspace_name": "Workspace",
        }
    ]

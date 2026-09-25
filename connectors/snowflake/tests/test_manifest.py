from datetime import UTC, datetime

import pytest
from fastapi.testclient import TestClient
from omni_connector import (
    ActionDefinition,
    ActionResponse,
    ManifestSourceContext,
    McpPromptDefinition,
    SdkConfig,
    Source,
)
from omni_connector.server import create_app

from snowflake_connector import connector as connector_module
from snowflake_connector.connector import SnowflakeConnector


class FakeAdapter:
    catalogs = {
        "https://one.snowflakecomputing.com/api/v2/databases/D/schemas/S/mcp-servers/M": [
            ActionDefinition(
                name="inspect",
                description="One inspect",
                input_schema={"type": "object"},
                mode="read",
                origin="mcp",
            )
        ],
        "https://two.snowflakecomputing.com/api/v2/databases/D/schemas/S/mcp-servers/M": [
            ActionDefinition(
                name="inspect",
                description="Two inspect",
                input_schema={"type": "object"},
                mode="read",
                origin="mcp",
            ),
            ActionDefinition(
                name="query",
                description="Two query",
                input_schema={"type": "object"},
                mode="write",
                origin="mcp",
            ),
        ],
    }
    calls: list[tuple[str, dict[str, str]]] = []
    prompt_calls: list[tuple[str, object, dict[str, object]]] = []
    action_calls: list[tuple[str, dict[str, object], dict[str, object]]] = []

    def __init__(self, server) -> None:
        self.endpoint = server.url

    async def discover(self, *, headers):
        self.calls.append((self.endpoint, headers))

    async def get_action_definitions(self):
        return list(self.catalogs[self.endpoint])

    async def execute_tool(self, name, arguments, **kwargs):
        self.action_calls.append((name, arguments, kwargs))
        return ActionResponse.success({"ok": True})

    async def get_resource_definitions(self):
        return []

    async def get_prompt_definitions(self):
        return [
            McpPromptDefinition(
                name="provider_prompt",
                description="Provider prompt",
            )
        ]

    async def get_prompt(self, name, arguments, **kwargs):
        self.prompt_calls.append((name, arguments, kwargs))
        return {"messages": [{"role": "user", "content": {"text": "prompt result"}}]}


@pytest.fixture
def source_context():
    def build(source_id: str, account: str, *, read_only: bool = False):
        endpoint = f"https://{account}.snowflakecomputing.com/api/v2/databases/D/schemas/S/mcp-servers/M"
        return ManifestSourceContext(
            id=source_id,
            source_type="snowflake",
            config={
                "account_url": f"https://{account}.snowflakecomputing.com",
                "warehouse": "W",
                "role": "R",
                "databases": ["D"],
                "mcp_enabled": True,
                "write_tools_enabled": True,
                "read_only": read_only,
                "mcp_endpoint_url": endpoint,
            },
            updated_at=datetime(2026, 6, 23, 10, tzinfo=UTC),
        )

    return build


@pytest.mark.asyncio
async def test_get_manifest_groups_each_discovery_by_exact_source(monkeypatch, source_context):
    FakeAdapter.calls.clear()
    FakeAdapter.prompt_calls.clear()
    monkeypatch.setattr(connector_module, "McpAdapter", FakeAdapter)
    connector = SnowflakeConnector()

    first = await connector.get_manifest(
        "http://snowflake:8000",
        source_context=source_context("one", "one"),
        credentials={"access_token": "token-one"},
    )
    second = await connector.get_manifest(
        "http://snowflake:8000",
        source_context=source_context("two", "two"),
        credentials={"access_token": "token-two"},
    )

    assert first.actions == []
    assert first.mcp_catalog_loaded is True
    assert [group.source_id for group in first.source_capabilities] == ["one"]
    assert [action.name for action in first.source_capabilities[0].actions] == ["inspect"]
    assert [group.source_id for group in second.source_capabilities] == ["two"]
    assert {action.name for action in second.source_capabilities[0].actions} == {
        "inspect",
        "query",
    }
    assert FakeAdapter.calls == [
        (FakeAdapter.calls[0][0], {"Authorization": "Bearer token-one"}),
        (FakeAdapter.calls[1][0], {"Authorization": "Bearer token-two"}),
    ]


@pytest.mark.asyncio
async def test_unchanged_source_manifest_uses_cache_and_no_cache_forces_refresh(
    monkeypatch, source_context
):
    FakeAdapter.calls.clear()
    monkeypatch.setattr(connector_module, "McpAdapter", FakeAdapter)
    connector = SnowflakeConnector()
    context = source_context("one", "one")
    first = await connector.get_manifest(
        "http://snowflake:8000",
        source_context=context,
        credentials={"access_token": "token-one"},
    )
    second = await connector.get_manifest(
        "http://snowflake:8000",
        source_context=context,
        credentials={"access_token": "token-two"},
    )
    refreshed = await connector.get_manifest(
        "http://snowflake:8000",
        source_context=context,
        credentials={"access_token": "token-two"},
        force_refresh=True,
    )

    assert first.source_capabilities == second.source_capabilities == refreshed.source_capabilities
    assert [headers for _endpoint, headers in FakeAdapter.calls] == [
        {"Authorization": "Bearer token-one"},
        {"Authorization": "Bearer token-two"},
    ]


@pytest.mark.asyncio
async def test_no_source_manifest_has_no_fake_mcp_adapter(monkeypatch, source_context):
    connector = SnowflakeConnector()
    assert connector.mcp_server is None
    assert connector.mcp_adapter is None
    await connector.bootstrap_mcp({"access_token": "token"})
    await connector.get_manifest("http://snowflake:8000")
    assert connector.mcp_adapter is None
    assert connector._prepare_mcp_auth({"access_token": "token"}) == {
        "headers": {"Authorization": "Bearer token"}
    }


@pytest.mark.asyncio
async def test_missing_discovery_credential_does_not_advertise_source(
    monkeypatch, source_context
):
    monkeypatch.setattr(connector_module, "McpAdapter", FakeAdapter)
    connector = SnowflakeConnector()

    manifest = await connector.get_manifest(
        "http://snowflake:8000",
        source_context=source_context("one", "one"),
        credentials=None,
    )

    assert manifest.source_capabilities == []
    assert manifest.actions == []


@pytest.mark.asyncio
async def test_source_scoped_skill_uses_source_adapter_and_provider_prompt(
    monkeypatch, source_context
):
    FakeAdapter.calls.clear()
    FakeAdapter.prompt_calls.clear()
    FakeAdapter.action_calls.clear()
    monkeypatch.setattr(connector_module, "McpAdapter", FakeAdapter)
    connector = SnowflakeConnector()
    context = source_context("one", "one")
    await connector.get_manifest(
        "http://snowflake:8000",
        source_context=context,
        credentials={"access_token": "token-one"},
    )
    source = Source(
        id="one",
        name="One",
        source_type="snowflake",
        config=context.config,
        is_active=True,
        is_deleted=False,
        scope="org",
        created_at=context.updated_at,
        updated_at=context.updated_at,
        created_by="test",
    )
    app = create_app(
        connector,
        config=SdkConfig(
            connector_manager_url="http://manager",
            connector_host_name="snowflake",
        ),
    )

    response = TestClient(app).post(
        "/skill",
        json={
            "skill_id": "mcp:one:provider_prompt",
            "source": source.model_dump(mode="json"),
            "credentials": {"access_token": "token-one"},
        },
    )

    assert response.status_code == 200
    assert response.json()["content"] == "prompt result"
    assert FakeAdapter.prompt_calls == [
        ("provider_prompt", None, {"headers": {"Authorization": "Bearer token-one"}})
    ]

    action_response = TestClient(app).post(
        "/action",
        json={
            "action": "inspect",
            "params": {},
            "source": source.model_dump(mode="json"),
            "credentials": {"access_token": "token-one"},
        },
    )
    assert action_response.status_code == 200
    assert FakeAdapter.action_calls[-1] == (
        "inspect",
        {},
        {"headers": {"Authorization": "Bearer token-one"}},
    )


@pytest.mark.asyncio
async def test_read_only_source_omits_write_capabilities(monkeypatch, source_context):
    monkeypatch.setattr(connector_module, "McpAdapter", FakeAdapter)
    connector = SnowflakeConnector()

    manifest = await connector.get_manifest(
        "http://snowflake:8000",
        source_context=source_context("two", "two", read_only=True),
        credentials={"access_token": "token"},
    )

    assert [action.name for action in manifest.source_capabilities[0].actions] == [
        "inspect"
    ]

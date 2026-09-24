from datetime import UTC, datetime

import pytest

from omni_connector import ActionDefinition, ManifestSourceContext

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

    def __init__(self, server) -> None:
        self.endpoint = server.url

    async def discover(self, *, headers):
        self.calls.append((self.endpoint, headers))

    async def get_action_definitions(self):
        return list(self.catalogs[self.endpoint])

    async def get_resource_definitions(self):
        return []

    async def get_prompt_definitions(self):
        return []


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

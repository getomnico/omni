import pytest
from omni_connector import ActionDefinition, ConnectorManifestSource

from snowflake_connector.connector import SnowflakeConnector


@pytest.mark.asyncio
async def test_multiple_sources_are_one_union_manifest() -> None:
    connector = SnowflakeConnector()
    connector._source_catalogs["one"] = (
        [ActionDefinition(name="inspect", description="Inspect", mode="read", origin="mcp")],
        [],
        [],
    )
    connector._source_catalogs["two"] = (
        [ActionDefinition(name="query", description="Query", mode="write", origin="mcp")],
        [],
        [],
    )
    sources = [
        ConnectorManifestSource(
            id="one",
            source_type="snowflake",
            scope="org",
            config={
                "account_url": "https://one.snowflakecomputing.com",
                "warehouse": "W",
                "role": "R",
                "databases": ["D"],
                "mcp_enabled": True,
                "write_tools_enabled": True,
                "mcp_endpoint_url": "https://one.snowflakecomputing.com/api/v2/databases/D/schemas/S/mcp-servers/M",
            },
            updated_at="2026-06-23T10:00:00Z",
        ),
        ConnectorManifestSource(
            id="two",
            source_type="snowflake",
            scope="org",
            config={
                "account_url": "https://two.snowflakecomputing.com",
                "warehouse": "W",
                "role": "R",
                "databases": ["D"],
                "mcp_enabled": True,
                "write_tools_enabled": True,
                "mcp_endpoint_url": "https://two.snowflakecomputing.com/api/v2/databases/D/schemas/S/mcp-servers/M",
            },
            updated_at="2026-06-23T10:00:00Z",
        ),
    ]
    manifest = await connector.build_manifest_for_sources(sources, None, "http://snowflake:8000")
    assert manifest.mcp_catalog_loaded
    assert {action.name for action in manifest.actions} == {"inspect", "query"}
    assert manifest.connector_id == "snowflake"


@pytest.mark.asyncio
async def test_conflicting_tool_names_are_omitted() -> None:
    connector = SnowflakeConnector()
    connector._source_catalogs["one"] = (
        [ActionDefinition(name="same", description="A", mode="read", origin="mcp")],
        [],
        [],
    )
    connector._source_catalogs["two"] = (
        [ActionDefinition(name="same", description="B", mode="read", origin="mcp")],
        [],
        [],
    )
    source = ConnectorManifestSource(
        id="one",
        source_type="snowflake",
        scope="org",
        config={
            "account_url": "https://one.snowflakecomputing.com",
            "warehouse": "W",
            "role": "R",
            "databases": ["D"],
            "mcp_enabled": True,
            "mcp_endpoint_url": "https://one.snowflakecomputing.com/api/v2/databases/D/schemas/S/mcp-servers/M",
        },
        updated_at="2026-06-23T10:00:00Z",
    )
    second = source.model_copy(
        update={
            "id": "two",
            "config": {
                **source.config,
                "account_url": "https://two.snowflakecomputing.com",
                "mcp_endpoint_url": "https://two.snowflakecomputing.com/api/v2/databases/D/schemas/S/mcp-servers/M",
            },
        }
    )
    manifest = await connector.build_manifest_for_sources(
        [source, second], None, "http://snowflake:8000"
    )
    assert manifest.actions == []

from datetime import UTC, datetime

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
    updated_at = datetime(2026, 6, 23, 10, tzinfo=UTC)
    connector._source_catalog_versions["one"] = (
        updated_at,
        "https://one.snowflakecomputing.com/api/v2/databases/D/schemas/S/mcp-servers/M",
        True,
        True,
    )
    connector._source_catalog_versions["two"] = (
        updated_at,
        "https://two.snowflakecomputing.com/api/v2/databases/D/schemas/S/mcp-servers/M",
        True,
        False,
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
            updated_at=datetime(2026, 6, 23, 10, tzinfo=UTC),
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
                "read_only": False,
                "mcp_endpoint_url": "https://two.snowflakecomputing.com/api/v2/databases/D/schemas/S/mcp-servers/M",
            },
            updated_at=datetime(2026, 6, 23, 10, tzinfo=UTC),
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
    updated_at = datetime(2026, 6, 23, 10, tzinfo=UTC)
    connector._source_catalog_versions["one"] = (
        updated_at,
        "https://one.snowflakecomputing.com/api/v2/databases/D/schemas/S/mcp-servers/M",
        False,
        True,
    )
    connector._source_catalog_versions["two"] = (
        updated_at,
        "https://two.snowflakecomputing.com/api/v2/databases/D/schemas/S/mcp-servers/M",
        False,
        True,
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
        updated_at=datetime(2026, 6, 23, 10, tzinfo=UTC),
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


@pytest.mark.asyncio
async def test_catalog_is_invalidated_when_source_policy_or_timestamp_changes() -> None:
    connector = SnowflakeConnector()
    endpoint = "https://one.snowflakecomputing.com/api/v2/databases/D/schemas/S/mcp-servers/M"
    original_time = datetime(2026, 6, 23, 10, tzinfo=UTC)
    connector._source_catalogs["one"] = (
        [ActionDefinition(name="inspect", description="Inspect", mode="read", origin="mcp")],
        [],
        [],
    )
    connector._source_catalog_versions["one"] = (original_time, endpoint, False, True)
    source = ConnectorManifestSource(
        id="one",
        source_type="snowflake",
        scope="org",
        config={
            "account_url": "https://one.snowflakecomputing.com",
            "mcp_enabled": True,
            "mcp_endpoint_url": endpoint,
        },
        updated_at=original_time,
    )
    initial = await connector.build_manifest_for_sources([source], None, "http://snowflake:8000")
    assert initial.mcp_catalog_loaded

    changed = source.model_copy(
        update={
            "updated_at": datetime(2026, 6, 23, 11, tzinfo=UTC),
            "config": {**source.config, "read_only": False},
        }
    )
    refreshed = await connector.build_manifest_for_sources(
        [changed], initial, "http://snowflake:8000"
    )
    assert refreshed.actions == []
    assert not refreshed.mcp_catalog_loaded

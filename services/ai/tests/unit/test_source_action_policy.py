from __future__ import annotations

import pytest
import respx
from httpx import Response

from db.models import Source
from tools.connector_handler import ConnectorToolHandler, action_is_available_for_source
from tools.meta_handler import MetaToolHandler
from tools.registry import ToolContext

pytestmark = pytest.mark.unit


def _source(
    source_id: str,
    config: dict[str, object],
    *,
    scope: str = "org",
    created_by: str | None = None,
) -> Source:
    return Source(
        id=source_id,
        source_type="crm",
        name=source_id,
        is_active=True,
        is_deleted=False,
        config=config,
        scope=scope,
        created_by=created_by,
    )


@pytest.mark.asyncio
async def test_action_origins_are_filtered_per_source_and_default_allows_all():
    restricted_source = _source(
        "restricted",
        {"allowed_action_origins": ["mcp"]},
    )
    unrestricted_source = _source("unrestricted", {})
    handler = ConnectorToolHandler(
        connector_manager_url="http://cm.test",
        user_id="user-1",
        prefetched_sources=[restricted_source, unrestricted_source],
    )
    manifest = {
        "source_type": "crm",
        "manifest": {
            "actions": [
                {"name": "native_lookup", "origin": "native", "mode": "read"},
                {"name": "mcp_lookup", "origin": "mcp", "mode": "read"},
            ],
            "search_operators": [
                {
                    "operator": "team",
                    "attribute_key": "team",
                    "value_type": "text",
                }
            ],
        },
    }

    with respx.mock(assert_all_called=True) as mock:
        connectors_route = mock.get("http://cm.test/connectors").mock(
            return_value=Response(200, json=[manifest])
        )
        await handler._ensure_initialized()

    by_source = {
        source_id: {
            (action.action_name, action.origin)
            for action in handler.actions.values()
            if action.source_id == source_id
        }
        for source_id in (restricted_source.id, unrestricted_source.id)
    }
    assert by_source[restricted_source.id] == {("mcp_lookup", "mcp")}
    assert by_source[unrestricted_source.id] == {
        ("native_lookup", "native"),
        ("mcp_lookup", "mcp"),
    }
    assert len(handler.search_operators) == 1
    assert handler.connector_catalog == [manifest]
    assert len(connectors_route.calls) == 1


@pytest.mark.asyncio
async def test_source_capabilities_are_bound_to_their_exact_source():
    first = _source("source-one", {})
    second = _source("source-two", {})
    handler = ConnectorToolHandler(
        connector_manager_url="http://cm.test",
        user_id="user-1",
        prefetched_sources=[first, second],
    )
    manifest = {
        "source_type": "crm",
        "healthy": True,
        "manifest": {
            "actions": [
                {"name": "legacy", "origin": "native", "mode": "read"},
                {"name": "shared", "origin": "native", "mode": "read"},
            ],
            "source_capabilities": [
                {
                    "source_id": "source-one",
                    "actions": [
                        {"name": "shared", "origin": "mcp", "mode": "read"},
                        {"name": "one_only", "origin": "mcp", "mode": "read"},
                    ],
                }
            ],
        },
    }

    with respx.mock:
        respx.get("http://cm.test/connectors").mock(
            return_value=Response(200, json=[manifest])
        )
        await handler._ensure_initialized()

    by_source = {
        source_id: {action.action_name for action in handler.actions.values() if action.source_id == source_id}
        for source_id in (first.id, second.id)
    }
    assert by_source[first.id] == {"legacy", "shared", "one_only"}
    assert by_source[second.id] == {"legacy", "shared"}
    assert next(
        action for action in handler.actions.values()
        if action.source_id == first.id and action.action_name == "shared"
    ).origin == "mcp"


def test_malformed_action_origin_policy_fails_closed():
    source = _source("malformed", {"allowed_action_origins": ["unknown"]})

    with pytest.raises(ValueError, match="allowed_action_origins"):
        action_is_available_for_source(source, "native")


def test_source_row_rejects_malformed_action_origin_policy():
    with pytest.raises(ValueError, match="allowed_action_origins"):
        Source.from_row(
            {
                "id": "malformed",
                "name": "Malformed",
                "source_type": "crm",
                "is_active": True,
                "is_deleted": False,
                "scope": "org",
                "created_by": "admin-1",
                "config": {"allowed_action_origins": "mcp"},
            }
        )


@pytest.mark.asyncio
async def test_explicitly_unhealthy_connector_hides_actions_and_search_operators():
    source = _source("org", {})
    handler = ConnectorToolHandler(
        connector_manager_url="http://cm.test",
        user_id="user-1",
        prefetched_sources=[source],
    )
    manifest = {
        "source_type": "crm",
        "healthy": False,
        "manifest": {
            "actions": [
                {"name": "list_records", "origin": "native", "mode": "read"}
            ],
            "search_operators": [
                {
                    "operator": "team",
                    "attribute_key": "team",
                    "value_type": "text",
                }
            ],
        },
    }

    with respx.mock:
        respx.get("http://cm.test/connectors").mock(
            return_value=Response(200, json=[manifest])
        )
        await handler._ensure_initialized()

    assert handler.actions == {}
    assert handler.search_operators == []


@pytest.mark.asyncio
async def test_connector_actions_and_toolsets_hide_foreign_personal_sources():
    sources = [
        _source("org", {}, scope="org", created_by="admin-1"),
        _source("own", {}, scope="user", created_by="user-1"),
        _source("foreign", {}, scope="user", created_by="user-2"),
    ]
    handler = ConnectorToolHandler(
        connector_manager_url="http://cm.test",
        user_id="user-1",
        prefetched_sources=sources,
    )
    manifest = {
        "source_type": "crm",
        "healthy": True,
        "manifest": {
            "actions": [
                {"name": "list_records", "origin": "native", "mode": "read"}
            ]
        },
    }

    with respx.mock:
        respx.get("http://cm.test/connectors").mock(
            return_value=Response(200, json=[manifest])
        )
        await handler._ensure_initialized()

    assert {action.source_id for action in handler.actions.values()} == {"org", "own"}
    assert {toolset["source_id"] for toolset in handler.list_toolsets()} == {
        "org",
        "own",
    }
    stale_foreign_tool = "crm__list_records__source_foreign"
    assert handler.filtered_tools({stale_foreign_tool}) == []
    assert (
        await handler.check_oauth_required(
            stale_foreign_tool, {}, ToolContext(chat_id="chat-1", user_id="user-1")
        )
        is None
    )

    meta = MetaToolHandler(handler, set(), lambda _: _noop())
    result = await meta._load_tool_set({"source_id": "foreign"})
    assert result.is_error
    assert "No connector toolset" in result.content[0]["text"]


async def _noop() -> None:
    return None

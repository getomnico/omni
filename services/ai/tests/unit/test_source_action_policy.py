from __future__ import annotations

import json

import pytest
import respx
from httpx import Response

from db.models import Source
from tools.connector_handler import (
    ConnectorAction,
    ConnectorToolHandler,
    action_is_available_for_source,
)
from tools.meta_handler import MetaToolHandler
from tools.omni_tool_result import OAuthRequiredPayload
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
async def test_no_sync_source_action_origins_are_generic_and_consistent():
    unrestricted = _source("no-sync-unrestricted", {"sync_enabled": False})
    restricted = _source(
        "no-sync-mcp-only",
        {"sync_enabled": False, "allowed_action_origins": ["mcp"]},
    )
    handler = ConnectorToolHandler(
        connector_manager_url="http://cm.test",
        user_id="user-1",
        prefetched_sources=[unrestricted, restricted],
    )
    manifest = {
        "source_type": "crm",
        "manifest": {
            "actions": [
                {"name": "run_soql_query", "origin": "native", "mode": "read"},
                {"name": "get_username", "origin": "native", "mode": "read"},
                {"name": "mcp_lookup", "origin": "mcp", "mode": "read"},
            ]
        },
    }

    with respx.mock:
        respx.get("http://cm.test/connectors").mock(
            return_value=Response(200, json=[manifest])
        )
        await handler._ensure_initialized()

    by_source = {
        source.id: {action.action_name for action in handler.actions.values() if action.source_id == source.id}
        for source in (unrestricted, restricted)
    }
    assert by_source[unrestricted.id] == {"run_soql_query", "get_username", "mcp_lookup"}
    assert by_source[restricted.id] == {"mcp_lookup"}


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


@pytest.mark.parametrize("flow", ["no-credential", "expired-credential"])
@pytest.mark.asyncio
async def test_salesforce_manager_412_becomes_reconnect_payload(flow: str):
    handler = ConnectorToolHandler(
        connector_manager_url="http://cm.test",
        user_id="actor-1",
    )
    handler._actions["salesforce__run_soql_query"] = ConnectorAction(
        source_id="source-1",
        source_type="salesforce",
        source_name="Salesforce",
        action_name="run_soql_query",
        description="Query Salesforce",
        input_schema={"type": "object"},
        mode="read",
    )
    manager_response = {
        "error": "needs_user_auth",
        "source_id": "source-1",
        "source_type": "salesforce",
        "provider": "salesforce",
        "oauth_start_url": "/api/oauth/start?source_id=source-1",
    }

    with respx.mock(assert_all_called=True) as mock:
        mock.post("http://cm.test/action").mock(
            return_value=Response(412, json=manager_response)
        )
        result = await handler.execute(
            "salesforce__run_soql_query",
            {},
            ToolContext(chat_id=f"chat-{flow}", user_id="actor-1"),
        )

    assert not result.is_error
    assert isinstance(result.oauth_required, OAuthRequiredPayload)
    assert result.oauth_required.source_id == "source-1"
    assert result.oauth_required.source_type == "salesforce"
    assert result.oauth_required.provider == "salesforce"
    assert result.oauth_required.oauth_start_url == "/api/oauth/start?source_id=source-1"


@pytest.mark.asyncio
async def test_large_salesforce_action_result_is_saved_to_chat_sandbox():
    source = _source("salesforce", {})
    handler = ConnectorToolHandler(
        connector_manager_url="http://cm.test",
        user_id="actor-1",
        sandbox_url="http://sandbox.test",
    )
    handler._actions["salesforce__run_soql_query"] = ConnectorAction(
        source_id=source.id,
        source_type="salesforce",
        source_name=source.name,
        action_name="run_soql_query",
        description="Query Salesforce",
        input_schema={"type": "object"},
        mode="read",
    )
    large_result = "SOQL query results:\n\n" + "x" * 50_000

    with respx.mock(assert_all_called=True) as mock:
        mock.post("http://cm.test/action").mock(
            return_value=Response(
                200,
                json={"status": "success", "result": {"content": large_result}},
            )
        )
        sandbox_route = mock.post("http://sandbox.test/files/write").mock(
            return_value=Response(200, json={"path": "run_soql_query_result.json"})
        )
        result = await handler.execute(
            "salesforce__run_soql_query",
            {},
            ToolContext(chat_id="chat-1", user_id="actor-1"),
        )

    assert not result.is_error
    assert sandbox_route.call_count == 1
    saved = sandbox_route.calls[0].request.read()
    payload = json.loads(saved)
    assert payload["chat_id"] == "chat-1"
    assert payload["path"] == "run_soql_query_result.json"
    assert json.loads(payload["content"])["content"] == large_result


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

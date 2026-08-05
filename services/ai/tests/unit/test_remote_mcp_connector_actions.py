from __future__ import annotations

import importlib

import pytest
import respx
from httpx import Response

from db.models import Source
from tools.connector_handler import (
    ConnectorToolHandler,
    fetch_active_sources_from_connector_manager,
)


def _source(source_id: str, integration_type: str) -> Source:
    return Source(
        id=source_id,
        name=f"{integration_type} docs",
        source_type="docs",
        integration_type=integration_type,
        is_active=True,
        is_deleted=False,
    )


@pytest.mark.asyncio
@respx.mock
async def test_actions_match_source_by_integration_type_and_source_type() -> None:
    respx.get("http://cm.test/connectors").mock(
        return_value=Response(
            200,
            json=[
                {
                    "source_type": "docs",
                    "healthy": True,
                    "manifest": {
                        "integration_type": "connector",
                        "display_name": "Native Docs",
                        "actions": [
                            {
                                "name": "search",
                                "description": "Native search",
                                "mode": "read",
                            }
                        ],
                    },
                },
                {
                    "source_type": "docs",
                    "healthy": True,
                    "manifest": {
                        "integration_type": "remote_mcp",
                        "display_name": "Remote Docs",
                        "actions": [
                            {
                                "name": "search",
                                "description": "Remote MCP search",
                                "mode": "read",
                            }
                        ],
                    },
                },
            ],
        )
    )
    handler = ConnectorToolHandler(
        connector_manager_url="http://cm.test",
        user_id="user-1",
        prefetched_sources=[
            _source("src-native", "connector"),
            _source("src-remote", "remote_mcp"),
        ],
    )

    await handler._ensure_initialized()

    assert len(handler.actions) == 2
    assert {action.source_id for action in handler.actions.values()} == {
        "src-native",
        "src-remote",
    }
    remote_action = next(
        action
        for action in handler.actions.values()
        if action.source_id == "src-remote"
    )
    assert remote_action.integration_type == "remote_mcp"
    assert remote_action.description == "Remote MCP search"


@pytest.mark.asyncio
@respx.mock
async def test_actions_fetch_active_sources_endpoint_including_remote_mcp_rows() -> None:
    respx.get("http://cm.test/connectors").mock(
        return_value=Response(
            200,
            json=[
                {
                    "source_type": "docs",
                    "healthy": True,
                    "manifest": {
                        "integration_type": "remote_mcp",
                        "display_name": "Remote Docs",
                        "actions": [
                            {
                                "name": "search",
                                "description": "Remote MCP search",
                                "mode": "read",
                            }
                        ],
                    },
                }
            ],
        )
    )
    active_sources = respx.get("http://cm.test/sources/active").mock(
        return_value=Response(
            200,
            json=[
                {
                    "source": {
                        "id": "src-remote",
                        "name": "Remote docs",
                        "source_type": "docs",
                        "integration_type": "remote_mcp",
                        "is_active": True,
                        "is_deleted": False,
                    },
                    "sync_runs": [],
                    "health": "healthy",
                }
            ],
        )
    )
    legacy_sources = respx.get("http://cm.test/sources").mock(
        return_value=Response(500, json={"error": "sync endpoint should not be used"})
    )

    handler = ConnectorToolHandler(
        connector_manager_url="http://cm.test",
        user_id="user-1",
    )

    await handler._ensure_initialized()

    assert active_sources.called
    assert not legacy_sources.called
    assert len(handler.actions) == 1
    assert next(iter(handler.actions.values())).source_id == "src-remote"


@pytest.mark.asyncio
@respx.mock
async def test_active_source_prefetch_helper_uses_active_endpoint_for_remote_mcp_rows() -> None:
    active_sources = respx.get("http://cm.test/sources/active").mock(
        return_value=Response(
            200,
            json=[
                {
                    "source": {
                        "id": "src-remote",
                        "name": "Remote docs",
                        "source_type": "docs",
                        "integration_type": "remote_mcp",
                        "is_active": True,
                        "is_deleted": False,
                    },
                    "sync_runs": [],
                    "health": "healthy",
                }
            ],
        )
    )
    legacy_sources = respx.get("http://cm.test/sources").mock(
        return_value=Response(500, json={"error": "sync-only endpoint should not be used"})
    )

    sources = await fetch_active_sources_from_connector_manager("http://cm.test")

    assert active_sources.called
    assert not legacy_sources.called
    assert [source.id for source in sources] == ["src-remote"]
    assert sources[0].integration_type == "remote_mcp"


@pytest.mark.asyncio
@respx.mock
async def test_chat_and_agent_prefetch_helpers_use_active_sources_endpoint(monkeypatch) -> None:
    chat_module = importlib.import_module("routers.chat")
    executor_module = importlib.import_module("agents.executor")
    monkeypatch.setattr(chat_module, "CONNECTOR_MANAGER_URL", "http://cm.test")
    monkeypatch.setattr(executor_module, "CONNECTOR_MANAGER_URL", "http://cm.test")

    active_sources = respx.get("http://cm.test/sources/active").mock(
        return_value=Response(
            200,
            json=[
                {
                    "source": {
                        "id": "src-remote",
                        "name": "Remote docs",
                        "source_type": "docs",
                        "integration_type": "remote_mcp",
                        "is_active": True,
                        "is_deleted": False,
                    },
                    "sync_runs": [],
                    "health": "healthy",
                }
            ],
        )
    )
    legacy_sources = respx.get("http://cm.test/sources").mock(
        return_value=Response(500, json={"error": "sync-only endpoint should not be used"})
    )

    chat_sources = await chat_module._fetch_sources_from_connector_manager()
    agent_sources = await executor_module._fetch_sources()

    assert active_sources.call_count == 2
    assert not legacy_sources.called
    assert [source.id for source in chat_sources or []] == ["src-remote"]
    assert [source.id for source in agent_sources or []] == ["src-remote"]

"""Integration tests for lazy connector tool loading (issue #203).

Tool search/load calls are ordinary conversation turns. Loaded connector sources
are rebuilt from persisted tool_use/tool_result history rather than chat-session
columns.
"""

from __future__ import annotations

import pytest
from anthropic.types import MessageParam, ToolResultBlockParam, ToolUseBlockParam

from routers.chat import _loaded_toolsets_from_history
from tools.connector_handler import ConnectorAction, ConnectorToolHandler
from tools.meta_handler import MetaToolHandler
from tools.registry import ToolContext

pytestmark = pytest.mark.integration


def _action(
    source_id: str, source_type: str, action_name: str, description: str = ""
) -> ConnectorAction:
    return ConnectorAction(
        source_id=source_id,
        source_type=source_type,
        source_name=source_type,
        action_name=action_name,
        description=description or f"{action_name} on {source_type}",
        input_schema={"type": "object", "properties": {}},
        mode="write",
    )


def _connector_with(actions: list[ConnectorAction]) -> ConnectorToolHandler:
    handler = ConnectorToolHandler(
        connector_manager_url="http://unused",
        user_id="u1",
    )
    handler._build_tools(actions)
    handler._initialized = True
    return handler


@pytest.mark.asyncio
async def test_meta_handler_records_loaded_sources_in_tool_result():
    actions = [
        _action("src-gmail-1", "gmail", "send_email", "Send email via Gmail."),
        _action("src-gmail-1", "gmail", "list_threads", "List Gmail threads."),
        _action("src-slack-1", "slack", "post_message", "Post to a Slack channel."),
    ]
    connector_handler = _connector_with(actions)
    loaded: set[str] = set()

    async def on_load(newly: set[str]) -> None:
        return None

    meta = MetaToolHandler(connector_handler, loaded, on_load)
    result = await meta.execute(
        "tool_search",
        {"query": "email"},
        ToolContext(chat_id="chat-1", user_id="user-1"),
    )

    assert not result.is_error
    assert loaded == {"src-gmail-1"}
    text = result.content[0]["text"]
    assert "Loaded source ids: src-gmail-1" in text
    assert "src-slack-1" not in text


@pytest.mark.asyncio
async def test_loaded_toolsets_filters_subsequent_turn():
    actions = [
        _action("src-gmail-1", "gmail", "send_email"),
        _action("src-gmail-1", "gmail", "list_threads"),
        _action("src-slack-1", "slack", "post_message"),
        _action("src-drive-1", "google_drive", "create_doc"),
    ]
    connector_handler = _connector_with(actions)
    loaded: set[str] = set()

    async def on_load(newly: set[str]) -> None:
        return None

    meta = MetaToolHandler(connector_handler, loaded, on_load)

    assert connector_handler.filtered_tools(loaded) == []

    await meta.execute(
        "load_tool_set",
        {"source_type": "gmail"},
        ToolContext(chat_id="chat-1", user_id="user-1"),
    )

    turn2_names = {t["name"] for t in connector_handler.filtered_tools(loaded)}
    assert turn2_names == {"gmail__send_email", "gmail__list_threads"}


@pytest.mark.asyncio
async def test_chat_resume_restores_loaded_toolsets_from_history():
    actions = [
        _action("src-gmail-1", "gmail", "send_email"),
        _action("src-slack-1", "slack", "post_message"),
    ]
    connector_handler = _connector_with(actions)

    messages = [
        MessageParam(
            role="assistant",
            content=[
                ToolUseBlockParam(
                    type="tool_use",
                    id="toolu_1",
                    name="load_tool_set",
                    input={"source_type": "gmail"},
                )
            ],
        ),
        MessageParam(
            role="user",
            content=[
                ToolResultBlockParam(
                    type="tool_result",
                    tool_use_id="toolu_1",
                    content=[
                        {
                            "type": "text",
                            "text": "Loaded 1 tool(s).\nLoaded source ids: src-gmail-1",
                        }
                    ],
                    is_error=False,
                )
            ],
        ),
    ]

    loaded = _loaded_toolsets_from_history(messages, connector_handler)
    names = {t["name"] for t in connector_handler.filtered_tools(loaded)}
    assert names == {"gmail__send_email"}

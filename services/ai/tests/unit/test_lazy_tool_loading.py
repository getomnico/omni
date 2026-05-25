"""Unit tests for lazy connector tool loading state.

These cover conversation-history reconstruction. Real searcher/Postgres coverage
for capability search lives in services/searcher/tests/integration_tests.rs.
"""

from __future__ import annotations

import pytest
from anthropic.types import MessageParam, ToolResultBlockParam, ToolUseBlockParam

from routers.chat import _loaded_tools_from_history
from tools.connector_handler import ConnectorAction, ConnectorToolHandler


def _action(source_id: str, source_type: str, action_name: str) -> ConnectorAction:
    return ConnectorAction(
        source_id=source_id,
        source_type=source_type,
        source_name=source_type,
        action_name=action_name,
        description=f"{action_name} on {source_type}",
        input_schema={"type": "object", "properties": {}},
        mode="write",
    )


def _connector_with(actions: list[ConnectorAction]) -> ConnectorToolHandler:
    handler = ConnectorToolHandler(connector_manager_url="http://unused", user_id="u1")
    handler._build_tools(actions)
    handler._initialized = True
    return handler


@pytest.mark.asyncio
async def test_chat_resume_restores_loaded_tool_from_successful_tool_call():
    connector_handler = _connector_with(
        [
            _action("src-gmail-1", "gmail", "send_email"),
            _action("src-slack-1", "slack", "post_message"),
        ]
    )

    messages = [
        MessageParam(
            role="assistant",
            content=[
                ToolUseBlockParam(
                    type="tool_use",
                    id="toolu_1",
                    name="load_tool",
                    input={"tool_name": "gmail__send_email"},
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
                            "text": "Loaded tool: gmail__send_email",
                        }
                    ],
                    is_error=False,
                )
            ],
        ),
    ]

    loaded = _loaded_tools_from_history(messages, connector_handler)
    names = {t["name"] for t in connector_handler.filtered_tools(loaded)}
    assert names == {"gmail__send_email"}


@pytest.mark.asyncio
async def test_chat_resume_restores_loaded_tool_set_from_tool_call():
    connector_handler = _connector_with(
        [
            _action("src-gmail-1", "gmail", "send_email"),
            _action("src-gmail-1", "gmail", "list_threads"),
            _action("src-slack-1", "slack", "post_message"),
        ]
    )

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
                    content=[{"type": "text", "text": "Loaded tool set."}],
                    is_error=False,
                )
            ],
        ),
    ]

    loaded = _loaded_tools_from_history(messages, connector_handler)
    names = {t["name"] for t in connector_handler.filtered_tools(loaded)}
    assert names == {"gmail__send_email", "gmail__list_threads"}

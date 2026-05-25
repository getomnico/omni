"""Per-turn tool list builder.

The chat router and the agent executor both rebuild the LLM `tools=` list on
every turn so that newly-loaded connector tools become available immediately
after the meta-tools (`load_tool`, `load_tool_set`) fire. This module owns the
shared assembly logic.
"""

from __future__ import annotations

from anthropic.types import ToolParam

from tools.connector_handler import ConnectorToolHandler
from tools.registry import ToolHandler


def build_turn_tools(
    always_on_handlers: list[ToolHandler],
    connector_handler: ConnectorToolHandler | None,
    loaded_tool_names: set[str],
) -> list[ToolParam]:
    """Build the tool list for a single LLM turn.

    `always_on_handlers` covers built-ins (search/document/sandbox/...) and the
    meta-tools. Connector tools are filtered to only those explicitly loaded
    into the session.
    """
    tools: list[ToolParam] = []
    for handler in always_on_handlers:
        tools.extend(handler.get_tools())
    if connector_handler is not None:
        tools.extend(connector_handler.filtered_tools(loaded_tool_names))
    return tools

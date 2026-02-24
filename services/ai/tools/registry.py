"""ToolRegistry: central abstraction for tool collection and dispatch."""

from __future__ import annotations

import logging
from dataclasses import dataclass, field
from typing import Protocol, runtime_checkable

logger = logging.getLogger(__name__)


@dataclass
class ToolContext:
    """Shared context passed to all tool handlers during execution."""

    chat_id: str
    user_id: str
    user_email: str | None = None
    original_user_query: str | None = None


@dataclass
class ToolResult:
    """Standardized result from tool execution."""

    content: list  # ToolResultBlockParam content blocks
    is_error: bool = False


@runtime_checkable
class ToolHandler(Protocol):
    """Interface that all tool handlers implement."""

    def get_tools(self) -> list[dict]:
        """Return LLM tool definitions this handler provides."""
        ...

    def can_handle(self, tool_name: str) -> bool:
        """Whether this handler owns the given tool name."""
        ...

    async def execute(
        self, tool_name: str, tool_input: dict, context: ToolContext
    ) -> ToolResult:
        """Execute the tool call and return the result."""
        ...

    def requires_approval(self, tool_name: str) -> bool:
        """Whether this tool call needs user approval before execution."""
        ...


class ToolRegistry:
    """Collects tools from all handlers and dispatches tool calls."""

    def __init__(self) -> None:
        self._handlers: list[ToolHandler] = []

    def register(self, handler: ToolHandler) -> None:
        self._handlers.append(handler)

    def get_all_tools(self) -> list[dict]:
        """Collect tool definitions from all handlers for LLM injection."""
        tools: list[dict] = []
        for handler in self._handlers:
            tools.extend(handler.get_tools())
        return tools

    def requires_approval(self, tool_name: str) -> bool:
        """Check if a tool call needs user approval."""
        for handler in self._handlers:
            if handler.can_handle(tool_name):
                return handler.requires_approval(tool_name)
        return True  # Unknown tools require approval by default

    async def execute(
        self, tool_name: str, tool_input: dict, context: ToolContext
    ) -> ToolResult:
        """Dispatch tool call to the appropriate handler."""
        for handler in self._handlers:
            if handler.can_handle(tool_name):
                return await handler.execute(tool_name, tool_input, context)
        return ToolResult(
            content=[{"type": "text", "text": f"Unknown tool: {tool_name}"}],
            is_error=True,
        )

from __future__ import annotations

import logging
from typing import TYPE_CHECKING, Any

from .models import (
    ActionDefinition,
    ActionParameter,
    ActionResponse,
    McpPromptArgument,
    McpPromptDefinition,
    McpResourceDefinition,
)

if TYPE_CHECKING:
    from mcp.server.fastmcp import FastMCP

logger = logging.getLogger(__name__)


class McpAdapter:
    """Bridges an MCP FastMCP server into Omni's connector protocol.

    Introspects the MCP server's tools, resources, and prompts and exposes them
    as Omni ActionDefinitions, resource definitions, and prompt definitions.
    Tool/resource/prompt calls are dispatched directly to the FastMCP instance
    (in-process, no transport needed).
    """

    def __init__(self, mcp_server: FastMCP) -> None:
        self._server = mcp_server

    async def get_action_definitions(self) -> list[ActionDefinition]:
        tools = await self._server.list_tools()
        actions: list[ActionDefinition] = []
        for tool in tools:
            params: dict[str, ActionParameter] = {}
            input_schema = tool.inputSchema or {}
            properties = input_schema.get("properties", {})
            required_set = set(input_schema.get("required", []))
            for param_name, param_schema in properties.items():
                params[param_name] = ActionParameter(
                    type=param_schema.get("type", "string"),
                    required=param_name in required_set,
                    description=param_schema.get("description"),
                )
            actions.append(
                ActionDefinition(
                    name=tool.name,
                    description=tool.description or "",
                    parameters=params,
                )
            )
        return actions

    async def get_resource_definitions(self) -> list[McpResourceDefinition]:
        definitions: list[McpResourceDefinition] = []

        templates = await self._server.list_resource_templates()
        for tmpl in templates:
            definitions.append(
                McpResourceDefinition(
                    uri_template=str(tmpl.uriTemplate),
                    name=tmpl.name,
                    description=tmpl.description,
                    mime_type=tmpl.mimeType,
                )
            )

        resources = await self._server.list_resources()
        for res in resources:
            definitions.append(
                McpResourceDefinition(
                    uri_template=str(res.uri),
                    name=res.name,
                    description=res.description,
                    mime_type=res.mimeType,
                )
            )

        return definitions

    async def get_prompt_definitions(self) -> list[McpPromptDefinition]:
        prompts = await self._server.list_prompts()
        definitions: list[McpPromptDefinition] = []
        for prompt in prompts:
            args = [
                McpPromptArgument(
                    name=arg.name,
                    description=arg.description,
                    required=arg.required or False,
                )
                for arg in (prompt.arguments or [])
            ]
            definitions.append(
                McpPromptDefinition(
                    name=prompt.name,
                    description=prompt.description,
                    arguments=args,
                )
            )
        return definitions

    async def execute_tool(
        self, name: str, arguments: dict[str, Any]
    ) -> ActionResponse:
        try:
            result = await self._server.call_tool(name, arguments)
            # FastMCP.call_tool returns either:
            # - A tuple of (content_blocks, structured_content_dict)
            # - A sequence of content blocks
            # - A dict
            if isinstance(result, tuple):
                content_blocks, structured = result
                if isinstance(structured, dict):
                    return ActionResponse.success(structured)
                return ActionResponse.success(
                    {"content": self._content_blocks_to_text(content_blocks)}
                )
            if isinstance(result, dict):
                return ActionResponse.success(result)
            return ActionResponse.success(
                {"content": self._content_blocks_to_text(result)}
            )
        except Exception as e:
            logger.error("MCP tool %s failed: %s", name, e)
            return ActionResponse.failure(str(e))

    @staticmethod
    def _content_blocks_to_text(blocks: Any) -> str:
        text_parts: list[str] = []
        for block in blocks:
            if hasattr(block, "text"):
                text_parts.append(block.text)
            elif hasattr(block, "data"):
                text_parts.append(f"[binary: {getattr(block, 'mimeType', 'unknown')}]")
        return "\n".join(text_parts)

    async def read_resource(self, uri: str) -> dict[str, Any]:
        contents = await self._server.read_resource(uri)
        items: list[dict[str, Any]] = []
        for item in contents:
            entry: dict[str, Any] = {"uri": uri}
            if hasattr(item, "content") and item.content is not None:
                entry["text"] = item.content
            if hasattr(item, "mime_type") and item.mime_type:
                entry["mime_type"] = item.mime_type
            items.append(entry)
        return {"contents": items}

    async def get_prompt(
        self, name: str, arguments: dict[str, Any] | None = None
    ) -> dict[str, Any]:
        result = await self._server.get_prompt(name, arguments)
        messages: list[dict[str, Any]] = []
        for msg in result.messages:
            content_data: dict[str, Any]
            if hasattr(msg.content, "text"):
                content_data = {"type": "text", "text": msg.content.text}
            else:
                content_data = {"type": "unknown"}
            messages.append({"role": msg.role, "content": content_data})
        return {"description": result.description, "messages": messages}

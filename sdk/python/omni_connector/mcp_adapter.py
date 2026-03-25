from __future__ import annotations

import logging
from contextlib import AsyncExitStack
from typing import Any

from mcp.client.session import ClientSession
from mcp.client.stdio import StdioServerParameters, stdio_client

from .models import (
    ActionDefinition,
    ActionParameter,
    ActionResponse,
    McpPromptArgument,
    McpPromptDefinition,
    McpResourceDefinition,
)

logger = logging.getLogger(__name__)


class McpAdapter:
    """Bridges an external MCP server (subprocess) into Omni's connector protocol.

    Spawns the MCP server as a subprocess, communicates via stdio transport
    (newline-delimited JSON-RPC over stdin/stdout). Works with MCP servers
    written in any language.
    """

    def __init__(self, server_params: StdioServerParameters) -> None:
        self._base_params = server_params
        self._current_env: dict[str, str] | None = None
        self._session: ClientSession | None = None
        self._exit_stack: AsyncExitStack | None = None
        # Cache discovered definitions so manifest builds work without credentials
        self._cached_actions: list[ActionDefinition] | None = None
        self._cached_resources: list[McpResourceDefinition] | None = None
        self._cached_prompts: list[McpPromptDefinition] | None = None

    async def ensure_connected(
        self, env: dict[str, str] | None = None
    ) -> ClientSession:
        """Start or reuse the MCP subprocess.

        If *env* differs from the current subprocess environment, the old
        process is torn down and a new one is spawned with the updated env.
        """
        # Reuse session if env matches, or if no env was requested (don't
        # downgrade a credentialed session to a credential-less one).
        if self._session is not None and (env is None or env == self._current_env):
            return self._session

        await self.disconnect()

        merged_env = {**(self._base_params.env or {}), **(env or {})}
        params = StdioServerParameters(
            command=self._base_params.command,
            args=self._base_params.args,
            env=merged_env or None,
            cwd=self._base_params.cwd,
        )

        self._exit_stack = AsyncExitStack()
        try:
            read_stream, write_stream = await self._exit_stack.enter_async_context(
                stdio_client(params)
            )
            session = await self._exit_stack.enter_async_context(
                ClientSession(read_stream, write_stream)
            )
            await session.initialize()
            self._session = session
            self._current_env = env
            logger.info("MCP subprocess connected: %s", self._base_params.command)
            return session
        except Exception:
            await self._exit_stack.aclose()
            self._exit_stack = None
            raise

    async def disconnect(self) -> None:
        """Terminate the MCP subprocess if running."""
        if self._exit_stack is not None:
            try:
                await self._exit_stack.aclose()
            except Exception:
                logger.warning("Error closing MCP subprocess", exc_info=True)
            self._exit_stack = None
        self._session = None
        self._current_env = None

    async def get_action_definitions(
        self, env: dict[str, str] | None = None
    ) -> list[ActionDefinition]:
        # Without credentials (env), return cache or empty — don't spawn the server
        if env is None and self._session is None:
            return self._cached_actions or []
        try:
            session = await self.ensure_connected(env)
            result = await session.list_tools()
            actions: list[ActionDefinition] = []
            for tool in result.tools:
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
                is_read_only = bool(tool.annotations and tool.annotations.readOnlyHint)
                actions.append(
                    ActionDefinition(
                        name=tool.name,
                        description=tool.description or "",
                        parameters=params,
                        mode="read" if is_read_only else "write",
                    )
                )
            self._cached_actions = actions
            return actions
        except Exception:
            if self._cached_actions is not None:
                return self._cached_actions
            raise

    async def get_resource_definitions(
        self, env: dict[str, str] | None = None
    ) -> list[McpResourceDefinition]:
        if env is None and self._session is None:
            return self._cached_resources or []
        try:
            session = await self.ensure_connected(env)
            definitions: list[McpResourceDefinition] = []

            templates_result = await session.list_resource_templates()
            for tmpl in templates_result.resourceTemplates:
                definitions.append(
                    McpResourceDefinition(
                        uri_template=str(tmpl.uriTemplate),
                        name=tmpl.name,
                        description=tmpl.description,
                        mime_type=tmpl.mimeType,
                    )
                )

            resources_result = await session.list_resources()
            for res in resources_result.resources:
                definitions.append(
                    McpResourceDefinition(
                        uri_template=str(res.uri),
                        name=res.name,
                        description=res.description,
                        mime_type=res.mimeType,
                    )
                )

            self._cached_resources = definitions
            return definitions
        except Exception:
            if self._cached_resources is not None:
                return self._cached_resources
            raise

    async def get_prompt_definitions(
        self, env: dict[str, str] | None = None
    ) -> list[McpPromptDefinition]:
        if env is None and self._session is None:
            return self._cached_prompts or []
        try:
            session = await self.ensure_connected(env)
            result = await session.list_prompts()
            definitions: list[McpPromptDefinition] = []
            for prompt in result.prompts:
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
            self._cached_prompts = definitions
            return definitions
        except Exception:
            if self._cached_prompts is not None:
                return self._cached_prompts
            raise

    async def execute_tool(
        self, name: str, arguments: dict[str, Any], env: dict[str, str] | None = None
    ) -> ActionResponse:
        try:
            session = await self.ensure_connected(env)
            result = await session.call_tool(name, arguments)
            text_parts: list[str] = []
            for block in result.content:
                if hasattr(block, "text"):
                    text_parts.append(block.text)
                elif hasattr(block, "data"):
                    text_parts.append(
                        f"[binary: {getattr(block, 'mimeType', 'unknown')}]"
                    )
            content = "\n".join(text_parts)
            if result.isError:
                return ActionResponse.failure(content)
            return ActionResponse.success({"content": content})
        except Exception as e:
            logger.error("MCP tool %s failed: %s", name, e)
            return ActionResponse.failure(str(e))

    async def read_resource(
        self, uri: str, env: dict[str, str] | None = None
    ) -> dict[str, Any]:
        session = await self.ensure_connected(env)
        result = await session.read_resource(uri)
        items: list[dict[str, Any]] = []
        for item in result.contents:
            entry: dict[str, Any] = {"uri": str(item.uri)}
            if hasattr(item, "text") and item.text is not None:
                entry["text"] = item.text
            if hasattr(item, "mimeType") and item.mimeType:
                entry["mime_type"] = item.mimeType
            items.append(entry)
        return {"contents": items}

    async def get_prompt(
        self,
        name: str,
        arguments: dict[str, Any] | None = None,
        env: dict[str, str] | None = None,
    ) -> dict[str, Any]:
        session = await self.ensure_connected(env)
        result = await session.get_prompt(name, arguments)
        messages: list[dict[str, Any]] = []
        for msg in result.messages:
            content_data: dict[str, Any]
            if hasattr(msg.content, "text"):
                content_data = {"type": "text", "text": msg.content.text}
            else:
                content_data = {"type": "unknown"}
            messages.append({"role": msg.role, "content": content_data})
        return {"description": result.description, "messages": messages}

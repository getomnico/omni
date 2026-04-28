from __future__ import annotations

import logging
from contextlib import asynccontextmanager
from dataclasses import dataclass, field
from datetime import timedelta
from typing import Any, AsyncIterator, Union

from mcp.client.session import ClientSession
from mcp.client.stdio import StdioServerParameters, stdio_client

from .models import (
    ActionDefinition,
    ActionResponse,
    McpPromptArgument,
    McpPromptDefinition,
    McpResourceDefinition,
)

logger = logging.getLogger(__name__)


@dataclass(frozen=True)
class StdioMcpServer:
    """Configuration for an MCP server reached via stdio (subprocess)."""

    command: str
    args: list[str] = field(default_factory=list)
    env: dict[str, str] | None = None
    cwd: str | None = None


@dataclass(frozen=True)
class HttpMcpServer:
    """Configuration for a remote MCP server reached via Streamable HTTP."""

    url: str
    headers: dict[str, str] = field(default_factory=dict)
    timeout_seconds: float = 30.0
    sse_read_timeout_seconds: float = 300.0


McpServer = Union[StdioMcpServer, HttpMcpServer]


class McpAdapter:
    """Bridges an external MCP server into Omni's connector protocol.

    Supports two transports:
    - stdio: spawns the MCP server as a subprocess and talks JSON-RPC over
      stdin/stdout. Works with MCP servers written in any language.
    - Streamable HTTP: connects to a remote MCP endpoint per the MCP spec.

    Each operation opens a fresh session and tears it down afterwards.
    Tool/resource/prompt definitions are cached after the first successful
    discovery so that manifest builds don't require live auth.
    """

    def __init__(self, server: McpServer) -> None:
        self._server = server
        self._cached_actions: list[ActionDefinition] | None = None
        self._cached_resources: list[McpResourceDefinition] | None = None
        self._cached_prompts: list[McpPromptDefinition] | None = None

    @asynccontextmanager
    async def _open_session(
        self,
        env: dict[str, str] | None,
        headers: dict[str, str] | None,
    ) -> AsyncIterator[ClientSession]:
        if isinstance(self._server, StdioMcpServer):
            merged_env = {**(self._server.env or {}), **(env or {})}
            params = StdioServerParameters(
                command=self._server.command,
                args=list(self._server.args),
                env=merged_env or None,
                cwd=self._server.cwd,
            )
            logger.debug(
                "Spawning MCP subprocess: %s %s (env keys: %s)",
                params.command,
                " ".join(params.args),
                sorted(merged_env.keys()),
            )
            async with stdio_client(params) as (read_stream, write_stream):
                async with ClientSession(read_stream, write_stream) as session:
                    await session.initialize()
                    yield session
        else:
            from mcp.client.streamable_http import streamablehttp_client

            merged_headers = {**self._server.headers, **(headers or {})}
            logger.debug(
                "Opening MCP HTTP session: %s (header keys: %s)",
                self._server.url,
                sorted(merged_headers.keys()),
            )
            async with streamablehttp_client(
                self._server.url,
                headers=merged_headers or None,
                timeout=timedelta(seconds=self._server.timeout_seconds),
                sse_read_timeout=timedelta(
                    seconds=self._server.sse_read_timeout_seconds
                ),
            ) as (read_stream, write_stream, _get_session_id):
                async with ClientSession(read_stream, write_stream) as session:
                    await session.initialize()
                    yield session

    async def _run(
        self,
        callback,
        *,
        env: dict[str, str] | None = None,
        headers: dict[str, str] | None = None,
    ):
        async with self._open_session(env, headers) as session:
            return await callback(session)

    async def discover(
        self,
        env: dict[str, str] | None = None,
        headers: dict[str, str] | None = None,
    ) -> None:
        """Connect to MCP server, discover tools/resources/prompts, cache them."""

        async def _discover(session: ClientSession) -> None:
            self._cached_actions = await self._fetch_actions(session)
            self._cached_resources = await self._fetch_resources(session)
            self._cached_prompts = await self._fetch_prompts(session)

        await self._run(_discover, env=env, headers=headers)
        logger.info(
            "MCP discovery complete: %d tools, %d resources, %d prompts",
            len(self._cached_actions or []),
            len(self._cached_resources or []),
            len(self._cached_prompts or []),
        )

    async def get_action_definitions(
        self,
        env: dict[str, str] | None = None,
        headers: dict[str, str] | None = None,
    ) -> list[ActionDefinition]:
        if env is not None or headers is not None:
            try:

                async def _fetch(session):
                    actions = await self._fetch_actions(session)
                    self._cached_actions = actions
                    logger.debug("Fetched %d action definitions (live)", len(actions))
                    return actions

                return await self._run(_fetch, env=env, headers=headers)
            except Exception:
                if self._cached_actions is not None:
                    logger.debug(
                        "Live fetch failed, returning %d cached actions",
                        len(self._cached_actions),
                    )
                    return self._cached_actions
                raise
        logger.debug(
            "No auth provided, returning %d cached actions",
            len(self._cached_actions or []),
        )
        return self._cached_actions or []

    async def get_resource_definitions(
        self,
        env: dict[str, str] | None = None,
        headers: dict[str, str] | None = None,
    ) -> list[McpResourceDefinition]:
        if env is not None or headers is not None:
            try:

                async def _fetch(session):
                    resources = await self._fetch_resources(session)
                    self._cached_resources = resources
                    return resources

                return await self._run(_fetch, env=env, headers=headers)
            except Exception:
                if self._cached_resources is not None:
                    return self._cached_resources
                raise
        return self._cached_resources or []

    async def get_prompt_definitions(
        self,
        env: dict[str, str] | None = None,
        headers: dict[str, str] | None = None,
    ) -> list[McpPromptDefinition]:
        if env is not None or headers is not None:
            try:

                async def _fetch(session):
                    prompts = await self._fetch_prompts(session)
                    self._cached_prompts = prompts
                    return prompts

                return await self._run(_fetch, env=env, headers=headers)
            except Exception:
                if self._cached_prompts is not None:
                    return self._cached_prompts
                raise
        return self._cached_prompts or []

    async def execute_tool(
        self,
        name: str,
        arguments: dict[str, Any],
        env: dict[str, str] | None = None,
        headers: dict[str, str] | None = None,
    ) -> ActionResponse:
        try:

            async def _call(session: ClientSession) -> ActionResponse:
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

            return await self._run(_call, env=env, headers=headers)
        except Exception as e:
            logger.error("MCP tool %s failed: %s", name, e)
            return ActionResponse.failure(str(e))

    async def read_resource(
        self,
        uri: str,
        env: dict[str, str] | None = None,
        headers: dict[str, str] | None = None,
    ) -> dict[str, Any]:
        async def _read(session: ClientSession) -> dict[str, Any]:
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

        return await self._run(_read, env=env, headers=headers)

    async def get_prompt(
        self,
        name: str,
        arguments: dict[str, Any] | None = None,
        env: dict[str, str] | None = None,
        headers: dict[str, str] | None = None,
    ) -> dict[str, Any]:
        async def _get(session: ClientSession) -> dict[str, Any]:
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

        return await self._run(_get, env=env, headers=headers)

    # -- internal helpers to convert MCP types to Omni models --

    @staticmethod
    async def _fetch_actions(session: ClientSession) -> list[ActionDefinition]:
        result = await session.list_tools()
        actions: list[ActionDefinition] = []
        for tool in result.tools:
            is_read_only = bool(tool.annotations and tool.annotations.readOnlyHint)
            actions.append(
                ActionDefinition(
                    name=tool.name,
                    description=tool.description or "",
                    input_schema=tool.inputSchema
                    or {"type": "object", "properties": {}},
                    mode="read" if is_read_only else "write",
                )
            )
        return actions

    @staticmethod
    async def _fetch_resources(session: ClientSession) -> list[McpResourceDefinition]:
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

        return definitions

    @staticmethod
    async def _fetch_prompts(session: ClientSession) -> list[McpPromptDefinition]:
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
        return definitions

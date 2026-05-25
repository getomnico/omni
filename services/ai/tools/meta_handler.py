"""MetaToolHandler: tool_search and load_tool_set for on-demand connector loading.

Connector tools are no longer dumped into the LLM context up front. Instead, the
system prompt advertises *toolsets* (one entry per source) and the model loads
specific actions on demand via the meta-tools defined here. See issue #203.
"""

from __future__ import annotations

import logging
import re
from collections.abc import Awaitable, Callable

from anthropic.types import ToolParam

from tools.connector_handler import ConnectorAction, ConnectorToolHandler
from tools.registry import ToolContext, ToolResult
from tools.searcher_client import (
    CapabilitiesUpsertRequest,
    CapabilitySearchRequest,
    CapabilityUpsert,
    SearcherClient,
)

logger = logging.getLogger(__name__)

_TOOL_NAMES = {"tool_search", "load_tool_set"}
_DEFAULT_LIMIT = 10
_MAX_LIMIT = 25
_TOKEN_RE = re.compile(r"[a-z0-9]+")
LOADED_SOURCES_MARKER = "Loaded source ids:"

OnLoad = Callable[[set[str]], Awaitable[None]]


class MetaToolHandler:
    """Two meta-tools that let the LLM discover and load connector tools on demand."""

    def __init__(
        self,
        connector_handler: ConnectorToolHandler,
        loaded: set[str],
        on_load: OnLoad,
        searcher_client: SearcherClient | None = None,
    ) -> None:
        self._ch = connector_handler
        self._loaded = loaded
        self._on_load = on_load
        self._searcher_client = searcher_client

    def get_tools(self) -> list[ToolParam]:
        return [
            ToolParam(
                name="tool_search",
                description=(
                    "Search across all available connector tools by keyword and load "
                    "the best matches into this conversation. The matched tool schemas "
                    "become callable on your next turn."
                ),
                input_schema={
                    "type": "object",
                    "properties": {
                        "query": {
                            "type": "string",
                            "description": "Keywords matched against tool name and description.",
                        },
                        "limit": {
                            "type": "integer",
                            "description": f"Max tools to load (default {_DEFAULT_LIMIT}, max {_MAX_LIMIT}).",
                            "default": _DEFAULT_LIMIT,
                            "maximum": _MAX_LIMIT,
                            "minimum": 1,
                        },
                    },
                    "required": ["query"],
                },
            ),
            ToolParam(
                name="load_tool_set",
                description=(
                    "Load every tool for a given connector source into this conversation. "
                    "Provide either source_id (a specific source) or source_type (all "
                    "sources of that type, e.g. 'gmail'). Loaded tools become callable "
                    "on your next turn."
                ),
                input_schema={
                    "type": "object",
                    "properties": {
                        "source_id": {
                            "type": "string",
                            "description": "Specific source id to load.",
                        },
                        "source_type": {
                            "type": "string",
                            "description": "Source type to load (loads all sources of this type).",
                        },
                    },
                    "oneOf": [
                        {"required": ["source_id"]},
                        {"required": ["source_type"]},
                    ],
                },
            ),
        ]

    def can_handle(self, tool_name: str) -> bool:
        return tool_name in _TOOL_NAMES

    def requires_approval(self, tool_name: str) -> bool:
        return False

    async def execute(
        self, tool_name: str, tool_input: dict, context: ToolContext
    ) -> ToolResult:
        if tool_name == "tool_search":
            return await self._tool_search(tool_input)
        if tool_name == "load_tool_set":
            return await self._load_tool_set(tool_input)
        return ToolResult(
            content=[{"type": "text", "text": f"Unknown meta-tool: {tool_name}"}],
            is_error=True,
        )

    async def _tool_search(self, tool_input: dict) -> ToolResult:
        query = (tool_input.get("query") or "").strip()
        if not query:
            return ToolResult(
                content=[{"type": "text", "text": "Missing required parameter: query"}],
                is_error=True,
            )

        raw_limit = tool_input.get("limit", _DEFAULT_LIMIT)
        try:
            limit = max(1, min(int(raw_limit), _MAX_LIMIT))
        except (TypeError, ValueError):
            limit = _DEFAULT_LIMIT

        query_tokens = set(_TOKEN_RE.findall(query.lower()))
        if not query_tokens:
            return ToolResult(
                content=[
                    {
                        "type": "text",
                        "text": f"No searchable tokens in query: {query!r}",
                    }
                ],
                is_error=True,
            )

        matches = await self._search_tool_capabilities(query, limit, query_tokens)

        if not matches:
            return ToolResult(
                content=[
                    {
                        "type": "text",
                        "text": (
                            f"No tools matched {query!r}. Try `load_tool_set` with a "
                            "specific source_type from the toolsets list, or rephrase."
                        ),
                    }
                ],
            )

        newly_loaded = await self._mark_loaded(
            {action.source_id for _, action in matches}
        )

        lines = [f"Loaded {len(matches)} tool(s) matching {query!r}:"]
        for tool_name, action in matches:
            desc = (
                (action.description or "").strip().splitlines()[0]
                if action.description
                else ""
            )
            lines.append(f"- {tool_name} — {desc}")
        loaded_ids = {action.source_id for _, action in matches}
        lines.append(f"{LOADED_SOURCES_MARKER} {', '.join(sorted(loaded_ids))}")
        if newly_loaded:
            lines.append(
                f"Sources newly available this turn: {', '.join(sorted(newly_loaded))}."
            )
        lines.append("Call any of these tools on your next turn.")

        return ToolResult(content=[{"type": "text", "text": "\n".join(lines)}])

    async def _search_tool_capabilities(
        self, query: str, limit: int, query_tokens: set[str]
    ) -> list[tuple[str, ConnectorAction]]:
        if self._searcher_client is not None:
            try:
                await self._publish_tool_capabilities()
                response = await self._searcher_client.search_capabilities(
                    CapabilitySearchRequest(
                        capability_type="tool",
                        query=query,
                        limit=limit,
                        allowed_ids=[
                            f"tool:{tool_name}" for tool_name in self._ch.actions
                        ],
                    )
                )
                matches: list[tuple[str, ConnectorAction]] = []
                seen: set[str] = set()
                for result in response.results:
                    tool_name = result.data["tool_name"]
                    action = self._ch.actions.get(tool_name)
                    if action is None or tool_name in seen:
                        continue
                    seen.add(tool_name)
                    matches.append((tool_name, action))
                if matches:
                    return matches
            except Exception as e:
                logger.warning(
                    f"Capability tool search failed; using local fallback: {e}"
                )

        return self._local_tool_search(query_tokens, limit)

    async def _publish_tool_capabilities(self) -> None:
        if self._searcher_client is None or not self._ch.actions:
            return
        capabilities: list[CapabilityUpsert] = []
        for tool_name, action in self._ch.actions.items():
            capabilities.append(
                CapabilityUpsert(
                    id=f"tool:{tool_name}",
                    capability_type="tool",
                    search_text=(
                        f"{tool_name} {action.source_type} {action.source_name} "
                        f"{action.action_name} {action.description or ''}"
                    ),
                    data={
                        "tool_name": tool_name,
                        "description": action.description or "",
                        "source_id": action.source_id,
                        "source_type": action.source_type,
                        "source_name": action.source_name,
                        "action_name": action.action_name,
                        "mode": action.mode,
                    },
                )
            )
        await self._searcher_client.upsert_capabilities(
            CapabilitiesUpsertRequest(capabilities=capabilities)
        )

    def _local_tool_search(
        self, query_tokens: set[str], limit: int
    ) -> list[tuple[str, ConnectorAction]]:
        scored: list[tuple[int, str, ConnectorAction]] = []
        for tool_name, action in self._ch.actions.items():
            name_tokens = set(_TOKEN_RE.findall(tool_name.lower()))
            desc_tokens = set(_TOKEN_RE.findall((action.description or "").lower()))
            type_tokens = set(_TOKEN_RE.findall(action.source_type.lower()))
            score = (
                3 * len(query_tokens & name_tokens)
                + 1 * len(query_tokens & desc_tokens)
                + 2 * len(query_tokens & type_tokens)
            )
            if score > 0:
                scored.append((score, tool_name, action))

        scored.sort(key=lambda x: (-x[0], x[1]))
        return [(tool_name, action) for _, tool_name, action in scored[:limit]]

    async def _load_tool_set(self, tool_input: dict) -> ToolResult:
        source_id = tool_input.get("source_id")
        source_type = tool_input.get("source_type")

        if not source_id and not source_type:
            return ToolResult(
                content=[
                    {
                        "type": "text",
                        "text": "Provide either source_id or source_type.",
                    }
                ],
                is_error=True,
            )

        target_ids: set[str] = set()
        matched_actions: list[ConnectorAction] = []
        for action in self._ch.actions.values():
            if (source_id and action.source_id == source_id) or (
                source_type and action.source_type == source_type
            ):
                target_ids.add(action.source_id)
                matched_actions.append(action)

        if not target_ids:
            key = source_id or source_type
            return ToolResult(
                content=[
                    {
                        "type": "text",
                        "text": (
                            f"No connector toolset found for {key!r}. "
                            "Use the toolsets list in the system prompt to find a valid source_type."
                        ),
                    }
                ],
                is_error=True,
            )

        newly_loaded = await self._mark_loaded(target_ids)

        # Group by actual LLM tool name for reporting. Duplicate source types may
        # have source-suffixed tool names, so do not reconstruct names here.
        unique_tools: dict[str, ConnectorAction] = {}
        matched_ids = {id(action) for action in matched_actions}
        for tool_name, action in self._ch.actions.items():
            if id(action) not in matched_ids:
                continue
            unique_tools.setdefault(tool_name, action)

        lines = [
            f"Loaded {len(unique_tools)} tool(s) from " f"{len(target_ids)} source(s):"
        ]
        for tool_name, action in sorted(unique_tools.items()):
            desc = (
                (action.description or "").strip().splitlines()[0]
                if action.description
                else ""
            )
            lines.append(f"- {tool_name} — {desc}")
        lines.append(f"{LOADED_SOURCES_MARKER} {', '.join(sorted(target_ids))}")
        if not newly_loaded:
            lines.append("(All targeted sources were already loaded.)")
        lines.append("Call any of these tools on your next turn.")

        return ToolResult(content=[{"type": "text", "text": "\n".join(lines)}])

    async def _mark_loaded(self, source_ids: set[str]) -> set[str]:
        """Add source_ids to the loaded set; persist if the set changed."""
        newly = source_ids - self._loaded
        if not newly:
            return set()
        self._loaded |= newly
        try:
            await self._on_load(newly)
        except Exception as e:
            logger.warning(f"Failed to persist loaded toolsets {newly}: {e}")
        return newly

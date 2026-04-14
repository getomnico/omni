"""SearchToolHandler: wraps existing search functionality."""

from __future__ import annotations

import json
import logging
import time
from datetime import datetime, timezone

import redis.asyncio as aioredis
from pydantic import ValidationError
from anthropic.types import (
    TextBlockParam,
    SearchResultBlockParam,
    CitationsConfigParam,
    ToolParam,
)

from models.chat import SearchToolParams
from tools.searcher_tool import SearcherTool
from tools.searcher_client import (
    SearcherClient,
    SearchRequest,
    SearchResponse,
    SearchResult,
)
from tools.connector_handler import SearchOperator
from tools.registry import ToolContext, ToolResult

logger = logging.getLogger(__name__)

_TOOL_NAMES = {"search_documents"}

# Operators already documented as universal — exclude from connector-specific lists
_UNIVERSAL_OPERATORS = {"by", "in", "from", "type", "before", "after"}

TYPE_VALID_VALUES = [
    "sheet",
    "doc",
    "slide",
    "pdf",
    "issue",
    "pr",
    "page",
    "email",
    "meeting",
]

_MAX_DISPLAYED_VALUES = 20
_OPERATOR_VALUES_CACHE_KEY = "search:operator_values"
_OPERATOR_VALUES_CACHE_TTL = 300  # 5 minutes

# In-memory cache so the hot path (every LLM call) is a timestamp check, not a Redis round-trip.
_operator_values_mem: dict[str, list[str]] = {}
_operator_values_mem_ts: float = 0


async def fetch_operator_values(
    searcher_client: SearcherClient,
    search_operators: list[SearchOperator],
    redis_client: aioredis.Redis | None = None,
) -> dict[str, list[str]]:
    """Fetch and cache distinct values for dynamic search operators.

    Cache hierarchy: in-memory (instant) → Redis (network) → searcher API (DB query).
    """
    global _operator_values_mem, _operator_values_mem_ts

    now = time.monotonic()
    if (
        _operator_values_mem
        and (now - _operator_values_mem_ts) < _OPERATOR_VALUES_CACHE_TTL
    ):
        return _operator_values_mem

    if redis_client:
        try:
            cached = await redis_client.get(_OPERATOR_VALUES_CACHE_KEY)
            if cached:
                _operator_values_mem = json.loads(cached)
                _operator_values_mem_ts = now
                return _operator_values_mem
        except Exception as e:
            logger.warning(f"Failed to read operator values cache: {e}")

    attribute_keys = [
        op.attribute_key
        for op in search_operators
        if op.operator not in _UNIVERSAL_OPERATORS and op.attribute_key
    ]
    if not attribute_keys:
        return {}

    try:
        values = await searcher_client.get_attribute_values(attribute_keys)
    except Exception as e:
        logger.warning(f"Failed to fetch operator values from searcher: {e}")
        return {}

    _operator_values_mem = values
    _operator_values_mem_ts = now

    if redis_client and values:
        try:
            await redis_client.set(
                _OPERATOR_VALUES_CACHE_KEY,
                json.dumps(values),
                ex=_OPERATOR_VALUES_CACHE_TTL,
            )
        except Exception as e:
            logger.warning(f"Failed to cache operator values: {e}")

    return values


def _build_query_description(
    search_operators: list[SearchOperator],
    connected_source_types: list[str] | None = None,
    operator_values: dict[str, list[str]] | None = None,
) -> str:
    """Build a rich description for the query parameter with operator syntax."""
    # Build in: values from connected source types (the searcher accepts source_type values directly)
    if connected_source_types:
        in_values_str = f". Values: {', '.join(sorted(connected_source_types))}"
    else:
        in_values_str = ""

    type_values_str = ", ".join(TYPE_VALID_VALUES)

    lines = [
        "The search query. Supports inline operators for filtering:",
        "",
        "Universal operators:",
        f"- in:<source> — filter by app{in_values_str}",
        "- by:<person> — filter by author/creator",
        "- from:<person> — filter by sender (emails, messages)",
        f"- type:<type> — content type. Values: {type_values_str}",
        "- before:<date> / after:<date> — date range (YYYY-MM-DD, YYYY-MM, or YYYY)",
        "Date keywords (no operator needed): last week, last month, this week, yesterday, today",
    ]

    # Group connector-specific operators by source_type
    ops_by_source: dict[str, list[str]] = {}
    for op in search_operators:
        if op.operator in _UNIVERSAL_OPERATORS:
            continue
        display_name = op.display_name or op.source_type

        # Build operator text with values if available
        values = (operator_values or {}).get(op.attribute_key, [])
        if values:
            displayed = values[:_MAX_DISPLAYED_VALUES]
            suffix = ", ..." if len(values) > _MAX_DISPLAYED_VALUES else ""
            values_str = f" ({', '.join(displayed)}{suffix})"
        else:
            values_str = ""
        ops_by_source.setdefault(display_name, []).append(
            f"{op.operator}:<value>{values_str}"
        )

    if ops_by_source:
        lines.append("")
        lines.append("Connector-specific operators:")
        for source_name in sorted(ops_by_source):
            ops_str = ", ".join(sorted(ops_by_source[source_name]))
            lines.append(f"- {source_name}: {ops_str}")

    lines.append("")
    lines.append(
        'Examples: "status:done in:jira sprint tasks", "type:pdf after:2024-01 invoice", "budget last week"'
    )
    lines.append("")
    lines.append(
        "Important: Boolean operators (AND, OR, NOT) are NOT supported. "
        "Use multiple inline operators in the same query instead."
    )

    return "\n".join(lines)


def _build_search_tools(
    search_operators: list[SearchOperator] | None = None,
    connected_source_types: list[str] | None = None,
    operator_values: dict[str, list[str]] | None = None,
) -> list[dict]:
    """Build the search tool definition with dynamic operators."""
    query_desc = _build_query_description(
        search_operators or [],
        connected_source_types=connected_source_types,
        operator_values=operator_values,
    )

    return [
        {
            "name": "search_documents",
            "description": "Search the Omni index for data across all connected apps using hybrid text and semantic search. Use this when you need to find information to answer user questions. Use inline query operators (in:, by:, type:, status:, etc.) for filtering.",
            "input_schema": {
                "type": "object",
                "properties": {
                    "query": {
                        "type": "string",
                        "description": query_desc,
                    },
                    "document_id": {
                        "type": "string",
                        "description": "Optional: restrict search to a specific document by ID. Use this to search within a single document for relevant sections.",
                    },
                    "limit": {
                        "type": "integer",
                        "description": "Maximum number of results to return (default: 10)",
                    },
                },
                "required": ["query"],
            },
        },
    ]


class SearchToolHandler:
    """Wraps existing search logic as a ToolHandler."""

    def __init__(
        self,
        searcher_tool: SearcherTool,
        search_operators: list[SearchOperator] | None = None,
        connected_source_types: list[str] | None = None,
        operator_values: dict[str, list[str]] | None = None,
    ) -> None:
        self._searcher = searcher_tool
        self._tools = _build_search_tools(
            search_operators,
            connected_source_types=connected_source_types,
            operator_values=operator_values,
        )

    def get_tools(self) -> list[ToolParam]:
        return self._tools

    def can_handle(self, tool_name: str) -> bool:
        return tool_name in _TOOL_NAMES

    def requires_approval(self, tool_name: str) -> bool:
        return False  # search is read-only

    async def execute(
        self, tool_name: str, tool_input: dict, context: ToolContext
    ) -> ToolResult:
        if tool_name == "search_documents":
            return await self._execute_search(tool_input, context)
        return ToolResult(
            content=[{"type": "text", "text": f"Unknown search tool: {tool_name}"}],
            is_error=True,
        )

    async def _execute_search(
        self, tool_input: dict, context: ToolContext
    ) -> ToolResult:
        try:
            params = SearchToolParams.model_validate(tool_input)
        except ValidationError as e:
            logger.error(f"Invalid search_documents input: {e}")
            return ToolResult(
                content=[{"type": "text", "text": f"Invalid parameters: {e}"}],
                is_error=True,
            )

        logger.info(
            f"Executing search_documents with query: {params.query}, document_id: {params.document_id}, context: {context}"
        )
        search_results = await _execute_search_tool(
            self._searcher,
            params,
            context.user_id,
            context.user_email,
            context.original_user_query,
        )

        content_blocks: list = []
        for result in search_results:
            doc = result.document
            doc_content_text_blocks = [
                TextBlockParam(type="text", text=h) for h in result.highlights
            ]

            metadata_blocks = [
                TextBlockParam(type="text", text=f"[Document ID: {doc.id}]"),
                TextBlockParam(type="text", text=f"[Document Name: {doc.title}]"),
                TextBlockParam(
                    type="text",
                    text=f"[Source: {result.source_type or 'unknown'}]",
                ),
                TextBlockParam(type="text", text=f"[URL: {doc.url or '<unknown>'}]"),
            ]

            # Extract a human-readable date for the LLM. Prefer metadata updated_at
            # (original content date) over created_at, falling back to a unix timestamp
            # in attributes when present.
            date_str: str | None = None
            if doc.metadata:
                raw_date = doc.metadata.get("updated_at") or doc.metadata.get(
                    "created_at"
                )
                if raw_date and isinstance(raw_date, str):
                    try:
                        dt = datetime.fromisoformat(raw_date.replace("Z", "+00:00"))
                        # Normalize to UTC so the label is always correct
                        dt = dt.astimezone(timezone.utc)
                        date_str = dt.strftime("%Y-%m-%d %H:%M UTC")
                    except (ValueError, AttributeError):
                        date_str = raw_date
            if not date_str and doc.attributes:
                raw_ts = doc.attributes.get("updated_at") or doc.attributes.get(
                    "created_at"
                )
                if raw_ts and isinstance(raw_ts, (int, float)):
                    try:
                        dt = datetime.fromtimestamp(raw_ts, tz=timezone.utc)
                        date_str = dt.strftime("%Y-%m-%d %H:%M UTC")
                    except (OSError, OverflowError, ValueError):
                        pass
            if date_str:
                metadata_blocks.append(
                    TextBlockParam(type="text", text=f"[Date: {date_str}]")
                )

            if doc.attributes:
                attrs_str = ", ".join(f"{k}: {v}" for k, v in doc.attributes.items())
                metadata_blocks.append(
                    TextBlockParam(type="text", text=f"[Attributes: {attrs_str}]")
                )

            extra = (doc.metadata or {}).get("extra")
            if extra and isinstance(extra, dict):
                extra_str = ", ".join(f"{k}: {v}" for k, v in extra.items())
                metadata_blocks.append(
                    TextBlockParam(type="text", text=f"[Extra: {extra_str}]")
                )

            content_blocks.append(
                SearchResultBlockParam(
                    type="search_result",
                    title=doc.title,
                    source=doc.url or "<unknown>",
                    content=[*metadata_blocks, *doc_content_text_blocks],
                    citations=CitationsConfigParam(enabled=True),
                )
            )

        return ToolResult(content=content_blocks)


async def _execute_search_tool(
    searcher_tool: SearcherTool,
    tool_input: SearchToolParams,
    user_id: str,
    user_email: str | None = None,
    original_user_query: str | None = None,
) -> list[SearchResult]:
    """Execute search_documents tool by calling omni-searcher."""
    search_request = SearchRequest(
        query=tool_input.query,
        document_id=tool_input.document_id,
        limit=tool_input.limit or 10,
        offset=0,
        mode="hybrid",
        user_id=user_id,
        user_email=user_email,
        is_generated_query=True,
        original_user_query=original_user_query,
        include_facets=False,
        ignore_typos=True,
    )
    try:
        response: SearchResponse = await searcher_tool.handle(search_request)
    except Exception as e:
        logger.error(f"Search failed: {e}")
        return []
    return response.results

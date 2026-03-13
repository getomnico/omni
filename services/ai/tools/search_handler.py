"""SearchToolHandler: wraps existing search functionality."""

from __future__ import annotations

import logging

from pydantic import ValidationError
from anthropic.types import (
    TextBlockParam,
    SearchResultBlockParam,
    CitationsConfigParam,
)

from models.chat import SearchToolParams
from tools.searcher_tool import SearcherTool
from tools.searcher_client import SearchRequest, SearchResponse, SearchResult
from tools.registry import ToolContext, ToolResult

logger = logging.getLogger(__name__)

_TOOL_NAMES = {"search_documents"}

# Operators already documented as universal — exclude from connector-specific lists
_UNIVERSAL_OPERATORS = {"by", "in", "from", "type", "before", "after"}

SOURCE_DISPLAY_NAMES = {
    "google_drive": "Google Drive",
    "gmail": "Gmail",
    "confluence": "Confluence",
    "jira": "Jira",
    "slack": "Slack",
    "hubspot": "HubSpot",
    "fireflies": "Fireflies",
    "web": "Web",
    "local_files": "Files",
    "github": "GitHub",
    "notion": "Notion",
    "one_drive": "OneDrive",
    "share_point": "SharePoint",
    "outlook": "Outlook",
    "outlook_calendar": "Outlook Calendar",
}


def _build_query_description(
    search_operators: list[dict],
) -> str:
    """Build a rich description for the query parameter with operator syntax."""
    lines = [
        "The search query. Supports inline operators for filtering:",
        "",
        "Universal operators:",
        "- in:<source> — filter by app (e.g., in:slack, in:drive, in:jira)",
        "- by:<person> — filter by author/creator",
        "- from:<person> — filter by sender (emails, messages)",
        "- type:<type> — content type (sheet, doc, pdf, email, issue, pr, meeting, slide, page)",
        "- before:<date> / after:<date> — date range (YYYY-MM-DD, YYYY-MM, or YYYY)",
        "Date keywords (no operator needed): last week, last month, this week, yesterday, today",
    ]

    # Group connector-specific operators by source_type
    ops_by_source: dict[str, list[str]] = {}
    for op in search_operators:
        if op["operator"] in _UNIVERSAL_OPERATORS:
            continue
        source_type = op.get("source_type", "")
        display_name = SOURCE_DISPLAY_NAMES.get(source_type, source_type)
        ops_by_source.setdefault(display_name, []).append(f"{op['operator']}:<value>")

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

    return "\n".join(lines)


def _build_search_tools(
    search_operators: list[dict] | None = None,
) -> list[dict]:
    """Build the search tool definition with dynamic operators."""
    query_desc = _build_query_description(search_operators or [])

    return [
        {
            "name": "search_documents",
            "description": "Search enterprise documents using hybrid text and semantic search. Use this when you need to find information to answer user questions. Use inline query operators (in:, by:, type:, status:, etc.) for filtering.",
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
        search_operators: list[dict] | None = None,
    ) -> None:
        self._searcher = searcher_tool
        self._tools = _build_search_tools(search_operators)

    def get_tools(self) -> list[dict]:
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
            content_blocks.append(
                SearchResultBlockParam(
                    type="search_result",
                    title=doc.title,
                    source=doc.url or "<unknown>",
                    content=[
                        TextBlockParam(type="text", text=f"[Document ID: {doc.id}]"),
                        TextBlockParam(
                            type="text", text=f"[Document Name: {doc.title}]"
                        ),
                        TextBlockParam(
                            type="text", text=f"[URL: {doc.url or '<unknown>'}]"
                        ),
                        *doc_content_text_blocks,
                    ],
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

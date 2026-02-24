"""SearchToolHandler: wraps existing search/read document functionality."""

from __future__ import annotations

import json
import logging

from pydantic import ValidationError
from anthropic.types import (
    TextBlockParam,
    SearchResultBlockParam,
    CitationsConfigParam,
)

from models.chat import SearchToolParams, ReadDocumentParams
from tools.searcher_tool import SearcherTool
from tools.searcher_client import SearchRequest, SearchResponse, SearchResult
from tools.registry import ToolContext, ToolResult

logger = logging.getLogger(__name__)

SEARCH_TOOLS = [
    {
        "name": "search_documents",
        "description": "Search enterprise documents using hybrid text and semantic search. Use this when you need to find information to answer user questions. Wherever possible, use the sources parameter to limit the search to specific apps.",
        "input_schema": {
            "type": "object",
            "properties": {
                "query": {
                    "type": "string",
                    "description": "The search query to find relevant documents. Can search using keywords, or a natural language question to get semantic search results.",
                },
                "sources": {
                    "type": "array",
                    "items": {"type": "string"},
                    "description": "Optional: specific source types to search (valid values: google_drive, slack, confluence, jira, web, slack, fireflies, hubspot.)",
                },
                "content_types": {
                    "type": "array",
                    "items": {"type": "string"},
                    "description": "Optional: file types to include (e.g., pdf, docx, txt)",
                },
                "attributes": {
                    "type": "object",
                    "description": (
                        "Optional: filter results by document attributes. "
                        "Common Jira attributes: status, priority, issue_type, assignee, reporter, labels, components, project_key. "
                        "Common Confluence attributes: space_id, status. "
                        'Values can be: a string for exact match (e.g., {"status": "Done"}), '
                        'an array for OR match (e.g., {"priority": ["High", "Critical"]}), '
                        'or an object with gte/lte keys for range queries (e.g., {"updated": {"gte": "2024-01-01"}}).'
                    ),
                },
                "limit": {
                    "type": "integer",
                    "description": "Maximum number of results to return (default: 10)",
                },
            },
            "required": ["query"],
        },
    },
    {
        "name": "read_document",
        "description": "Read the content of a specific document by its URL. For small documents, returns the full content. For large documents, you can provide a query parameter to get the most relevant sections. Use this when you need detailed information from a specific document (e.g., from search results).",
        "input_schema": {
            "type": "object",
            "properties": {
                "id": {
                    "type": "string",
                    "description": "The ID of the document to read",
                },
                "name": {
                    "type": "string",
                    "description": "The name of the document to read",
                },
                "query": {
                    "type": "string",
                    "description": "Optional: specify what you're looking for to get the most relevant sections. If you specify line numbers, this this will be ignored.",
                },
                "start_line": {
                    "type": "integer",
                    "description": "Optional: start line number (inclusive) to read from.",
                },
                "end_line": {
                    "type": "integer",
                    "description": "Optional: end line number (inclusive) to read to",
                },
            },
            "required": ["id", "name"],
        },
    },
]

_TOOL_NAMES = {"search_documents", "read_document"}


class SearchToolHandler:
    """Wraps existing search/read logic as a ToolHandler."""

    def __init__(self, searcher_tool: SearcherTool) -> None:
        self._searcher = searcher_tool

    def get_tools(self) -> list[dict]:
        return SEARCH_TOOLS

    def can_handle(self, tool_name: str) -> bool:
        return tool_name in _TOOL_NAMES

    def requires_approval(self, tool_name: str) -> bool:
        return False  # search/read are read-only

    async def execute(
        self, tool_name: str, tool_input: dict, context: ToolContext
    ) -> ToolResult:
        if tool_name == "search_documents":
            return await self._execute_search(tool_input, context)
        elif tool_name == "read_document":
            return await self._execute_read(tool_input, context)
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
            f"Executing search_documents with query: {params.query}, context: {context}"
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

    async def _execute_read(self, tool_input: dict, context: ToolContext) -> ToolResult:
        try:
            params = ReadDocumentParams.model_validate(tool_input)
        except ValidationError as e:
            logger.error(f"Invalid read_document input: {e}")
            return ToolResult(
                content=[{"type": "text", "text": f"Invalid parameters: {e}"}],
                is_error=True,
            )

        logger.info(f"Executing read_document with params: {params}")
        read_results = await _execute_read_document_tool(
            self._searcher, params, context.user_id, context.user_email
        )

        content_blocks: list[TextBlockParam] = []

        doc = read_results[0].document if read_results else None
        if doc:
            content_blocks.append(
                TextBlockParam(
                    type="text",
                    text=json.dumps({"attributes": doc.attributes}),
                )
            )

        for result in read_results:
            content_blocks.append(
                TextBlockParam(type="text", text="\n".join(result.highlights))
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
        source_types=tool_input.sources,
        content_types=tool_input.content_types,
        limit=tool_input.limit or 10,
        offset=0,
        mode="hybrid",
        user_id=user_id,
        user_email=user_email,
        is_generated_query=True,
        original_user_query=original_user_query,
        include_facets=False,
        ignore_typos=True,
        attribute_filters=tool_input.attributes,
    )
    try:
        response: SearchResponse = await searcher_tool.handle(search_request)
    except Exception as e:
        logger.error(f"Search failed: {e}")
        return []
    return response.results


async def _execute_read_document_tool(
    searcher_tool: SearcherTool,
    tool_input: ReadDocumentParams,
    user_id: str,
    user_email: str | None = None,
) -> list[SearchResult]:
    """Execute read_document tool by calling omni-searcher with document_id filter."""
    search_request = SearchRequest(
        query=tool_input.query or "",
        document_id=tool_input.id,
        document_content_start_line=tool_input.start_line,
        document_content_end_line=tool_input.end_line,
        limit=20,
        offset=0,
        mode="hybrid",
        user_id=user_id,
        user_email=user_email,
    )
    try:
        response: SearchResponse = await searcher_tool.handle(search_request)
    except Exception as e:
        logger.error(f"Read document failed: {e}")
        return []
    return response.results

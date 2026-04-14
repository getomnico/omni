"""PeopleSearchHandler: search_people tool for the LLM agent."""

from __future__ import annotations

import logging

from anthropic.types import ToolParam

from tools.searcher_client import PeopleSearchRequest, PeopleSearchResponse
from tools.searcher_tool import SearcherTool
from tools.registry import ToolContext, ToolResult

logger = logging.getLogger(__name__)

TOOL_NAME = "search_people"


class PeopleSearchHandler:
    """Lets the LLM search the people directory."""

    def __init__(self, searcher_tool: SearcherTool) -> None:
        self._searcher = searcher_tool

    def get_tools(self) -> list[ToolParam]:
        return [
            {
                "name": TOOL_NAME,
                "description": (
                    "Search the people directory to find colleagues by name, "
                    "email, job title, or department."
                ),
                "input_schema": {
                    "type": "object",
                    "properties": {
                        "query": {
                            "type": "string",
                            "description": "Search query — a name, email address, job title, department, or keyword.",
                        },
                        "limit": {
                            "type": "integer",
                            "description": "Maximum number of results to return (default: 10)",
                        },
                    },
                    "required": ["query"],
                },
            }
        ]

    def can_handle(self, tool_name: str) -> bool:
        return tool_name == TOOL_NAME

    def requires_approval(self, tool_name: str) -> bool:
        return False

    async def execute(
        self, tool_name: str, tool_input: dict, context: ToolContext
    ) -> ToolResult:
        query = tool_input.get("query", "").strip()
        if not query:
            return ToolResult(
                content=[{"type": "text", "text": "Error: 'query' is required"}],
                is_error=True,
            )

        limit = tool_input.get("limit", 10)
        request = PeopleSearchRequest(query=query, limit=limit)

        try:
            response: PeopleSearchResponse = await self._searcher.client.search_people(
                request
            )
        except Exception as e:
            logger.error(f"People search failed: {e}")
            return ToolResult(
                content=[{"type": "text", "text": f"People search failed: {e}"}],
                is_error=True,
            )

        if not response.people:
            return ToolResult(
                content=[
                    {"type": "text", "text": "No people found matching the query."}
                ],
            )

        lines: list[str] = []
        for person in response.people:
            parts = [f"Email: {person.email}"]
            if person.display_name:
                parts.insert(0, f"Name: {person.display_name}")
            if person.job_title:
                parts.append(f"Title: {person.job_title}")
            if person.department:
                parts.append(f"Department: {person.department}")
            lines.append("\n".join(parts))

        text = "\n\n".join(lines)
        return ToolResult(content=[{"type": "text", "text": text}])

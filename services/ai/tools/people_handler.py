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
                    "email, job title, or department. Use the structured "
                    "filters (department, office location, work country, "
                    "employee type) to narrow results."
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
                        "department": {
                            "type": "string",
                            "description": "Filter results to this department, e.g. 'Marketing'.",
                        },
                        "office_location": {
                            "type": "string",
                            "description": "Filter results to this office location, e.g. 'Paris' or 'New York'.",
                        },
                        "work_country": {
                            "type": "string",
                            "description": "Filter results to this work country, e.g. 'India' or 'USA'.",
                        },
                        "employee_type": {
                            "type": "string",
                            "description": "Filter results to this employee type, e.g. 'Full Time'.",
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
        request = PeopleSearchRequest(
            query=query,
            limit=limit,
            department=tool_input.get("department"),
            office_location=tool_input.get("office_location"),
            work_country=tool_input.get("work_country"),
            employee_type=tool_input.get("employee_type"),
        )

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
            if person.company_name:
                parts.append(f"Company: {person.company_name}")
            if person.office_location:
                parts.append(f"Office: {person.office_location}")
            if person.work_country:
                parts.append(f"Work Country: {person.work_country}")
            if person.employee_id:
                parts.append(f"Employee ID: {person.employee_id}")
            lines.append("\n".join(parts))

        text = "\n\n".join(lines)
        return ToolResult(content=[{"type": "text", "text": text}])

"""SubmitAnswerHandler: benchmark-only tool that lets the agent emit its final
answer and the dsids it used in a structured form.

Registered only when BENCHMARK_MODE=true so it doesn't surface in production
chats. The execute() side is a no-op acknowledgement — the args are captured
client-side from the SSE `tool_use` event.
"""

from __future__ import annotations

import logging

from anthropic.types import ToolParam

from tools.registry import ToolContext, ToolResult

logger = logging.getLogger(__name__)

TOOL_NAME = "submit_answer"


class SubmitAnswerHandler:
    """Lets the LLM submit a final structured answer for benchmark scoring."""

    def get_tools(self) -> list[ToolParam]:
        return [
            {
                "name": TOOL_NAME,
                "description": (
                    "Submit your final natural-language answer. Call this exactly "
                    "once after you have gathered enough information from the "
                    "documents. After calling submit_answer, do not produce any "
                    "further output and do not call any other tools."
                ),
                "input_schema": {
                    "type": "object",
                    "properties": {
                        "answer": {
                            "type": "string",
                            "description": (
                                "The final natural-language answer to the user's "
                                "question. If the answer cannot be determined from "
                                "the available documents, state that explicitly."
                            ),
                        },
                        "document_ids": {
                            "type": "array",
                            "items": {"type": "string"},
                            "maxItems": 10,
                            "description": (
                                "Only dsid_* document IDs that directly support the "
                                "final answer, ordered from most to least important. "
                                "Include a document only if you would cite it as "
                                "evidence for at least one answer fact. Use the "
                                "external dsid_* IDs shown in search result URLs, not "
                                "the internal ULID document IDs. Do not include "
                                "tangentially related documents, background reading, "
                                "or top search results that do not support the final "
                                "answer. Do not fill to 10; an empty list is correct "
                                "when no evidence was found."
                            ),
                        },
                    },
                    "required": ["answer"],
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
        answer = tool_input.get("answer", "")
        document_ids = tool_input.get("document_ids", [])
        logger.info(
            "submit_answer received: %d-char answer, %d document_ids",
            len(answer),
            len(document_ids) if isinstance(document_ids, list) else 0,
        )
        return ToolResult(
            content=[
                {
                    "type": "text",
                    "text": "Answer submitted. Stop now — do not produce any further output.",
                }
            ]
        )

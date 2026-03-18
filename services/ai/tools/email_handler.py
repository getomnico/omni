"""EmailToolHandler: send_email tool for org agents."""

from __future__ import annotations

import html
import logging

from email_service.sender import EmailSender
from tools.registry import ToolContext, ToolResult

logger = logging.getLogger(__name__)

TOOL_NAME = "send_email"


def _body_to_html(body: str) -> str:
    """Convert plain text body to simple HTML email."""
    escaped = html.escape(body)
    paragraphs = escaped.split("\n\n")
    html_parts = []
    for p in paragraphs:
        lines = p.replace("\n", "<br>")
        html_parts.append(f"<p>{lines}</p>")

    return f"""<!DOCTYPE html>
<html>
<head><meta charset="utf-8"></head>
<body style="font-family: -apple-system, BlinkMacSystemFont, 'Segoe UI', Roboto, sans-serif; line-height: 1.6; color: #1a1a1a; max-width: 600px; margin: 0 auto; padding: 20px;">
{''.join(html_parts)}
</body>
</html>"""


class EmailToolHandler:
    def __init__(self):
        self._sender = EmailSender()

    def get_tools(self) -> list[dict]:
        return [
            {
                "name": TOOL_NAME,
                "description": (
                    "Send an email to a recipient. Use this to send reports, summaries, "
                    "notifications, or other content via email."
                ),
                "input_schema": {
                    "type": "object",
                    "properties": {
                        "to": {
                            "type": "string",
                            "description": "Recipient email address",
                        },
                        "subject": {
                            "type": "string",
                            "description": "Email subject line",
                        },
                        "body": {
                            "type": "string",
                            "description": "Email body content in plain text",
                        },
                    },
                    "required": ["to", "subject", "body"],
                },
            }
        ]

    def can_handle(self, tool_name: str) -> bool:
        return tool_name == TOOL_NAME

    def requires_approval(self, tool_name: str) -> bool:
        return True

    async def execute(
        self, tool_name: str, tool_input: dict, context: ToolContext
    ) -> ToolResult:
        to = tool_input.get("to", "").strip()
        subject = tool_input.get("subject", "").strip()
        body = tool_input.get("body", "").strip()

        if not to:
            return ToolResult(
                content=[{"type": "text", "text": "Error: 'to' is required"}],
                is_error=True,
            )
        if not subject:
            return ToolResult(
                content=[{"type": "text", "text": "Error: 'subject' is required"}],
                is_error=True,
            )
        if not body:
            return ToolResult(
                content=[{"type": "text", "text": "Error: 'body' is required"}],
                is_error=True,
            )

        html_body = _body_to_html(body)
        result = await self._sender.send(
            to=to, subject=subject, html=html_body, text=body
        )

        if result.success:
            msg = f"Email sent to {to}"
            if result.message_id:
                msg += f" (ID: {result.message_id})"
            return ToolResult(content=[{"type": "text", "text": msg}])
        else:
            return ToolResult(
                content=[
                    {"type": "text", "text": f"Failed to send email: {result.error}"}
                ],
                is_error=True,
            )

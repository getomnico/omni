"""Shared sandbox utilities for tool handlers."""

import base64
import logging

import httpx

from tools.registry import ToolResult

logger = logging.getLogger(__name__)


async def write_text_to_sandbox(
    sandbox_url: str,
    text: str,
    file_name: str,
    chat_id: str,
    *,
    message: str | None = None,
) -> ToolResult:
    """Write text data to the sandbox and return a ToolResult for the LLM."""
    size_kb = len(text.encode("utf-8")) / 1024

    async with httpx.AsyncClient(timeout=60.0) as client:
        resp = await client.post(
            f"{sandbox_url.rstrip('/')}/files/write",
            json={
                "path": file_name,
                "content": text,
                "chat_id": chat_id,
            },
        )
        resp.raise_for_status()

    text_message = message or f"File saved to workspace: {file_name} ({size_kb:.0f} KB)"
    return ToolResult(content=[{"type": "text", "text": text_message}])


async def write_binary_to_sandbox(
    sandbox_url: str,
    binary_data: bytes,
    file_name: str,
    chat_id: str,
) -> ToolResult:
    """Write binary data to the sandbox and return a ToolResult for the LLM."""
    encoded = base64.b64encode(binary_data).decode("ascii")
    size_kb = len(binary_data) / 1024

    async with httpx.AsyncClient(timeout=60.0) as client:
        resp = await client.post(
            f"{sandbox_url.rstrip('/')}/files/write_binary",
            json={
                "path": file_name,
                "content_base64": encoded,
                "chat_id": chat_id,
            },
        )
        resp.raise_for_status()

    return ToolResult(
        content=[
            {
                "type": "text",
                "text": f"File saved to workspace: {file_name} ({size_kb:.0f} KB)",
            }
        ],
    )

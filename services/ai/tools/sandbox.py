"""Shared sandbox utilities for tool handlers."""

import base64
import logging

import httpx

from tools.registry import ToolResult

logger = logging.getLogger(__name__)

DEFAULT_INLINE_TEXT_MAX_BYTES = 40 * 1024


def is_textual_content_type(content_type: str) -> bool:
    normalized = content_type.lower()
    return normalized.startswith("text/") or any(
        marker in normalized
        for marker in (
            "application/xml",
            "application/yaml",
            "application/x-yaml",
            "application/javascript",
        )
    )


async def text_result_or_sandbox(
    *,
    text: str,
    sandbox_url: str | None,
    chat_id: str,
    file_name: str,
    description: str,
    inline_max_bytes: int = DEFAULT_INLINE_TEXT_MAX_BYTES,
) -> ToolResult:
    """Return small text inline; save large text to the sandbox when available."""
    result_size = len(text.encode("utf-8"))
    if result_size <= inline_max_bytes:
        return ToolResult(content=[{"type": "text", "text": text}])

    if not sandbox_url:
        return ToolResult(
            content=[
                {
                    "type": "text",
                    "text": (
                        f"{description} ({result_size / 1024:.0f} KB), which is too "
                        "large to include inline, and no sandbox is available to save "
                        "it. Narrow the request or ask for an export."
                    ),
                }
            ],
            is_error=True,
        )

    return await write_text_to_sandbox(
        sandbox_url,
        text,
        file_name,
        chat_id,
        message=(
            f"{description} ({result_size / 1024:.0f} KB), so I saved it to "
            f"workspace: {file_name}. Use read_file, jq, or Python in the sandbox "
            "to inspect/analyze it."
        ),
    )


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

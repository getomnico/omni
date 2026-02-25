"""DocumentToolHandler: unified handler for reading/fetching documents."""

from __future__ import annotations

import base64
import logging
from typing import Union

import httpx

from db.documents import DocumentsRepository
from storage import ContentStorage, PostgresContentStorage
from tools.registry import ToolContext, ToolResult

logger = logging.getLogger(__name__)

# Content types considered binary (not extracted text)
BINARY_CONTENT_TYPES = {
    "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet",
    "application/vnd.ms-excel",
    "application/vnd.google-apps.spreadsheet",
    "application/pdf",
    "application/vnd.openxmlformats-officedocument.wordprocessingml.document",
    "application/vnd.google-apps.document",
    "application/vnd.openxmlformats-officedocument.presentationml.presentation",
    "application/vnd.google-apps.presentation",
    "image/png",
    "image/jpeg",
    "image/gif",
    "image/webp",
    "application/zip",
    "application/octet-stream",
}

# Max text size to return directly in LLM context (characters)
DIRECT_RETURN_THRESHOLD = 32_000

DOCUMENT_TOOL = {
    "name": "read_document",
    "description": (
        "Read a document's content. For text documents, returns content directly or saves to sandbox if large. "
        "For binary files (spreadsheets, PDFs, etc.), fetches the actual file from the source and saves to sandbox workspace."
    ),
    "input_schema": {
        "type": "object",
        "properties": {
            "id": {
                "type": "string",
                "description": "The document ID (from search results)",
            },
            "name": {
                "type": "string",
                "description": "The document name",
            },
            "start_line": {
                "type": "integer",
                "description": "Optional: start line number (inclusive) for partial text reads",
            },
            "end_line": {
                "type": "integer",
                "description": "Optional: end line number (inclusive) for partial text reads",
            },
        },
        "required": ["id", "name"],
    },
}

_TOOL_NAMES = {"read_document"}


class DocumentToolHandler:
    """Unified handler for reading text documents and fetching binary files."""

    def __init__(
        self,
        content_storage: Union[ContentStorage, PostgresContentStorage, None] = None,
        documents_repo: DocumentsRepository | None = None,
        sandbox_url: str | None = None,
        connector_manager_url: str | None = None,
    ) -> None:
        self._content_storage = content_storage
        self._documents_repo = documents_repo
        self._sandbox_url = sandbox_url.rstrip("/") if sandbox_url else None
        self._connector_manager_url = (
            connector_manager_url.rstrip("/") if connector_manager_url else None
        )

    def get_tools(self) -> list[dict]:
        return [DOCUMENT_TOOL]

    def can_handle(self, tool_name: str) -> bool:
        return tool_name in _TOOL_NAMES

    def requires_approval(self, tool_name: str) -> bool:
        return False  # read-only operation

    async def execute(
        self, tool_name: str, tool_input: dict, context: ToolContext
    ) -> ToolResult:
        if tool_name != "read_document":
            return ToolResult(
                content=[{"type": "text", "text": f"Unknown tool: {tool_name}"}],
                is_error=True,
            )

        document_id = tool_input.get("id")
        document_name = tool_input.get("name", document_id)
        start_line = tool_input.get("start_line")
        end_line = tool_input.get("end_line")

        if not document_id:
            return ToolResult(
                content=[{"type": "text", "text": "Missing required parameter: id"}],
                is_error=True,
            )

        try:
            # Look up document metadata
            doc = await self._documents_repo.get_by_id(document_id)
            if doc is None:
                return ToolResult(
                    content=[
                        {
                            "type": "text",
                            "text": f"Document not found: {document_id}",
                        }
                    ],
                    is_error=True,
                )

            # Determine if this is a binary file
            is_binary = doc.content_type in BINARY_CONTENT_TYPES

            if is_binary and self._connector_manager_url and doc.source_id:
                return await self._fetch_binary(doc, document_name, context)
            else:
                return await self._read_text(
                    doc, document_name, start_line, end_line, context
                )

        except Exception as e:
            logger.error(f"read_document failed: {e}", exc_info=True)
            return ToolResult(
                content=[{"type": "text", "text": f"read_document error: {e}"}],
                is_error=True,
            )

    async def _fetch_binary(
        self, doc, document_name: str, context: ToolContext
    ) -> ToolResult:
        """Fetch binary file from source via connector-manager and write to sandbox."""
        logger.info(
            f"Fetching binary file '{document_name}' (id={doc.id}) from source {doc.source_id}"
        )

        async with httpx.AsyncClient(timeout=120.0) as client:
            # Call connector-manager to fetch the file
            resp = await client.post(
                f"{self._connector_manager_url}/action",
                json={
                    "source_id": doc.source_id,
                    "action": "fetch_file",
                    "params": {"document_id": doc.id},
                },
            )
            resp.raise_for_status()

            content_type = resp.headers.get("content-type", "")

            if "application/json" in content_type:
                # The connector returned a JSON error
                result = resp.json()
                error = result.get("error", "Unknown error")
                return ToolResult(
                    content=[
                        {
                            "type": "text",
                            "text": f"Failed to fetch file: {error}",
                        }
                    ],
                    is_error=True,
                )

            binary_data = resp.content
            file_name = resp.headers.get("x-file-name", document_name)

            # Base64 encode and write to sandbox
            encoded = base64.b64encode(binary_data).decode("ascii")
            size_kb = len(binary_data) / 1024

            write_resp = await client.post(
                f"{self._sandbox_url}/files/write_binary",
                json={
                    "path": file_name,
                    "content_base64": encoded,
                    "chat_id": context.chat_id,
                },
            )
            write_resp.raise_for_status()

        return ToolResult(
            content=[
                {
                    "type": "text",
                    "text": f"File saved to workspace: {file_name} ({size_kb:.0f} KB)",
                }
            ],
        )

    async def _read_text(
        self,
        doc,
        document_name: str,
        start_line: int | None,
        end_line: int | None,
        context: ToolContext,
    ) -> ToolResult:
        """Read text document content, returning directly or writing to sandbox."""
        if not doc.content_id:
            return ToolResult(
                content=[
                    {
                        "type": "text",
                        "text": f"Document '{document_name}' has no text content available.",
                    }
                ],
                is_error=True,
            )

        content = await self._content_storage.get_text(doc.content_id)

        # Apply line range if specified
        if start_line is not None or end_line is not None:
            lines = content.split("\n")
            start = (start_line or 1) - 1  # Convert to 0-indexed
            end = end_line or len(lines)
            content = "\n".join(lines[start:end])

        # Size check: return directly or write to sandbox
        if len(content) <= DIRECT_RETURN_THRESHOLD:
            return ToolResult(
                content=[{"type": "text", "text": content}],
            )

        # Large text: write to sandbox
        if self._sandbox_url:
            # Determine a reasonable filename
            file_name = document_name or doc.title or f"document_{doc.id}.txt"
            if "." not in file_name:
                file_name += ".txt"

            async with httpx.AsyncClient(timeout=60.0) as client:
                resp = await client.post(
                    f"{self._sandbox_url}/files/write",
                    json={
                        "path": file_name,
                        "content": content,
                        "chat_id": context.chat_id,
                    },
                )
                resp.raise_for_status()

            size_kb = len(content.encode("utf-8")) / 1024
            return ToolResult(
                content=[
                    {
                        "type": "text",
                        "text": f"Document saved to workspace: {file_name} ({size_kb:.1f} KB). Use read_file or run_python to process it.",
                    }
                ],
            )

        # No sandbox available, return truncated content
        truncated = content[:DIRECT_RETURN_THRESHOLD]
        return ToolResult(
            content=[
                {
                    "type": "text",
                    "text": f"{truncated}\n\n... (truncated, {len(content)} total characters)",
                }
            ],
        )

"""SandboxToolHandler: provides file and code execution tools via the sandbox sidecar."""

from __future__ import annotations

import json
import logging

import httpx

from db.documents import DocumentsRepository
from storage import ContentStorage, PostgresContentStorage
from tools.registry import ToolContext, ToolResult

logger = logging.getLogger(__name__)

SANDBOX_TOOLS = [
    {
        "name": "write_file",
        "description": "Write content to a file in the scratch workspace. Use this to save data, create scripts, or prepare files for processing.",
        "input_schema": {
            "type": "object",
            "properties": {
                "path": {
                    "type": "string",
                    "description": "Relative file path within the scratch workspace (e.g., 'data.csv', 'scripts/process.py')",
                },
                "content": {
                    "type": "string",
                    "description": "The content to write to the file",
                },
            },
            "required": ["path", "content"],
        },
    },
    {
        "name": "read_file",
        "description": "Read content from a file in the scratch workspace.",
        "input_schema": {
            "type": "object",
            "properties": {
                "path": {
                    "type": "string",
                    "description": "Relative file path within the scratch workspace",
                },
            },
            "required": ["path"],
        },
    },
    {
        "name": "run_bash",
        "description": "Run a bash command in the scratch workspace. Use for file operations, data processing with standard unix tools, etc.",
        "input_schema": {
            "type": "object",
            "properties": {
                "command": {
                    "type": "string",
                    "description": "The bash command to execute",
                },
            },
            "required": ["command"],
        },
    },
    {
        "name": "run_python",
        "description": "Run Python code in the scratch workspace. Pre-installed libraries: pandas, numpy, openpyxl, json, csv. Use for data analysis, processing, and transformation.",
        "input_schema": {
            "type": "object",
            "properties": {
                "code": {
                    "type": "string",
                    "description": "The Python code to execute",
                },
            },
            "required": ["code"],
        },
    },
]

COPY_TO_SANDBOX_TOOL = {
    "name": "copy_to_sandbox",
    "description": (
        "Copy a document directly from storage into the sandbox workspace. "
        "Use this to efficiently get a full document into the workspace for processing "
        "with run_python or run_bash, instead of reading it into the conversation first. "
        "Note: this copies the extracted text content, not the original binary format."
    ),
    "input_schema": {
        "type": "object",
        "properties": {
            "document_id": {
                "type": "string",
                "description": "The document ID (from search results) to copy",
            },
            "path": {
                "type": "string",
                "description": "Relative file path in the workspace to write to (e.g., 'data.csv', 'report.txt')",
            },
            "document_name": {
                "type": "string",
                "description": "Optional human-readable name for logging purposes",
            },
        },
        "required": ["document_id", "path"],
    },
}

_TOOL_NAMES = {"write_file", "read_file", "run_bash", "run_python", "copy_to_sandbox"}


class SandboxToolHandler:
    """Dispatches sandbox tool calls to the sidecar service."""

    def __init__(
        self,
        sandbox_url: str,
        content_storage: ContentStorage | PostgresContentStorage | None = None,
        documents_repo: DocumentsRepository | None = None,
    ) -> None:
        self._sandbox_url = sandbox_url.rstrip("/")
        self._content_storage = content_storage
        self._documents_repo = documents_repo

    def get_tools(self) -> list[dict]:
        tools = list(SANDBOX_TOOLS)
        if self._content_storage and self._documents_repo:
            tools.append(COPY_TO_SANDBOX_TOOL)
        return tools

    def can_handle(self, tool_name: str) -> bool:
        return tool_name in _TOOL_NAMES

    def requires_approval(self, tool_name: str) -> bool:
        return (
            False  # No approval needed — sandbox only affects ephemeral scratch space
        )

    async def _execute_copy_to_sandbox(
        self, tool_input: dict, context: ToolContext
    ) -> ToolResult:
        """Copy a document from storage directly into the sandbox filesystem."""
        document_id = tool_input["document_id"]
        path = tool_input["path"]
        document_name = tool_input.get("document_name", document_id)

        try:
            # 1. Look up the document to get its content_id
            doc = await self._documents_repo.get_by_id(document_id)
            if doc is None:
                return ToolResult(
                    content=[
                        {"type": "text", "text": f"Document not found: {document_id}"}
                    ],
                    is_error=True,
                )

            if not doc.content_id:
                return ToolResult(
                    content=[
                        {
                            "type": "text",
                            "text": f"Document '{document_name}' has no extracted text content available.",
                        }
                    ],
                    is_error=True,
                )

            # 2. Fetch the full text content from storage
            content = await self._content_storage.get_text(doc.content_id)

            # 3. Write the content to the sandbox filesystem
            async with httpx.AsyncClient(timeout=60.0) as client:
                resp = await client.post(
                    f"{self._sandbox_url}/files/write",
                    json={
                        "path": path,
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
                        "text": f"Copied '{document_name}' to {path} ({size_kb:.1f} KB)",
                    }
                ],
            )

        except ValueError as e:
            return ToolResult(
                content=[
                    {"type": "text", "text": f"Failed to read document content: {e}"}
                ],
                is_error=True,
            )
        except httpx.TimeoutException:
            return ToolResult(
                content=[{"type": "text", "text": "Timed out writing file to sandbox"}],
                is_error=True,
            )
        except Exception as e:
            logger.error(f"copy_to_sandbox failed: {e}")
            return ToolResult(
                content=[{"type": "text", "text": f"copy_to_sandbox error: {e}"}],
                is_error=True,
            )

    async def execute(
        self, tool_name: str, tool_input: dict, context: ToolContext
    ) -> ToolResult:
        if tool_name == "copy_to_sandbox":
            return await self._execute_copy_to_sandbox(tool_input, context)

        try:
            async with httpx.AsyncClient(timeout=60.0) as client:
                if tool_name == "write_file":
                    resp = await client.post(
                        f"{self._sandbox_url}/files/write",
                        json={
                            "path": tool_input["path"],
                            "content": tool_input["content"],
                            "chat_id": context.chat_id,
                        },
                    )
                elif tool_name == "read_file":
                    resp = await client.post(
                        f"{self._sandbox_url}/files/read",
                        json={
                            "path": tool_input["path"],
                            "chat_id": context.chat_id,
                        },
                    )
                elif tool_name == "run_bash":
                    resp = await client.post(
                        f"{self._sandbox_url}/execute/bash",
                        json={
                            "command": tool_input["command"],
                            "chat_id": context.chat_id,
                        },
                    )
                elif tool_name == "run_python":
                    resp = await client.post(
                        f"{self._sandbox_url}/execute/python",
                        json={
                            "code": tool_input["code"],
                            "chat_id": context.chat_id,
                        },
                    )
                else:
                    return ToolResult(
                        content=[
                            {
                                "type": "text",
                                "text": f"Unknown sandbox tool: {tool_name}",
                            }
                        ],
                        is_error=True,
                    )

                resp.raise_for_status()
                result = resp.json()

        except httpx.TimeoutException:
            return ToolResult(
                content=[{"type": "text", "text": "Execution timed out"}],
                is_error=True,
            )
        except Exception as e:
            logger.error(f"Sandbox tool {tool_name} failed: {e}")
            return ToolResult(
                content=[{"type": "text", "text": f"Sandbox error: {str(e)}"}],
                is_error=True,
            )

        # Format the result
        if tool_name in ("write_file", "read_file"):
            return ToolResult(
                content=[{"type": "text", "text": result.get("content", "")}],
            )
        else:
            # Execution result with stdout/stderr
            output_parts = []
            if result.get("stdout"):
                output_parts.append(f"stdout:\n{result['stdout']}")
            if result.get("stderr"):
                output_parts.append(f"stderr:\n{result['stderr']}")
            if not output_parts:
                output_parts.append("(no output)")

            text = "\n\n".join(output_parts)
            is_error = result.get("exit_code", 0) != 0

            return ToolResult(
                content=[{"type": "text", "text": text}],
                is_error=is_error,
            )

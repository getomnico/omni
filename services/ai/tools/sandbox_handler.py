"""SandboxToolHandler: provides file and code execution tools via the sandbox sidecar."""

from __future__ import annotations

import json
import logging
import re

import httpx
from anthropic.types import ToolParam

from tools.registry import ToolContext, ToolResult

logger = logging.getLogger(__name__)

SANDBOX_TOOLS: list[ToolParam] = [
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
                "start_line": {
                    "type": "integer",
                    "description": "1-based start line number (default: 1)",
                },
                "end_line": {
                    "type": "integer",
                    "description": "1-based end line number, inclusive (default: last line)",
                },
            },
            "required": ["path"],
        },
    },
    {
        "name": "edit_file",
        "description": "Make an exact string replacement in an existing text file in the scratch workspace. Prefer this over write_file when modifying an existing file — it preserves the rest of the content. old_string must match the file content exactly (including whitespace). If it matches multiple locations, either include more surrounding context to make it unique or pass replace_all=true.",
        "input_schema": {
            "type": "object",
            "properties": {
                "path": {
                    "type": "string",
                    "description": "Relative file path of the existing file within the scratch workspace",
                },
                "old_string": {
                    "type": "string",
                    "description": "The exact text to replace",
                },
                "new_string": {
                    "type": "string",
                    "description": "The replacement text",
                },
                "replace_all": {
                    "type": "boolean",
                    "description": "Replace every occurrence of old_string instead of failing when it matches multiple locations (default: false)",
                },
            },
            "required": ["path", "old_string", "new_string"],
        },
    },
    {
        "name": "run_bash",
        "description": "Run a bash command in the scratch workspace. The `excel` CLI is available for spreadsheet operations (run `excel --help` for usage). Use for file operations, data processing with standard unix tools, etc.",
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
        "description": "Run Python code in the scratch workspace. Pre-installed libraries: pandas, numpy, openpyxl, xlsxwriter, matplotlib, seaborn, python-docx, python-pptx, reportlab, pypdf, json, csv. Use for data analysis, processing, transformation, visualization, and creating office files (docx via python-docx, pptx via python-pptx, xlsx via openpyxl/xlsxwriter) and PDFs (via reportlab).",
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
    {
        "name": "present_artifact",
        "description": "Present a generated file to the user so they can view or download it. The file must already exist in the scratch workspace; without calling this tool, users cannot see files you generate. Supported types and how they are shown: images (PNG, JPEG, etc.) render inline in the chat; PDF, Word (.docx), Excel (.xlsx), Markdown (.md) and HTML files open in a viewer pane on the right of the chat; other file types appear as a downloadable card. For rich output such as dashboards or landing pages, write an HTML file and present it. For written documents or notes, prefer Markdown. Make HTML files self-contained (inline CSS/JS) or reference assets by absolute URL so they render correctly in the viewer.",
        "input_schema": {
            "type": "object",
            "properties": {
                "path": {
                    "type": "string",
                    "description": "Relative file path within the scratch workspace (e.g., 'chart.png', 'output.xlsx')",
                },
                "title": {
                    "type": "string",
                    "description": "A short, descriptive title for the artifact (e.g., 'Sales Chart Q4')",
                },
            },
            "required": ["path", "title"],
        },
    },
]

_TOOL_NAMES = {"write_file", "read_file", "edit_file", "run_bash", "run_python", "present_artifact"}
_UNSAFE_CONTROL_CHARS_RE = re.compile(r"[\x00-\x08\x0b\x0c\x0e-\x1f\x7f]")


def _sanitize_tool_text(text: str) -> str:
    return _UNSAFE_CONTROL_CHARS_RE.sub(
        lambda match: f"\\x{ord(match.group(0)):02x}",
        text,
    )


class SandboxToolHandler:
    """Dispatches sandbox tool calls to the sidecar service."""

    def __init__(self, sandbox_url: str) -> None:
        self._sandbox_url = sandbox_url.rstrip("/")

    def get_tools(self) -> list[ToolParam]:
        return list(SANDBOX_TOOLS)

    def can_handle(self, tool_name: str) -> bool:
        return tool_name in _TOOL_NAMES

    def requires_approval(self, tool_name: str) -> bool:
        return (
            False  # No approval needed — sandbox only affects ephemeral scratch space
        )

    async def execute(
        self, tool_name: str, tool_input: dict, context: ToolContext
    ) -> ToolResult:

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
                    body = {
                        "path": tool_input["path"],
                        "chat_id": context.chat_id,
                        "start_line": tool_input.get("start_line"),
                        "end_line": tool_input.get("end_line"),
                    }
                    resp = await client.post(
                        f"{self._sandbox_url}/files/read",
                        json={k: v for k, v in body.items() if v is not None},
                    )
                elif tool_name == "edit_file":
                    resp = await client.post(
                        f"{self._sandbox_url}/files/edit",
                        json={
                            "path": tool_input["path"],
                            "old_string": tool_input["old_string"],
                            "new_string": tool_input["new_string"],
                            "replace_all": tool_input.get("replace_all", False),
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
                elif tool_name == "present_artifact":
                    # Stat the file to verify it exists and get metadata
                    resp = await client.post(
                        f"{self._sandbox_url}/files/stat",
                        json={
                            "path": tool_input["path"],
                            "chat_id": context.chat_id,
                        },
                    )
                    if resp.status_code != 200:
                        try:
                            error_msg = resp.json().get("detail", resp.text)
                        except Exception:
                            error_msg = resp.text
                        return ToolResult(
                            content=[{"type": "text", "text": error_msg}],
                            is_error=True,
                        )
                    stat = resp.json()

                    if not stat.get("exists"):
                        return ToolResult(
                            content=[
                                {
                                    "type": "text",
                                    "text": f"File not found: {tool_input['path']}",
                                }
                            ],
                            is_error=True,
                        )

                    artifact_url = f"/api/chat/{context.chat_id}/artifacts/{tool_input['path']}"
                    # Pin the artifact to the committed version it was presented
                    # at, so later edits never change what this card shows.
                    version = stat.get("version")
                    if version:
                        artifact_url = f"{artifact_url}?v={version}"
                    artifact_info = {
                        "url": artifact_url,
                        "title": tool_input["title"],
                        "content_type": stat["content_type"],
                        "size_bytes": stat["size_bytes"],
                        "version": version,
                    }
                    return ToolResult(
                        content=[
                            {
                                "type": "text",
                                "text": json.dumps(artifact_info),
                            }
                        ],
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

                if resp.status_code != 200:
                    try:
                        error_msg = resp.json().get("detail", resp.text)
                    except Exception:
                        error_msg = resp.text
                    return ToolResult(
                        content=[{"type": "text", "text": error_msg}],
                        is_error=True,
                    )
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
        if tool_name in ("write_file", "read_file", "edit_file"):
            return ToolResult(
                content=[
                    {
                        "type": "text",
                        "text": _sanitize_tool_text(result.get("content", "")),
                    }
                ],
            )
        else:
            # Execution result with stdout/stderr
            output_parts = []
            if result.get("stdout"):
                output_parts.append(f"stdout:\n{_sanitize_tool_text(result['stdout'])}")
            if result.get("stderr"):
                output_parts.append(f"stderr:\n{_sanitize_tool_text(result['stderr'])}")
            if not output_parts:
                output_parts.append("(no output)")

            text = "\n\n".join(output_parts)
            is_error = result.get("exit_code", 0) != 0

            return ToolResult(
                content=[{"type": "text", "text": text}],
                is_error=is_error,
            )

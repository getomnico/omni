"""SandboxToolHandler: provides file and code execution tools via the sandbox sidecar."""

from __future__ import annotations

import json
import logging
import re
from pathlib import PurePosixPath
from typing import NotRequired, TypedDict

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
        "name": "build_component",
        "description": "Compile a Svelte component source file into one self-contained HTML artifact using Omni's fixed, preinstalled component SDK. Create or edit the .svelte source with write_file/edit_file first. The compiler owns Vite configuration and dependencies; do not create config files or install packages. The output can be passed to present_artifact with display_mode='inline'.",
        "input_schema": {
            "type": "object",
            "properties": {
                "source_path": {
                    "type": "string",
                    "description": "Relative .svelte entry file in the scratch workspace (for example, 'components/App.svelte').",
                },
                "output_path": {
                    "type": "string",
                    "description": "Relative .html output path in the scratch workspace (for example, 'components/revenue.html').",
                },
            },
            "required": ["source_path", "output_path"],
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
        "description": "Present a generated file to the user so they can view or download it. The file must already exist in the scratch workspace; without calling this tool, users cannot see files you generate. Images render inline in chat by default. PDF, Word (.docx), Excel (.xlsx), Markdown (.md), and HTML files open in the viewer pane by default. To render an interactive chart or arbitrary component inline, create a self-contained HTML file and set display_mode='inline'. Inline HTML runs in a security-sandboxed iframe with scripts enabled. Other file types appear as downloadable cards. For written documents or notes, prefer Markdown. Make HTML files self-contained (inline CSS/JS, including library code) or reference assets by absolute URL.",
        "input_schema": {
            "type": "object",
            "properties": {
                "path": {
                    "type": "string",
                    "description": "Relative file path within the scratch workspace (e.g., 'chart.html', 'output.xlsx')",
                },
                "title": {
                    "type": "string",
                    "description": "A short, descriptive title for the artifact (e.g., 'Sales Chart Q4')",
                },
                "display_mode": {
                    "type": "string",
                    "enum": ["inline", "panel"],
                    "description": "Optional presentation override. Use 'inline' for an HTML interactive chart or component; omit it to use the default for the file type.",
                },
                "inline_height": {
                    "type": "integer",
                    "minimum": 160,
                    "maximum": 800,
                    "description": "Optional inline frame height in pixels (default: 420). Used only for inline HTML.",
                },
            },
            "required": ["path", "title"],
        },
    },
]

_TOOL_NAMES = {
    "write_file",
    "read_file",
    "edit_file",
    "build_component",
    "run_bash",
    "run_python",
    "present_artifact",
}


class ComponentBuildResult(TypedDict):
    path: str
    size_bytes: int
    content_type: str
    version: NotRequired[str | None]


class FileStatResult(TypedDict):
    path: str
    size_bytes: int
    content_type: str
    exists: bool
    version: str | None


def _component_path(value: object, extension: str, name: str) -> str | None:
    if not isinstance(value, str) or not value or len(value) > 256:
        return f"{name} must be a non-empty relative path of at most 256 characters."
    if "\\" in value or "\x00" in value:
        return f"{name} must use relative POSIX path segments."
    parts = value.split("/")
    if any(part in ("", ".", "..") for part in parts) or PurePosixPath(value).is_absolute():
        return f"{name} must not contain empty, '.', or '..' path segments."
    if not value.endswith(extension):
        return f"{name} must end in {extension}."
    return None


def _relative_sandbox_path(value: object, name: str) -> str | None:
    if not isinstance(value, str) or not value or len(value) > 256:
        return f"{name} must be a non-empty relative path of at most 256 characters."
    if "\\" in value or "\x00" in value:
        return f"{name} must use relative POSIX path segments."
    parts = value.split("/")
    if any(part in ("", ".", "..") for part in parts) or PurePosixPath(value).is_absolute():
        return f"{name} must not contain empty, '.', or '..' path segments."
    return None


def _build_component_input(tool_input: dict) -> tuple[str, str] | str:
    source_path = tool_input.get("source_path")
    output_path = tool_input.get("output_path")
    for value, extension, name in (
        (source_path, ".svelte", "source_path"),
        (output_path, ".html", "output_path"),
    ):
        error = _component_path(value, extension, name)
        if error:
            return error
    assert isinstance(source_path, str)
    assert isinstance(output_path, str)
    if source_path == output_path:
        return "source_path and output_path must be different files."
    return source_path, output_path


def _parse_component_build_result(payload: object) -> ComponentBuildResult | None:
    if not isinstance(payload, dict):
        return None
    path = payload.get("path")
    size_bytes = payload.get("size_bytes")
    content_type = payload.get("content_type")
    version = payload.get("version")
    if (
        not isinstance(path, str)
        or not isinstance(size_bytes, int)
        or isinstance(size_bytes, bool)
        or size_bytes < 0
        or content_type != "text/html"
        or (version is not None and not isinstance(version, str))
    ):
        return None
    result: ComponentBuildResult = {
        "path": path,
        "size_bytes": size_bytes,
        "content_type": content_type,
    }
    if version is not None:
        result["version"] = version
    return result


def _parse_file_stat(payload: object) -> FileStatResult | None:
    if not isinstance(payload, dict):
        return None
    path = payload.get("path")
    size_bytes = payload.get("size_bytes")
    content_type = payload.get("content_type")
    exists = payload.get("exists")
    version = payload.get("version")
    if (
        not isinstance(path, str)
        or not isinstance(size_bytes, int)
        or isinstance(size_bytes, bool)
        or size_bytes < 0
        or not isinstance(content_type, str)
        or not isinstance(exists, bool)
        or (version is not None and not isinstance(version, str))
    ):
        return None
    return {
        "path": path,
        "size_bytes": size_bytes,
        "content_type": content_type,
        "exists": exists,
        "version": version,
    }


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
            if tool_name == "build_component":
                component_input = _build_component_input(tool_input)
                if isinstance(component_input, str):
                    return ToolResult(
                        content=[{"type": "text", "text": component_input}],
                        is_error=True,
                    )
            elif tool_name == "present_artifact":
                path_error = _relative_sandbox_path(tool_input.get("path"), "path")
                title = tool_input.get("title")
                display_mode = tool_input.get("display_mode")
                inline_height = tool_input.get("inline_height")
                if path_error:
                    return ToolResult(content=[{"type": "text", "text": path_error}], is_error=True)
                if not isinstance(title, str) or not title or len(title) > 200:
                    return ToolResult(
                        content=[{"type": "text", "text": "title must be a non-empty string of at most 200 characters."}],
                        is_error=True,
                    )
                if display_mode not in (None, "inline", "panel"):
                    return ToolResult(content=[{"type": "text", "text": "display_mode must be 'inline' or 'panel'."}], is_error=True)
                if inline_height is not None and (
                    not isinstance(inline_height, int)
                    or isinstance(inline_height, bool)
                    or not 160 <= inline_height <= 800
                ):
                    return ToolResult(
                        content=[{"type": "text", "text": "inline_height must be an integer between 160 and 800."}],
                        is_error=True,
                    )

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
                elif tool_name == "build_component":
                    source_path, output_path = component_input
                    resp = await client.post(
                        f"{self._sandbox_url}/components/build",
                        json={
                            "source_path": source_path,
                            "output_path": output_path,
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
                    stat = _parse_file_stat(resp.json())
                    if stat is None or stat["path"] != tool_input["path"]:
                        return ToolResult(
                            content=[
                                {
                                    "type": "text",
                                    "text": "Sandbox returned invalid file metadata.",
                                }
                            ],
                            is_error=True,
                        )

                    if not stat["exists"]:
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
                    version = stat["version"]
                    if version:
                        artifact_url = f"{artifact_url}?v={version}"
                    display_mode = tool_input.get("display_mode")
                    inline_height = tool_input.get("inline_height")
                    content_type = stat["content_type"]
                    if display_mode not in (None, "inline", "panel"):
                        return ToolResult(
                            content=[
                                {
                                    "type": "text",
                                    "text": "display_mode must be 'inline' or 'panel'.",
                                }
                            ],
                            is_error=True,
                        )
                    if display_mode == "inline" and not (
                        content_type.startswith("image/") or content_type == "text/html"
                    ):
                        return ToolResult(
                            content=[
                                {
                                    "type": "text",
                                    "text": (
                                        "Inline artifacts must be an image or an HTML file. "
                                        "Use an HTML wrapper for an interactive component."
                                    ),
                                }
                            ],
                            is_error=True,
                        )
                    if inline_height is not None and (
                        not isinstance(inline_height, int)
                        or isinstance(inline_height, bool)
                        or not 160 <= inline_height <= 800
                    ):
                        return ToolResult(
                            content=[
                                {
                                    "type": "text",
                                    "text": "inline_height must be an integer between 160 and 800.",
                                }
                            ],
                            is_error=True,
                        )

                    artifact_info = {
                        "url": artifact_url,
                        "title": tool_input["title"],
                        "content_type": content_type,
                        "size_bytes": stat["size_bytes"],
                        "version": version,
                    }
                    if display_mode is not None:
                        artifact_info["display_mode"] = display_mode
                    if inline_height is not None:
                        artifact_info["inline_height"] = inline_height
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

                if tool_name == "build_component":
                    component_result = _parse_component_build_result(result)
                    if component_result is None or component_result["path"] != output_path:
                        return ToolResult(
                            content=[
                                {
                                    "type": "text",
                                    "text": "Sandbox returned invalid component build metadata.",
                                }
                            ],
                            is_error=True,
                        )
                    return ToolResult(
                        content=[{"type": "text", "text": json.dumps(component_result)}],
                    )

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

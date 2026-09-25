"""Lightweight MCP server configuration shared by MCP and non-MCP connectors."""

from __future__ import annotations

from dataclasses import dataclass, field

MCP_AUTH_STATUS_FILE_ENV = "OMNI_MCP_AUTH_STATUS_FILE"
MCP_AUTH_REQUIRED_MESSAGE = "MCP authentication required"


@dataclass(frozen=True)
class StdioMcpServer:
    """Configuration for an MCP server reached via stdio (subprocess)."""

    command: str
    args: list[str] = field(default_factory=list)
    env: dict[str, str] | None = None
    cwd: str | None = None


@dataclass(frozen=True)
class HttpMcpServer:
    """Configuration for a remote MCP server reached via Streamable HTTP."""

    url: str
    headers: dict[str, str] = field(default_factory=dict)
    timeout_seconds: float = 30.0
    sse_read_timeout_seconds: float = 300.0


McpServer = StdioMcpServer | HttpMcpServer

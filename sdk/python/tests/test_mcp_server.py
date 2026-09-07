"""A minimal MCP server used as a subprocess for testing the SDK's adapter.

Supports both transports:

- ``python test_mcp_server.py``                     # stdio (default)
- ``python test_mcp_server.py http <port>``         # Streamable HTTP
"""

import sys

import anyio
from mcp import types
from mcp.server.fastmcp import FastMCP
from mcp.server.lowlevel import Server
from mcp.server.stdio import stdio_server
from mcp.types import ToolAnnotations

server = FastMCP("test")


@server.tool(annotations=ToolAnnotations(readOnlyHint=True))
def greet(name: str) -> str:
    """Greet someone by name."""
    return f"Hello, {name}!"


@server.tool()
def add(a: int, b: int) -> str:
    """Add two numbers."""
    return str(a + b)


@server.resource("test://item/{item_id}")
def get_item(item_id: str) -> str:
    """Get an item by ID."""
    return f"Item {item_id}"


@server.prompt()
def summarize(text: str) -> str:
    """Summarize the given text."""
    return f"Please summarize: {text}"


tools_only_server = Server("tools-only")


@tools_only_server.list_tools()
async def list_tools() -> list[types.Tool]:
    return [
        types.Tool(
            name="greet",
            description="Greet someone",
            inputSchema={
                "type": "object",
                "properties": {"name": {"type": "string"}},
                "required": ["name"],
            },
            annotations=ToolAnnotations(readOnlyHint=True),
        )
    ]


@tools_only_server.call_tool()
async def call_tool(name: str, arguments: dict[str, object]) -> list[types.TextContent]:
    return [types.TextContent(type="text", text=f"Hello, {arguments['name']}!")]


async def run_tools_only_server() -> None:
    async with stdio_server() as (read_stream, write_stream):
        await tools_only_server.run(
            read_stream,
            write_stream,
            tools_only_server.create_initialization_options(),
        )


if __name__ == "__main__":
    if len(sys.argv) > 1 and sys.argv[1] == "http":
        port = int(sys.argv[2]) if len(sys.argv) > 2 else 8765
        server.settings.host = "127.0.0.1"
        server.settings.port = port
        server.run(transport="streamable-http")
    elif len(sys.argv) > 1 and sys.argv[1] == "tools-only":
        anyio.run(run_tools_only_server)
    else:
        server.run(transport="stdio")

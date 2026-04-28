"""A minimal MCP server used as a subprocess for testing the SDK's adapter.

Supports both transports:

- ``python test_mcp_server.py``                     # stdio (default)
- ``python test_mcp_server.py http <port>``         # Streamable HTTP
"""

import sys

from mcp.server.fastmcp import FastMCP
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


if __name__ == "__main__":
    if len(sys.argv) > 1 and sys.argv[1] == "http":
        port = int(sys.argv[2]) if len(sys.argv) > 2 else 8765
        server.settings.host = "127.0.0.1"
        server.settings.port = port
        server.run(transport="streamable-http")
    else:
        server.run(transport="stdio")

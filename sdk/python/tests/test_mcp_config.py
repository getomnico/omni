from __future__ import annotations

import subprocess
import sys


def test_public_mcp_configs_import_without_the_optional_mcp_dependency() -> None:
    script = """
import importlib.abc
import sys

class BlockMcpImports(importlib.abc.MetaPathFinder):
    def find_spec(self, fullname, path=None, target=None):
        if fullname == "mcp" or fullname.startswith("mcp."):
            raise ModuleNotFoundError("MCP intentionally unavailable", name="mcp")
        return None

sys.meta_path.insert(0, BlockMcpImports())
from typing import get_args
from omni_connector import HttpMcpServer, McpServer, StdioMcpServer

assert set(get_args(McpServer)) == {StdioMcpServer, HttpMcpServer}
stdio = StdioMcpServer(command="example")
http = HttpMcpServer(url="https://example.test/mcp")
assert isinstance(stdio, StdioMcpServer)
assert isinstance(http, HttpMcpServer)
assert "omni_connector.mcp_adapter" not in sys.modules
"""
    subprocess.run(
        [sys.executable, "-c", script],
        check=True,
        capture_output=True,
        text=True,
    )

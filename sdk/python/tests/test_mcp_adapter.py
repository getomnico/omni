"""Tests for the MCP adapter (stdio + Streamable HTTP transports)."""

import asyncio
import os
import socket
import subprocess
import sys
from typing import Any

import httpx
import pytest

from omni_connector import Connector, HttpMcpServer, StdioMcpServer
from omni_connector.mcp_adapter import McpAdapter

# Path to the test MCP server script (supports both stdio and http modes)
TEST_SERVER = os.path.join(os.path.dirname(__file__), "test_mcp_server.py")
TEST_STDIO_SERVER = StdioMcpServer(command=sys.executable, args=[TEST_SERVER])
# Dummy env to simulate having credentials (test server doesn't need real ones)
TEST_ENV: dict[str, str] = {"TEST_MODE": "1"}


def _free_port() -> int:
    with socket.socket(socket.AF_INET, socket.SOCK_STREAM) as s:
        s.bind(("127.0.0.1", 0))
        return s.getsockname()[1]


@pytest.fixture
async def http_server_url():
    """Spawn the test MCP server in Streamable HTTP mode on a random port."""
    port = _free_port()
    proc = subprocess.Popen(
        [sys.executable, TEST_SERVER, "http", str(port)],
        stdout=subprocess.DEVNULL,
        stderr=subprocess.DEVNULL,
    )
    url = f"http://127.0.0.1:{port}/mcp"

    async def wait_ready():
        async with httpx.AsyncClient() as client:
            for _ in range(80):
                try:
                    # 4xx response means the server is up; the MCP endpoint
                    # rejects bare GETs but a TCP-level reply is enough.
                    await client.get(url, timeout=0.5)
                    return
                except (httpx.ConnectError, httpx.ReadError):
                    await asyncio.sleep(0.1)
        raise RuntimeError(f"HTTP MCP fixture did not start on {url}")

    try:
        await wait_ready()
        yield url
    finally:
        proc.terminate()
        try:
            proc.wait(timeout=5)
        except subprocess.TimeoutExpired:
            proc.kill()


class TestStdioAdapter:
    @pytest.fixture
    def adapter(self):
        return McpAdapter(TEST_STDIO_SERVER)

    async def test_get_action_definitions(self, adapter: McpAdapter):
        actions = await adapter.get_action_definitions(env=TEST_ENV)
        assert len(actions) == 2
        names = {a.name for a in actions}
        assert names == {"greet", "add"}

        greet_action = next(a for a in actions if a.name == "greet")
        assert greet_action.description == "Greet someone by name."
        assert "name" in greet_action.input_schema.get("properties", {})
        assert "name" in greet_action.input_schema.get("required", [])
        assert greet_action.input_schema["properties"]["name"]["type"] == "string"
        assert greet_action.mode == "read"

        add_action = next(a for a in actions if a.name == "add")
        assert "a" in add_action.input_schema.get("properties", {})
        assert "b" in add_action.input_schema.get("properties", {})
        assert add_action.mode == "write"

    async def test_get_resource_definitions(self, adapter: McpAdapter):
        resources = await adapter.get_resource_definitions(env=TEST_ENV)
        assert len(resources) == 1
        assert resources[0].name == "get_item"
        assert resources[0].uri_template == "test://item/{item_id}"

    async def test_get_prompt_definitions(self, adapter: McpAdapter):
        prompts = await adapter.get_prompt_definitions(env=TEST_ENV)
        assert len(prompts) == 1
        assert prompts[0].name == "summarize"
        assert prompts[0].description == "Summarize the given text."
        assert len(prompts[0].arguments) == 1
        assert prompts[0].arguments[0].name == "text"
        assert prompts[0].arguments[0].required is True

    async def test_execute_tool(self, adapter: McpAdapter):
        result = await adapter.execute_tool("greet", {"name": "World"}, env=TEST_ENV)
        assert result.status == "success"
        assert result.result is not None
        assert "Hello, World!" in result.result.get("content", "")

    async def test_execute_tool_error(self, adapter: McpAdapter):
        result = await adapter.execute_tool("nonexistent", {}, env=TEST_ENV)
        assert result.status == "error"

    async def test_read_resource(self, adapter: McpAdapter):
        result = await adapter.read_resource("test://item/42", env=TEST_ENV)
        assert "contents" in result
        contents = result["contents"]
        assert len(contents) >= 1

    async def test_get_prompt(self, adapter: McpAdapter):
        result = await adapter.get_prompt(
            "summarize", {"text": "hello world"}, env=TEST_ENV
        )
        assert "messages" in result
        assert len(result["messages"]) >= 1
        msg = result["messages"][0]
        assert msg["role"] == "user"
        assert "hello world" in msg["content"]["text"]

    async def test_discover_caches_definitions(self, adapter: McpAdapter):
        """discover() populates cache, then no-auth calls return cached data."""
        await adapter.discover(env=TEST_ENV)
        actions = await adapter.get_action_definitions()
        assert len(actions) == 2
        resources = await adapter.get_resource_definitions()
        assert len(resources) == 1
        prompts = await adapter.get_prompt_definitions()
        assert len(prompts) == 1

    async def test_no_auth_no_cache_returns_empty(self, adapter: McpAdapter):
        """Without auth and without cache, returns empty lists."""
        assert await adapter.get_action_definitions() == []
        assert await adapter.get_resource_definitions() == []
        assert await adapter.get_prompt_definitions() == []

    async def test_cache_survives_connection_failure(self):
        """After successful discovery, cache is returned if subprocess can't start."""
        adapter = McpAdapter(TEST_STDIO_SERVER)
        await adapter.discover(env=TEST_ENV)
        assert len(adapter._cached_actions or []) == 2

        # Replace command with something that will fail
        adapter._server = StdioMcpServer(command="nonexistent-binary", args=[])
        cached = await adapter.get_action_definitions(env=TEST_ENV)
        assert len(cached) == 2
        assert {a.name for a in cached} == {"greet", "add"}


class TestHttpAdapter:
    """Streamable HTTP transport against the same fixture server."""

    async def test_get_action_definitions(self, http_server_url: str):
        adapter = McpAdapter(HttpMcpServer(url=http_server_url))
        actions = await adapter.get_action_definitions(headers={"X-Test": "1"})
        names = {a.name for a in actions}
        assert names == {"greet", "add"}

    async def test_execute_tool(self, http_server_url: str):
        adapter = McpAdapter(HttpMcpServer(url=http_server_url))
        result = await adapter.execute_tool(
            "greet", {"name": "Remote"}, headers={"X-Test": "1"}
        )
        assert result.status == "success"
        assert result.result is not None
        assert "Hello, Remote!" in result.result.get("content", "")

    async def test_read_resource(self, http_server_url: str):
        adapter = McpAdapter(HttpMcpServer(url=http_server_url))
        result = await adapter.read_resource("test://item/99", headers={"X-Test": "1"})
        assert "contents" in result and len(result["contents"]) >= 1

    async def test_get_prompt(self, http_server_url: str):
        adapter = McpAdapter(HttpMcpServer(url=http_server_url))
        result = await adapter.get_prompt(
            "summarize", {"text": "remote text"}, headers={"X-Test": "1"}
        )
        assert "messages" in result and len(result["messages"]) >= 1

    async def test_discover_caches_definitions(self, http_server_url: str):
        adapter = McpAdapter(HttpMcpServer(url=http_server_url))
        await adapter.discover(headers={"X-Test": "1"})
        # No headers — returns cache
        assert {a.name for a in await adapter.get_action_definitions()} == {
            "greet",
            "add",
        }
        assert len(await adapter.get_resource_definitions()) == 1
        assert len(await adapter.get_prompt_definitions()) == 1

    async def test_static_headers_merged_with_per_call_headers(
        self, http_server_url: str
    ):
        """Static headers on HttpMcpServer + per-call headers both reach the server."""
        adapter = McpAdapter(
            HttpMcpServer(url=http_server_url, headers={"X-Static": "yes"})
        )
        # No assertion on headers here (the test fixture doesn't expose them);
        # we just ensure the call succeeds with both sets configured.
        actions = await adapter.get_action_definitions(headers={"X-Per-Call": "yes"})
        assert len(actions) == 2


class TestConnectorMcpIntegration:
    """A Connector with an MCP server config delegates correctly."""

    @pytest.fixture
    def stdio_connector(self) -> Connector:
        class StdioMcpConnector(Connector):
            @property
            def name(self) -> str:
                return "mcp-test-stdio"

            @property
            def version(self) -> str:
                return "0.1.0"

            @property
            def source_types(self) -> list[str]:
                return ["mcp_test"]

            @property
            def mcp_server(self) -> StdioMcpServer:
                return TEST_STDIO_SERVER

            async def sync(
                self,
                source_config: dict[str, Any],
                credentials: dict[str, Any],
                state: dict[str, Any] | None,
                ctx: Any,
            ) -> None:
                pass

        return StdioMcpConnector()

    async def test_manifest_includes_mcp_tools_as_actions(
        self, stdio_connector: Connector
    ):
        await stdio_connector.bootstrap_mcp({"token": "test"})
        manifest = await stdio_connector.get_manifest(connector_url="http://test:8000")
        assert manifest.mcp_enabled is True
        action_names = {a.name for a in manifest.actions}
        assert "greet" in action_names
        assert "add" in action_names

    async def test_manifest_includes_resources(self, stdio_connector: Connector):
        await stdio_connector.bootstrap_mcp({"token": "test"})
        manifest = await stdio_connector.get_manifest(connector_url="http://test:8000")
        assert len(manifest.resources) == 1
        assert manifest.resources[0].uri_template == "test://item/{item_id}"

    async def test_manifest_includes_prompts(self, stdio_connector: Connector):
        await stdio_connector.bootstrap_mcp({"token": "test"})
        manifest = await stdio_connector.get_manifest(connector_url="http://test:8000")
        assert len(manifest.prompts) == 1
        assert manifest.prompts[0].name == "summarize"

    async def test_execute_action_delegates_to_mcp(self, stdio_connector: Connector):
        result = await stdio_connector.execute_action("greet", {"name": "Omni"}, {})
        assert result.status_code == 200

    async def test_execute_action_unknown_returns_not_supported(
        self, stdio_connector: Connector
    ):
        result = await stdio_connector.execute_action("unknown_action", {}, {})
        assert result.status_code == 404

    async def test_http_connector_round_trip(self, http_server_url: str):
        """A Connector pointing at an HttpMcpServer surfaces tools and dispatches."""

        class HttpMcpConnector(Connector):
            @property
            def name(self) -> str:
                return "mcp-test-http"

            @property
            def version(self) -> str:
                return "0.1.0"

            @property
            def source_types(self) -> list[str]:
                return ["mcp_test_http"]

            @property
            def mcp_server(self) -> HttpMcpServer:
                return HttpMcpServer(url=http_server_url)

            def prepare_mcp_headers(
                self, credentials: dict[str, Any]
            ) -> dict[str, str]:
                return {"Authorization": f"Bearer {credentials.get('token', '')}"}

            async def sync(self, *args: Any, **kwargs: Any) -> None:
                pass

        connector = HttpMcpConnector()
        await connector.bootstrap_mcp({"token": "abc"})
        manifest = await connector.get_manifest(connector_url="http://test:8000")
        assert manifest.mcp_enabled is True
        assert {a.name for a in manifest.actions} >= {"greet", "add"}

        result = await connector.execute_action(
            "greet", {"name": "HTTP"}, {"token": "abc"}
        )
        assert result.status_code == 200

    async def test_non_mcp_connector_manifest(self):
        class PlainConnector(Connector):
            @property
            def name(self) -> str:
                return "plain"

            @property
            def version(self) -> str:
                return "0.1.0"

            @property
            def source_types(self) -> list[str]:
                return ["plain"]

            async def sync(self, *args: Any, **kwargs: Any) -> None:
                pass

        connector = PlainConnector()
        manifest = await connector.get_manifest(connector_url="http://test:8000")
        assert manifest.mcp_enabled is False
        assert manifest.resources == []
        assert manifest.prompts == []

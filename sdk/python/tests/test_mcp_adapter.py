"""Tests for the MCP adapter integration."""

from typing import Any

import pytest
from mcp.server.fastmcp import FastMCP

from omni_connector import Connector
from omni_connector.mcp_adapter import McpAdapter


def _create_test_mcp_server() -> FastMCP:
    server = FastMCP("test")

    @server.tool()
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

    return server


class TestMcpAdapter:
    @pytest.fixture
    def mcp_server(self) -> FastMCP:
        return _create_test_mcp_server()

    @pytest.fixture
    def adapter(self, mcp_server: FastMCP) -> McpAdapter:
        return McpAdapter(mcp_server)

    async def test_get_action_definitions(self, adapter: McpAdapter):
        actions = await adapter.get_action_definitions()
        assert len(actions) == 2
        names = {a.name for a in actions}
        assert names == {"greet", "add"}

        greet_action = next(a for a in actions if a.name == "greet")
        assert greet_action.description == "Greet someone by name."
        assert "name" in greet_action.parameters
        assert greet_action.parameters["name"].required is True
        assert greet_action.parameters["name"].type == "string"

        add_action = next(a for a in actions if a.name == "add")
        assert "a" in add_action.parameters
        assert "b" in add_action.parameters
        assert add_action.parameters["a"].required is True

    async def test_get_resource_definitions(self, adapter: McpAdapter):
        resources = await adapter.get_resource_definitions()
        assert len(resources) == 1
        assert resources[0].name == "get_item"
        assert resources[0].uri_template == "test://item/{item_id}"

    async def test_get_prompt_definitions(self, adapter: McpAdapter):
        prompts = await adapter.get_prompt_definitions()
        assert len(prompts) == 1
        assert prompts[0].name == "summarize"
        assert prompts[0].description == "Summarize the given text."
        assert len(prompts[0].arguments) == 1
        assert prompts[0].arguments[0].name == "text"
        assert prompts[0].arguments[0].required is True

    async def test_execute_tool(self, adapter: McpAdapter):
        result = await adapter.execute_tool("greet", {"name": "World"})
        assert result.status == "success"
        assert result.result is not None
        assert result.result.get("result") == "Hello, World!"

    async def test_execute_tool_error(self, adapter: McpAdapter):
        result = await adapter.execute_tool("nonexistent", {})
        assert result.status == "error"

    async def test_read_resource(self, adapter: McpAdapter):
        result = await adapter.read_resource("test://item/42")
        assert "contents" in result
        contents = result["contents"]
        assert len(contents) >= 1
        # Resource should contain the item text
        first = contents[0]
        assert "text" in first or "blob" in first

    async def test_get_prompt(self, adapter: McpAdapter):
        result = await adapter.get_prompt("summarize", {"text": "hello world"})
        assert "messages" in result
        assert len(result["messages"]) >= 1
        msg = result["messages"][0]
        assert msg["role"] == "user"
        assert "hello world" in msg["content"]["text"]


class TestConnectorMcpIntegration:
    """Test that a Connector with an MCP server properly delegates."""

    @pytest.fixture
    def mcp_connector(self) -> Connector:
        mcp_server = _create_test_mcp_server()

        class McpTestConnector(Connector):
            @property
            def name(self) -> str:
                return "mcp-test"

            @property
            def version(self) -> str:
                return "0.1.0"

            @property
            def source_types(self) -> list[str]:
                return ["mcp_test"]

            @property
            def mcp_server(self) -> FastMCP:
                return mcp_server

            async def sync(
                self,
                source_config: dict[str, Any],
                credentials: dict[str, Any],
                state: dict[str, Any] | None,
                ctx: Any,
            ) -> None:
                pass

        return McpTestConnector()

    async def test_manifest_includes_mcp_tools_as_actions(
        self, mcp_connector: Connector
    ):
        manifest = await mcp_connector.get_manifest(connector_url="http://test:8000")
        assert manifest.mcp_enabled is True
        action_names = {a.name for a in manifest.actions}
        assert "greet" in action_names
        assert "add" in action_names

    async def test_manifest_includes_resources(self, mcp_connector: Connector):
        manifest = await mcp_connector.get_manifest(connector_url="http://test:8000")
        assert len(manifest.resources) == 1
        assert manifest.resources[0].uri_template == "test://item/{item_id}"

    async def test_manifest_includes_prompts(self, mcp_connector: Connector):
        manifest = await mcp_connector.get_manifest(connector_url="http://test:8000")
        assert len(manifest.prompts) == 1
        assert manifest.prompts[0].name == "summarize"

    async def test_execute_action_delegates_to_mcp(self, mcp_connector: Connector):
        result = await mcp_connector.execute_action("greet", {"name": "Omni"}, {})
        assert result.status == "success"
        assert result.result is not None

    async def test_execute_action_unknown_returns_not_supported(
        self, mcp_connector: Connector
    ):
        result = await mcp_connector.execute_action("unknown_action", {}, {})
        assert result.status == "error"
        assert "not supported" in (result.error or "").lower()

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

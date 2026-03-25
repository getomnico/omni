"""Tests for the MCP adapter (stdio transport)."""

import os
import sys
from typing import Any

import pytest
from mcp.client.stdio import StdioServerParameters

from omni_connector import Connector
from omni_connector.mcp_adapter import McpAdapter

# Path to the test MCP server script
TEST_SERVER = os.path.join(os.path.dirname(__file__), "test_mcp_server.py")
TEST_PARAMS = StdioServerParameters(command=sys.executable, args=[TEST_SERVER])
# Dummy env to simulate having credentials (test server doesn't need real ones)
TEST_ENV: dict[str, str] = {"TEST_MODE": "1"}


class TestMcpAdapter:
    @pytest.fixture
    async def adapter(self):
        adapter = McpAdapter(TEST_PARAMS)
        await adapter.ensure_connected(TEST_ENV)
        yield adapter
        await adapter.disconnect()

    async def test_get_action_definitions(self, adapter: McpAdapter):
        actions = await adapter.get_action_definitions(TEST_ENV)
        assert len(actions) == 2
        names = {a.name for a in actions}
        assert names == {"greet", "add"}

        greet_action = next(a for a in actions if a.name == "greet")
        assert greet_action.description == "Greet someone by name."
        assert "name" in greet_action.parameters
        assert greet_action.parameters["name"].required is True
        assert greet_action.parameters["name"].type == "string"
        assert greet_action.mode == "read"

        add_action = next(a for a in actions if a.name == "add")
        assert "a" in add_action.parameters
        assert "b" in add_action.parameters
        assert add_action.parameters["a"].required is True
        assert add_action.mode == "write"

    async def test_get_resource_definitions(self, adapter: McpAdapter):
        resources = await adapter.get_resource_definitions(TEST_ENV)
        assert len(resources) == 1
        assert resources[0].name == "get_item"
        assert resources[0].uri_template == "test://item/{item_id}"

    async def test_get_prompt_definitions(self, adapter: McpAdapter):
        prompts = await adapter.get_prompt_definitions(TEST_ENV)
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

    async def test_reconnect_with_new_env(self, adapter: McpAdapter):
        """Calling ensure_connected with different env restarts the subprocess."""
        actions1 = await adapter.get_action_definitions(TEST_ENV)
        assert len(actions1) == 2

        # Reconnect with different env — should restart
        await adapter.ensure_connected(env={"SOME_VAR": "value"})
        actions2 = await adapter.get_action_definitions(env={"SOME_VAR": "value"})
        assert len(actions2) == 2

    async def test_disconnect_and_reconnect(self, adapter: McpAdapter):
        await adapter.disconnect()
        # Should reconnect on next call with env
        actions = await adapter.get_action_definitions(TEST_ENV)
        assert len(actions) == 2

    async def test_cache_survives_connection_failure(self):
        """After successful discovery, cache is returned if subprocess can't start."""
        adapter = McpAdapter(TEST_PARAMS)
        # First: connect and populate cache
        actions = await adapter.get_action_definitions(TEST_ENV)
        assert len(actions) == 2
        await adapter.disconnect()

        # Replace command with something that will fail
        adapter._base_params = StdioServerParameters(
            command="nonexistent-binary", args=[]
        )
        # Should return cached actions instead of raising
        cached = await adapter.get_action_definitions()
        assert len(cached) == 2
        assert {a.name for a in cached} == {"greet", "add"}


class TestConnectorMcpIntegration:
    """Test that a Connector with an MCP command properly delegates."""

    @pytest.fixture
    def mcp_connector(self) -> Connector:
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
            def mcp_command(self) -> StdioServerParameters:
                return TEST_PARAMS

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
        await mcp_connector.bootstrap_mcp({"token": "test"})
        manifest = await mcp_connector.get_manifest(connector_url="http://test:8000")
        assert manifest.mcp_enabled is True
        action_names = {a.name for a in manifest.actions}
        assert "greet" in action_names
        assert "add" in action_names
        await mcp_connector.stop_mcp()

    async def test_manifest_includes_resources(self, mcp_connector: Connector):
        await mcp_connector.bootstrap_mcp({"token": "test"})
        manifest = await mcp_connector.get_manifest(connector_url="http://test:8000")
        assert len(manifest.resources) == 1
        assert manifest.resources[0].uri_template == "test://item/{item_id}"
        await mcp_connector.stop_mcp()

    async def test_manifest_includes_prompts(self, mcp_connector: Connector):
        await mcp_connector.bootstrap_mcp({"token": "test"})
        manifest = await mcp_connector.get_manifest(connector_url="http://test:8000")
        assert len(manifest.prompts) == 1
        assert manifest.prompts[0].name == "summarize"
        await mcp_connector.stop_mcp()

    async def test_execute_action_delegates_to_mcp(self, mcp_connector: Connector):
        result = await mcp_connector.execute_action("greet", {"name": "Omni"}, {})
        assert result.status == "success"
        assert result.result is not None
        await mcp_connector.stop_mcp()

    async def test_execute_action_unknown_returns_not_supported(
        self, mcp_connector: Connector
    ):
        result = await mcp_connector.execute_action("unknown_action", {}, {})
        assert result.status == "error"
        assert "not supported" in (result.error or "").lower()
        await mcp_connector.stop_mcp()

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

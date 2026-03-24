from __future__ import annotations

import logging
from abc import ABC, abstractmethod
from typing import TYPE_CHECKING, Any

from .context import SyncContext
from .models import ActionDefinition, ActionResponse, ConnectorManifest, SearchOperator

if TYPE_CHECKING:
    from mcp.server.fastmcp import FastMCP

    from .mcp_adapter import McpAdapter

logger = logging.getLogger(__name__)


class Connector(ABC):
    """Base class for Omni connectors."""

    def __init__(self) -> None:
        self._cancelled_syncs: set[str] = set()
        self._mcp_adapter: McpAdapter | None = None

    @property
    @abstractmethod
    def name(self) -> str:
        """Connector name (e.g., 'google-drive', 'slack')."""
        pass

    @property
    @abstractmethod
    def version(self) -> str:
        """Connector version (semver)."""
        pass

    @property
    @abstractmethod
    def source_types(self) -> list[str]:
        """Source type slugs this connector handles (e.g., ['google_drive', 'gmail'])."""
        pass

    @property
    def display_name(self) -> str:
        """Human-readable display name. Override to customize."""
        return self.name

    @property
    def description(self) -> str:
        """Short description for the UI. Override to customize."""
        return ""

    @property
    def sync_modes(self) -> list[str]:
        """Supported sync modes. Override to customize."""
        return ["full"]

    @property
    def actions(self) -> list[ActionDefinition]:
        """Available connector actions. Override to add actions."""
        return []

    @property
    def search_operators(self) -> list[SearchOperator]:
        """Search operators this connector supports. Override to declare operators."""
        return []

    @property
    def mcp_server(self) -> FastMCP | None:
        """Return an MCP FastMCP server instance if this connector supports MCP.

        Override this property to enable MCP support. The SDK will automatically
        introspect the server's tools, resources, and prompts and expose them
        through the Omni protocol.
        """
        return None

    @property
    def mcp_adapter(self) -> McpAdapter | None:
        if self._mcp_adapter is not None:
            return self._mcp_adapter
        server = self.mcp_server
        if server is None:
            return None
        from .mcp_adapter import McpAdapter

        self._mcp_adapter = McpAdapter(server)
        return self._mcp_adapter

    async def _get_all_actions(self) -> list[ActionDefinition]:
        """Merge manually-defined actions with MCP-derived actions."""
        manual_actions = self.actions
        adapter = self.mcp_adapter
        if adapter is None:
            return manual_actions
        mcp_actions = await adapter.get_action_definitions()
        manual_names = {a.name for a in manual_actions}
        merged = list(manual_actions)
        for action in mcp_actions:
            if action.name not in manual_names:
                merged.append(action)
        return merged

    async def get_manifest(self, connector_url: str) -> ConnectorManifest:
        """Return connector manifest."""
        adapter = self.mcp_adapter
        return ConnectorManifest(
            name=self.name,
            display_name=self.display_name,
            version=self.version,
            sync_modes=self.sync_modes,
            connector_id=self.name,
            connector_url=connector_url,
            source_types=self.source_types,
            description=self.description,
            actions=await self._get_all_actions(),
            search_operators=self.search_operators,
            mcp_enabled=adapter is not None,
            resources=await adapter.get_resource_definitions() if adapter else [],
            prompts=await adapter.get_prompt_definitions() if adapter else [],
        )

    @abstractmethod
    async def sync(
        self,
        source_config: dict[str, Any],
        credentials: dict[str, Any],
        state: dict[str, Any] | None,
        ctx: SyncContext,
    ) -> None:
        """
        Execute a sync operation.

        Args:
            source_config: Source configuration from database
            credentials: Authentication credentials
            state: Previous sync state for incremental syncs
            ctx: Sync context with emit(), complete(), etc.
        """
        pass

    def cancel(self, sync_run_id: str) -> bool:
        """
        Handle cancellation request.

        Returns True if sync was found and marked for cancellation.
        """
        self._cancelled_syncs.add(sync_run_id)
        return True

    def prepare_mcp_env(self, credentials: dict[str, Any]) -> None:
        """Set up environment for MCP tool/resource/prompt calls.

        Override this to bridge Omni credentials to the env vars your MCP
        server expects. Called before every MCP dispatch.

        Example::

            def prepare_mcp_env(self, credentials):
                os.environ["GITHUB_TOKEN"] = credentials.get("token", "")
        """

    async def execute_action(
        self,
        action: str,
        params: dict[str, Any],
        credentials: dict[str, Any],
    ) -> ActionResponse:
        """
        Execute a connector action.

        Override this method to implement connector-specific actions.
        If MCP is enabled and the action matches an MCP tool, it is
        dispatched to the MCP server automatically.
        """
        adapter = self.mcp_adapter
        if adapter is not None:
            mcp_tool_names = {a.name for a in await adapter.get_action_definitions()}
            if action in mcp_tool_names:
                self.prepare_mcp_env(credentials)
                return await adapter.execute_tool(action, params)
        return ActionResponse.not_supported(action)

    def serve(self, port: int = 8000, host: str = "0.0.0.0") -> None:
        """Start the HTTP server for this connector."""
        import uvicorn

        from .server import create_app

        app = create_app(self)
        logger.info("Starting %s connector on %s:%d", self.name, host, port)
        uvicorn.run(app, host=host, port=port)

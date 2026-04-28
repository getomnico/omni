from __future__ import annotations

import logging
from abc import ABC, abstractmethod
from typing import TYPE_CHECKING, Any

from fastapi.responses import JSONResponse

from .context import SyncContext
from .models import (
    ActionDefinition,
    ActionResponse,
    ConnectorManifest,
    SearchOperator,
)

if TYPE_CHECKING:
    from .mcp_adapter import McpAdapter, McpServer

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
    def mcp_server(self) -> McpServer | None:
        """Return MCP server config (stdio or Streamable HTTP).

        Override this property to enable MCP support. Return either a
        ``StdioMcpServer`` to spawn a local subprocess MCP server, or an
        ``HttpMcpServer`` to connect to a remote one. The SDK exposes the
        server's tools, resources, and prompts through the Omni protocol.

        Examples::

            from omni_connector import StdioMcpServer, HttpMcpServer

            @property
            def mcp_server(self):
                return StdioMcpServer(
                    command="github-mcp-server",
                    args=["stdio"],
                )

            @property
            def mcp_server(self):
                return HttpMcpServer(url="https://api.example.com/mcp")
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

    def _prepare_mcp_auth(self, credentials: dict[str, Any]) -> dict[str, Any]:
        """Build the env-or-headers kwargs to pass to the MCP adapter.

        Dispatches based on the configured transport: stdio servers receive
        ``env=...`` (from ``prepare_mcp_env``); HTTP servers receive
        ``headers=...`` (from ``prepare_mcp_headers``).
        """
        from .mcp_adapter import HttpMcpServer

        server = self.mcp_server
        if isinstance(server, HttpMcpServer):
            return {"headers": self.prepare_mcp_headers(credentials)}
        return {"env": self.prepare_mcp_env(credentials)}

    async def bootstrap_mcp(self, credentials: dict[str, Any]) -> None:
        """Discover MCP tools/resources/prompts and cache them.

        Called when credentials first become available (e.g., during initial sync).
        Opens a temporary session, introspects it, caches the results, then
        shuts down. Subsequent manifest builds use the cache.
        """
        adapter = self.mcp_adapter
        if adapter is None:
            logger.debug("bootstrap_mcp: no MCP adapter, skipping")
            return
        auth = self._prepare_mcp_auth(credentials)
        logger.info("Bootstrapping MCP: discovering tools")
        try:
            await adapter.discover(**auth)
        except Exception:
            logger.warning("MCP bootstrap failed", exc_info=True)

    async def _get_all_actions(self) -> list[ActionDefinition]:
        """Merge manually-defined actions with MCP-derived actions."""
        manual_actions = self.actions
        adapter = self.mcp_adapter
        if adapter is None:
            return manual_actions
        try:
            mcp_actions = await adapter.get_action_definitions()
        except Exception:
            logger.warning("Failed to list MCP tools", exc_info=True)
            return manual_actions
        manual_names = {a.name for a in manual_actions}
        merged = list(manual_actions)
        for action in mcp_actions:
            if action.name not in manual_names:
                merged.append(action)
        return merged

    async def get_manifest(self, connector_url: str) -> ConnectorManifest:
        """Return connector manifest."""
        adapter = self.mcp_adapter
        resources = []
        prompts = []
        if adapter is not None:
            try:
                resources = await adapter.get_resource_definitions()
            except Exception:
                logger.warning("Failed to list MCP resources", exc_info=True)
            try:
                prompts = await adapter.get_prompt_definitions()
            except Exception:
                logger.warning("Failed to list MCP prompts", exc_info=True)
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
            resources=resources,
            prompts=prompts,
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

    def prepare_mcp_env(self, credentials: dict[str, Any]) -> dict[str, str]:
        """Return env vars for a stdio MCP subprocess given Omni credentials.

        Override this to bridge Omni credentials to the env vars your MCP
        server expects. Used only when ``mcp_server`` returns a
        ``StdioMcpServer``. The returned dict is merged into the subprocess env.

        Example::

            def prepare_mcp_env(self, credentials):
                return {"GITHUB_PERSONAL_ACCESS_TOKEN": credentials["token"]}
        """
        return {}

    def prepare_mcp_headers(self, credentials: dict[str, Any]) -> dict[str, str]:
        """Return HTTP headers for a remote MCP server given Omni credentials.

        Override this to inject auth headers (e.g., ``Authorization: Bearer ...``)
        into Streamable HTTP requests. Used only when ``mcp_server`` returns
        an ``HttpMcpServer``.

        Example::

            def prepare_mcp_headers(self, credentials):
                return {"Authorization": f"Bearer {credentials['token']}"}
        """
        return {}

    async def execute_action(
        self,
        action: str,
        params: dict[str, Any],
        credentials: dict[str, Any],
    ) -> JSONResponse:

        adapter = self.mcp_adapter
        if adapter is not None:
            auth = self._prepare_mcp_auth(credentials)
            mcp_tool_names = {
                a.name for a in await adapter.get_action_definitions(**auth)
            }
            if action in mcp_tool_names:
                response = await adapter.execute_tool(action, params, **auth)
                return JSONResponse(content=response.model_dump())
        return ActionResponse.not_supported(action).to_response(status_code=404)

    def serve(self, port: int = 8000, host: str = "0.0.0.0") -> None:
        """Start the HTTP server for this connector."""
        import uvicorn

        from .server import create_app

        app = create_app(self)
        logger.info("Starting %s connector on %s:%d", self.name, host, port)
        uvicorn.run(app, host=host, port=port)

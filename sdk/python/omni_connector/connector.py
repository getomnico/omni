import logging
from abc import ABC, abstractmethod
from typing import Any

from .context import SyncContext
from .models import ActionDefinition, ActionResponse, ConnectorManifest

logger = logging.getLogger(__name__)


class Connector(ABC):
    """Base class for Omni connectors."""

    def __init__(self) -> None:
        self._cancelled_syncs: set[str] = set()

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
    def sync_modes(self) -> list[str]:
        """Supported sync modes. Override to customize."""
        return ["full"]

    @property
    def actions(self) -> list[ActionDefinition]:
        """Available connector actions. Override to add actions."""
        return []

    def get_manifest(self) -> ConnectorManifest:
        """Return connector manifest."""
        return ConnectorManifest(
            name=self.name,
            version=self.version,
            sync_modes=self.sync_modes,
            actions=self.actions,
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

    async def execute_action(
        self,
        action: str,
        params: dict[str, Any],
        credentials: dict[str, Any],
    ) -> ActionResponse:
        """
        Execute a connector action.

        Override this method to implement connector-specific actions.
        Default implementation returns not_supported.
        """
        return ActionResponse.not_supported(action)

    def serve(self, port: int = 8000, host: str = "0.0.0.0") -> None:
        """Start the HTTP server for this connector."""
        import uvicorn

        from .server import create_app

        app = create_app(self)
        logger.info("Starting %s connector on %s:%d", self.name, host, port)
        uvicorn.run(app, host=host, port=port)

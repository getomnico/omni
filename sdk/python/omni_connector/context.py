import asyncio
import logging
from typing import Any

from .client import SdkClient
from .models import ConnectorEvent, Document, EventType
from .storage import ContentStorage

logger = logging.getLogger(__name__)


class SyncContext:
    """Context provided to sync() method with utilities for emitting documents."""

    def __init__(
        self,
        sdk_client: SdkClient,
        sync_run_id: str,
        source_id: str,
        state: dict[str, Any] | None = None,
    ):
        self._client = sdk_client
        self._sync_run_id = sync_run_id
        self._source_id = source_id
        self._state = state or {}
        self._cancelled = asyncio.Event()
        self._documents_emitted = 0
        self._documents_scanned = 0
        self._content_storage = ContentStorage(sdk_client, sync_run_id)

    @property
    def sync_run_id(self) -> str:
        return self._sync_run_id

    @property
    def source_id(self) -> str:
        return self._source_id

    @property
    def state(self) -> dict[str, Any]:
        return self._state

    @property
    def content_storage(self) -> ContentStorage:
        return self._content_storage

    @property
    def documents_emitted(self) -> int:
        return self._documents_emitted

    @property
    def documents_scanned(self) -> int:
        return self._documents_scanned

    async def emit(self, doc: Document) -> None:
        """Push document to queue. Implicitly heartbeats (updates last_activity_at)."""
        event = ConnectorEvent(
            type=EventType.DOCUMENT_CREATED,
            sync_run_id=self._sync_run_id,
            source_id=self._source_id,
            document_id=doc.external_id,
            content_id=doc.content_id,
            metadata=doc.metadata,
            permissions=doc.permissions,
            attributes=doc.attributes,
        )

        await self._client.emit_event(
            self._sync_run_id,
            self._source_id,
            event,
        )
        self._documents_emitted += 1

    async def emit_updated(self, doc: Document) -> None:
        """Push document update to queue."""
        event = ConnectorEvent(
            type=EventType.DOCUMENT_UPDATED,
            sync_run_id=self._sync_run_id,
            source_id=self._source_id,
            document_id=doc.external_id,
            content_id=doc.content_id,
            metadata=doc.metadata,
            permissions=doc.permissions,
            attributes=doc.attributes,
        )

        await self._client.emit_event(
            self._sync_run_id,
            self._source_id,
            event,
        )
        self._documents_emitted += 1

    async def emit_deleted(self, external_id: str) -> None:
        """Mark document as deleted in source."""
        event = ConnectorEvent(
            type=EventType.DOCUMENT_DELETED,
            sync_run_id=self._sync_run_id,
            source_id=self._source_id,
            document_id=external_id,
        )

        await self._client.emit_event(
            self._sync_run_id,
            self._source_id,
            event,
        )

    async def emit_error(self, external_id: str, error: str) -> None:
        """Report non-fatal error for a specific document. Sync continues."""
        logger.warning("Document error for %s: %s", external_id, error)

    async def increment_scanned(self) -> None:
        """Increment scanned counter and send heartbeat."""
        self._documents_scanned += 1
        await self._client.increment_scanned(self._sync_run_id)

    async def save_state(self, state: dict[str, Any]) -> None:
        """Checkpoint state for resumability. Call periodically for long syncs."""
        self._state = state
        await self._client.heartbeat(self._sync_run_id)

    async def complete(self, new_state: dict[str, Any] | None = None) -> None:
        """Mark sync as successfully completed. Saves final state."""
        await self._client.complete(
            self._sync_run_id,
            self._documents_scanned,
            self._documents_emitted,
            new_state,
        )

    async def fail(self, error: str) -> None:
        """Mark sync as failed with error message."""
        await self._client.fail(self._sync_run_id, error)

    def is_cancelled(self) -> bool:
        """Check if sync was cancelled. Connector should poll this periodically."""
        return self._cancelled.is_set()

    def _set_cancelled(self) -> None:
        """Internal method to signal cancellation."""
        self._cancelled.set()

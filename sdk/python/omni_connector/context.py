import asyncio
import logging
import time
from typing import Any

from .client import SdkClient
from .models import (
    ConnectorEvent,
    DocumentEvent,
    Document,
    EventType,
    GroupMembershipSyncEvent,
    SyncMode,
    UserFilterMode,
)
from .storage import ContentStorage

logger = logging.getLogger(__name__)


def _thresholds_for(sync_mode: SyncMode) -> tuple[int, float | None]:
    """Buffer thresholds (size, time_secs) per sync mode.

    - Full: batch aggressively, generous wait
    - Incremental: balanced
    - Realtime: flush on every emit (size=1, no time bound)
    """
    if sync_mode == SyncMode.FULL:
        return (500, 300.0)
    if sync_mode == SyncMode.REALTIME:
        return (1, None)
    return (100, 60.0)  # Incremental (default)


class SyncContext:
    """Context provided to sync() method with utilities for emitting documents."""

    def __init__(
        self,
        sdk_client: SdkClient,
        sync_run_id: str,
        source_id: str,
        source_type: str | None = None,
        state: dict[str, Any] | None = None,
        user_filter_mode: UserFilterMode = UserFilterMode.ALL,
        user_whitelist: list[str] | None = None,
        user_blacklist: list[str] | None = None,
        sync_mode: SyncMode = SyncMode.INCREMENTAL,
        documents_scanned: int = 0,
        documents_updated: int = 0,
    ):
        self._client = sdk_client
        self._sync_run_id = sync_run_id
        self._source_id = source_id
        self._source_type = source_type
        self._state = state or {}
        self._cancelled = asyncio.Event()
        # Counters report seed (from the dispatch payload) plus everything
        # incremented during this run, so resume picks up where the previous
        # attempt left off rather than restarting at zero.
        self._documents_emitted = documents_updated
        self._documents_scanned = documents_scanned
        self._content_storage = ContentStorage(sdk_client, sync_run_id)
        self._user_filter_mode = user_filter_mode
        self._user_whitelist = {e.lower() for e in (user_whitelist or [])}
        self._user_blacklist = {e.lower() for e in (user_blacklist or [])}
        self._sync_mode = sync_mode
        self._buffer_size_threshold, self._buffer_time_threshold = _thresholds_for(
            sync_mode
        )
        self._event_buffer: list[ConnectorEvent] = []
        self._oldest_event_at: float | None = None

    @property
    def sync_run_id(self) -> str:
        return self._sync_run_id

    @property
    def source_id(self) -> str:
        return self._source_id

    @property
    def source_type(self) -> str | None:
        return self._source_type

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

    def should_index_user(self, user_email: str) -> bool:
        """Check if a user should be indexed based on filter settings."""
        if self._user_filter_mode == UserFilterMode.ALL:
            return True
        email = user_email.lower()
        if not email:
            return False
        if self._user_filter_mode == UserFilterMode.WHITELIST:
            return email in self._user_whitelist
        if self._user_filter_mode == UserFilterMode.BLACKLIST:
            return email not in self._user_blacklist
        return True

    async def _buffer_event(self, event: ConnectorEvent) -> None:
        """Append an event to the buffer and flush if size/time threshold is hit.

        Auto-flush errors propagate to the caller so the connector knows an
        event was not persisted before it checkpoints past it.
        """
        self._event_buffer.append(event)
        if self._oldest_event_at is None:
            self._oldest_event_at = time.monotonic()

        size_hit = len(self._event_buffer) >= self._buffer_size_threshold
        time_hit = (
            self._buffer_time_threshold is not None
            and self._oldest_event_at is not None
            and (time.monotonic() - self._oldest_event_at)
            >= self._buffer_time_threshold
        )
        if size_hit or time_hit:
            await self.flush()

    async def flush(self) -> None:
        """Flush all buffered events to the connector manager."""
        if not self._event_buffer:
            return
        batch = self._event_buffer
        try:
            await self._client.emit_event_batch(
                self._sync_run_id, self._source_id, batch
            )
            self._event_buffer = []
            self._oldest_event_at = None
        except:
            logger.error("Failed to flush event batch, will retry on next flush")
            self._event_buffer = batch

    async def emit(self, doc: Document) -> None:
        """Push document to queue. Implicitly heartbeats (updates last_activity_at)."""
        if doc.metadata and not doc.metadata.title:
            doc.metadata.title = doc.title
        event = DocumentEvent(
            type=EventType.DOCUMENT_CREATED,
            sync_run_id=self._sync_run_id,
            source_id=self._source_id,
            document_id=doc.external_id,
            content_id=doc.content_id,
            metadata=doc.metadata,
            permissions=doc.permissions,
            attributes=doc.attributes,
        )
        await self._buffer_event(event)
        self._documents_emitted += 1

    async def emit_updated(self, doc: Document) -> None:
        """Push document update to queue."""
        if doc.metadata and not doc.metadata.title:
            doc.metadata.title = doc.title
        event = DocumentEvent(
            type=EventType.DOCUMENT_UPDATED,
            sync_run_id=self._sync_run_id,
            source_id=self._source_id,
            document_id=doc.external_id,
            content_id=doc.content_id,
            metadata=doc.metadata,
            permissions=doc.permissions,
            attributes=doc.attributes,
        )
        await self._buffer_event(event)
        self._documents_emitted += 1

    async def emit_deleted(self, external_id: str) -> None:
        """Mark document as deleted in source."""
        event = DocumentEvent(
            type=EventType.DOCUMENT_DELETED,
            sync_run_id=self._sync_run_id,
            source_id=self._source_id,
            document_id=external_id,
        )
        await self._buffer_event(event)

    async def emit_group_membership(
        self,
        group_email: str,
        member_emails: list[str],
        group_name: str | None = None,
    ) -> None:
        """Emit a group membership sync event."""
        event = GroupMembershipSyncEvent(
            sync_run_id=self._sync_run_id,
            source_id=self._source_id,
            group_email=group_email,
            group_name=group_name,
            member_emails=member_emails,
        )
        await self._buffer_event(event)

    async def emit_error(self, external_id: str, error: str) -> None:
        """Report non-fatal error for a specific document. Sync continues."""
        logger.warning("Document error for %s: %s", external_id, error)

    async def increment_scanned(self) -> None:
        """Increment scanned counter and send heartbeat."""
        self._documents_scanned += 1
        await self._client.increment_scanned(self._sync_run_id)

    async def save_state(self, state: dict[str, Any]) -> None:
        """Checkpoint state for resumability. Call periodically for long syncs.

        Persists `state` to the manager so an interrupted sync can resume from
        this point. Flushes buffered events first — without this, a crash
        right after checkpointing would lose events that the connector
        considered emitted (the next run resumes past them).
        """
        await self.flush()
        self._state = state
        await self._client.update_connector_state(self._source_id, state)
        await self._client.heartbeat(self._sync_run_id)

    async def complete(self, new_state: dict[str, Any] | None = None) -> None:
        """Mark sync as successfully completed. Saves final state."""
        await self.flush()
        await self._client.complete(
            self._sync_run_id,
            self._documents_scanned,
            self._documents_emitted,
            new_state,
        )

    async def fail(self, error: str) -> None:
        """Mark sync as failed with error message.

        Best-effort flush of buffered events first — a flush failure is
        logged and swallowed so we always mark the sync as failed.
        """
        try:
            await self.flush()
        except Exception as e:
            logger.warning(
                "flush before fail() failed (continuing): sync_run=%s: %s",
                self._sync_run_id,
                e,
            )
        await self._client.fail(self._sync_run_id, error)

    def is_cancelled(self) -> bool:
        """Check if sync was cancelled. Connector should poll this periodically."""
        return self._cancelled.is_set()

    def _set_cancelled(self) -> None:
        """Internal method to signal cancellation."""
        self._cancelled.set()

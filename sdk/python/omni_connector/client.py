import logging
import os
from typing import Any

import httpx

from pydantic import ValidationError

from .exceptions import SdkClientError, ServiceOverloadedError
from .models import ConnectorEvent, SdkSourceSyncData

logger = logging.getLogger(__name__)


class SdkClient:
    """HTTP client for communicating with connector-manager SDK endpoints."""

    def __init__(self, base_url: str | None = None, timeout: float = 30.0):
        self.base_url = (
            base_url or os.environ.get("CONNECTOR_MANAGER_URL", "")
        ).rstrip("/")
        if not self.base_url:
            raise ValueError("CONNECTOR_MANAGER_URL environment variable not set")
        self._timeout = timeout
        self._client: httpx.AsyncClient | None = None

    @classmethod
    def from_env(cls) -> "SdkClient":
        return cls()

    async def _get_client(self) -> httpx.AsyncClient:
        if self._client is None:
            self._client = httpx.AsyncClient(timeout=self._timeout)
        return self._client

    async def fetch_source_sync_data(self, source_id: str) -> SdkSourceSyncData:
        """Fetch source sync data (config, credentials, state, filters) from connector-manager."""
        client = await self._get_client()
        response = await client.get(
            f"{self.base_url}/sdk/source/{source_id}/sync-config"
        )

        if not response.is_success:
            raise SdkClientError(
                f"Failed to fetch source sync data: {response.status_code} - {response.text}"
            )

        try:
            return SdkSourceSyncData.model_validate(response.json())
        except ValidationError as e:
            logger.error(
                "Failed to deserialize source sync data for %s: %s",
                source_id,
                e,
            )
            raise SdkClientError(f"Invalid source sync data response: {e}") from e

    async def emit_event(
        self,
        sync_run_id: str,
        source_id: str,
        event: ConnectorEvent,
    ) -> None:
        """Emit a connector event (document or group membership) to the queue."""
        logger.debug("SDK: Emitting event for sync_run=%s", sync_run_id)

        payload = {
            "sync_run_id": sync_run_id,
            "source_id": source_id,
            "event": event.to_dict(),
        }

        client = await self._get_client()
        response = await client.post(
            f"{self.base_url}/sdk/events",
            json=payload,
        )

        if not response.is_success:
            raise SdkClientError(
                f"Failed to emit event: {response.status_code} - {response.text}"
            )

    async def extract_and_store_content(
        self,
        sync_run_id: str,
        data: bytes,
        mime_type: str,
        filename: str | None = None,
    ) -> str:
        """Extract text from binary file content and store it, returning content_id.

        The connector manager extracts text based on the MIME type (PDF, DOCX,
        XLSX, PPTX, HTML, etc.) and stores the result.
        """
        logger.debug(
            "SDK: Extracting content for sync_run=%s, mime=%s, size=%d",
            sync_run_id,
            mime_type,
            len(data),
        )

        files: dict[str, Any] = {
            "data": ("file", data, "application/octet-stream"),
        }
        form_data: dict[str, str] = {
            "sync_run_id": sync_run_id,
            "mime_type": mime_type,
        }
        if filename is not None:
            form_data["filename"] = filename

        client = await self._get_client()
        response = await client.post(
            f"{self.base_url}/sdk/extract-content",
            data=form_data,
            files=files,
        )

        if response.status_code == 429:
            retry_after = int(response.headers.get("retry-after", "30"))
            raise ServiceOverloadedError(
                f"Extraction service overloaded: {response.text}",
                retry_after=retry_after,
            )

        if not response.is_success:
            raise SdkClientError(
                f"Failed to extract content: {response.status_code} - {response.text}"
            )

        return response.json()["content_id"]

    async def extract_text(
        self,
        sync_run_id: str,
        data: bytes,
        mime_type: str,
        filename: str | None = None,
    ) -> str:
        """Extract text from binary file content without storing.

        Same extraction as extract_and_store_content (uses Docling when
        enabled) but returns the extracted text instead of storing it.
        Use when the caller needs to post-process or combine text before
        storing.
        """
        logger.debug(
            "SDK: Extracting text for sync_run=%s, mime=%s, size=%d",
            sync_run_id,
            mime_type,
            len(data),
        )

        files: dict[str, Any] = {
            "data": ("file", data, "application/octet-stream"),
        }
        form_data: dict[str, str] = {
            "sync_run_id": sync_run_id,
            "mime_type": mime_type,
        }
        if filename is not None:
            form_data["filename"] = filename

        client = await self._get_client()
        response = await client.post(
            f"{self.base_url}/sdk/extract-text",
            data=form_data,
            files=files,
        )

        if response.status_code == 429:
            retry_after = int(response.headers.get("retry-after", "30"))
            raise ServiceOverloadedError(
                f"Extraction service overloaded: {response.text}",
                retry_after=retry_after,
            )

        if not response.is_success:
            raise SdkClientError(
                f"Failed to extract text: {response.status_code} - {response.text}"
            )

        return response.json()["text"]

    async def store_content(
        self,
        sync_run_id: str,
        content: str,
        content_type: str | None = "text/plain",
    ) -> str:
        """Store content and return content_id."""
        logger.debug("SDK: Storing content for sync_run=%s", sync_run_id)

        payload = {
            "sync_run_id": sync_run_id,
            "content": content,
            "content_type": content_type,
        }

        client = await self._get_client()
        response = await client.post(
            f"{self.base_url}/sdk/content",
            json=payload,
        )

        if not response.is_success:
            raise SdkClientError(
                f"Failed to store content: {response.status_code} - {response.text}"
            )

        return response.json()["content_id"]

    async def heartbeat(self, sync_run_id: str) -> None:
        """Send heartbeat to update last_activity_at."""
        logger.debug("SDK: Heartbeat for sync_run=%s", sync_run_id)

        client = await self._get_client()
        response = await client.post(
            f"{self.base_url}/sdk/sync/{sync_run_id}/heartbeat"
        )

        if not response.is_success:
            raise SdkClientError(
                f"Failed to heartbeat: {response.status_code} - {response.text}"
            )

    async def increment_scanned(self, sync_run_id: str) -> None:
        """Increment scanned count and update heartbeat."""
        logger.debug("SDK: Incrementing scanned for sync_run=%s", sync_run_id)

        client = await self._get_client()
        response = await client.post(
            f"{self.base_url}/sdk/sync/{sync_run_id}/scanned",
            json={"count": 1},
        )

        if not response.is_success:
            raise SdkClientError(
                f"Failed to increment scanned: {response.status_code} - {response.text}"
            )

    async def complete(
        self,
        sync_run_id: str,
        documents_scanned: int,
        documents_updated: int,
        new_state: dict[str, Any] | None = None,
    ) -> None:
        """Mark sync as completed."""
        logger.info("SDK: Completing sync_run=%s", sync_run_id)

        payload: dict[str, Any] = {
            "documents_scanned": documents_scanned,
            "documents_updated": documents_updated,
        }
        if new_state is not None:
            payload["new_state"] = new_state

        client = await self._get_client()
        response = await client.post(
            f"{self.base_url}/sdk/sync/{sync_run_id}/complete",
            json=payload,
        )

        if not response.is_success:
            raise SdkClientError(
                f"Failed to complete: {response.status_code} - {response.text}"
            )

    async def fail(self, sync_run_id: str, error: str) -> None:
        """Mark sync as failed."""
        logger.info("SDK: Failing sync_run=%s: %s", sync_run_id, error)

        client = await self._get_client()
        response = await client.post(
            f"{self.base_url}/sdk/sync/{sync_run_id}/fail",
            json={"error": error},
        )

        if not response.is_success:
            raise SdkClientError(
                f"Failed to mark as failed: {response.status_code} - {response.text}"
            )

    async def register(self, manifest: dict) -> None:
        """Register this connector with the connector manager."""
        logger.debug("SDK: Registering connector")

        client = await self._get_client()
        response = await client.post(
            f"{self.base_url}/sdk/register",
            json=manifest,
        )

        if not response.is_success:
            raise SdkClientError(
                f"Failed to register: {response.status_code} - {response.text}"
            )

    async def close(self) -> None:
        """Close the HTTP client."""
        if self._client is not None:
            await self._client.aclose()
            self._client = None

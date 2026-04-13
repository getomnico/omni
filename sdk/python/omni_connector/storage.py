import asyncio
import base64
import logging
from typing import TYPE_CHECKING

if TYPE_CHECKING:
    from .client import SdkClient

from .exceptions import ServiceOverloadedError

logger = logging.getLogger(__name__)

_MAX_RETRIES = 5


class ContentStorage:
    """Content storage that delegates to connector-manager via SDK client."""

    def __init__(self, sdk_client: "SdkClient", sync_run_id: str):
        self._client = sdk_client
        self._sync_run_id = sync_run_id

    async def save(
        self,
        content: str | bytes,
        content_type: str = "text/plain",
    ) -> str:
        """Store content and return content_id (ULID)."""
        if isinstance(content, bytes):
            content = content.decode("utf-8")

        return await self._client.store_content(
            self._sync_run_id,
            content,
            content_type,
        )

    async def extract_and_store_content(
        self,
        data: bytes,
        mime_type: str,
        filename: str | None = None,
    ) -> str:
        """Extract text from binary file content and store it, returning content_id.

        The connector manager handles extraction based on MIME type.
        Retries with exponential backoff when the extraction service is overloaded.
        """
        for attempt in range(_MAX_RETRIES):
            try:
                return await self._client.extract_and_store_content(
                    self._sync_run_id,
                    data,
                    mime_type,
                    filename,
                )
            except ServiceOverloadedError as e:
                if attempt == _MAX_RETRIES - 1:
                    raise
                wait = e.retry_after * (1.5**attempt)
                logger.warning(
                    "Extraction service overloaded for file %s, retrying in %.0fs (%d/%d)",
                    filename,
                    wait,
                    attempt + 1,
                    _MAX_RETRIES,
                )
                await asyncio.sleep(wait)
        raise RuntimeError("unreachable")

    async def extract_text(
        self,
        data: bytes,
        mime_type: str,
        filename: str | None = None,
    ) -> str:
        """Extract text from binary file content without storing.

        Use when you need to post-process or combine text before storing.
        Retries with exponential backoff when the extraction service is overloaded.
        """
        for attempt in range(_MAX_RETRIES):
            try:
                return await self._client.extract_text(
                    self._sync_run_id,
                    data,
                    mime_type,
                    filename,
                )
            except ServiceOverloadedError as e:
                if attempt == _MAX_RETRIES - 1:
                    raise
                wait = e.retry_after * (1.5**attempt)
                logger.warning(
                    "Extraction service overloaded for file %s, retrying in %.0fs (%d/%d)",
                    filename,
                    wait,
                    attempt + 1,
                    _MAX_RETRIES,
                )
                await asyncio.sleep(wait)
        raise RuntimeError("unreachable")

    async def save_binary(
        self,
        content: bytes,
        content_type: str = "application/octet-stream",
    ) -> str:
        """Store binary content as base64."""
        encoded = base64.b64encode(content).decode("utf-8")
        return await self._client.store_content(
            self._sync_run_id,
            encoded,
            content_type,
        )

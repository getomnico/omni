import base64
import logging
from typing import TYPE_CHECKING

if TYPE_CHECKING:
    from .client import SdkClient

logger = logging.getLogger(__name__)


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

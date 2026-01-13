"""Main HubSpotConnector class."""

import logging
from typing import Any

from omni_connector import Connector, Document, SyncContext

from .client import AuthenticationError, HubSpotClient, HubSpotError
from .config import HUBSPOT_OBJECT_TYPES
from .mappers import generate_content, map_hubspot_object_to_document
from .pagination import paginate_all

logger = logging.getLogger(__name__)


class HubSpotConnector(Connector):
    """HubSpot CRM connector for Omni."""

    @property
    def name(self) -> str:
        return "hubspot"

    @property
    def version(self) -> str:
        return "1.0.0"

    @property
    def sync_modes(self) -> list[str]:
        return ["full"]

    async def sync(
        self,
        source_config: dict[str, Any],
        credentials: dict[str, Any],
        state: dict[str, Any] | None,
        ctx: SyncContext,
    ) -> None:
        """
        Execute full sync of all HubSpot CRM objects.

        Args:
            source_config: Source configuration (may contain portal_id)
            credentials: Must contain 'access_token'
            state: Previous sync state (unused for full sync)
            ctx: Sync context with emit(), complete(), etc.
        """
        access_token = credentials.get("access_token")
        if not access_token:
            await ctx.fail("Missing access_token in credentials")
            return

        portal_id = source_config.get("portal_id") or credentials.get("portal_id")

        client = HubSpotClient(access_token=access_token)

        # Test connection first
        try:
            await client.test_connection()
        except AuthenticationError as e:
            await ctx.fail(f"Authentication failed: {e}")
            return
        except HubSpotError as e:
            await ctx.fail(f"Connection test failed: {e}")
            return

        logger.info("Starting HubSpot sync for portal %s", portal_id or "unknown")

        try:
            # Sync each object type sequentially
            for object_type in HUBSPOT_OBJECT_TYPES:
                if ctx.is_cancelled():
                    await ctx.fail("Cancelled by user")
                    return

                await self._sync_object_type(client, object_type, portal_id, ctx)

            await ctx.complete(new_state={})
            logger.info(
                "Sync completed: %d scanned, %d emitted",
                ctx.documents_scanned,
                ctx.documents_emitted,
            )
        except AuthenticationError as e:
            logger.error("Authentication error during sync: %s", e)
            await ctx.fail(f"Authentication failed: {e}")
        except Exception as e:
            logger.exception("Sync failed with unexpected error")
            await ctx.fail(str(e))

    async def _sync_object_type(
        self,
        client: HubSpotClient,
        object_type: str,
        portal_id: str | None,
        ctx: SyncContext,
    ) -> None:
        """
        Sync all objects of a specific type.

        Args:
            client: HubSpot API client
            object_type: Type of object to sync
            portal_id: HubSpot portal ID for URL generation
            ctx: Sync context
        """
        logger.info("Syncing %s objects", object_type)
        count = 0

        try:
            async for obj in paginate_all(client, object_type):
                if ctx.is_cancelled():
                    logger.info("Sync cancelled during %s sync", object_type)
                    return

                await ctx.increment_scanned()
                count += 1

                try:
                    doc = await self._process_object(object_type, obj, portal_id, ctx)
                    if doc:
                        await ctx.emit(doc)
                except Exception as e:
                    external_id = f"{object_type}:{obj.get('id', 'unknown')}"
                    logger.warning("Error processing %s: %s", external_id, e)
                    await ctx.emit_error(external_id, str(e))
        except HubSpotError as e:
            logger.error("Error fetching %s objects: %s", object_type, e)
            # Report the failure so user knows this object type was skipped
            await ctx.emit_error(
                f"{object_type}:*",
                f"Failed to fetch {object_type}: {e}",
            )

        logger.info("Finished syncing %s: %d objects processed", object_type, count)

    async def _process_object(
        self,
        object_type: str,
        obj: dict[str, Any],
        portal_id: str | None,
        ctx: SyncContext,
    ) -> Document:
        """
        Process a single HubSpot object into a Document.

        Args:
            object_type: Type of object
            obj: HubSpot object data
            portal_id: HubSpot portal ID
            ctx: Sync context

        Returns:
            Document instance or None if processing fails
        """
        # Generate content from object properties
        content = generate_content(object_type, obj)
        content_id = await ctx.content_storage.save(content, "text/plain")

        return map_hubspot_object_to_document(
            object_type=object_type,
            obj=obj,
            content_id=content_id,
            portal_id=portal_id,
        )

"""Main MicrosoftConnector class."""

import logging
from typing import Any

from omni_connector import Connector, SyncContext

from .auth import MSGraphAuth
from .graph_client import AuthenticationError, GraphAPIError, GraphClient
from .syncers.calendar import CalendarSyncer
from .syncers.mail import MailSyncer
from .syncers.onedrive import OneDriveSyncer
from .syncers.sharepoint import SharePointSyncer

logger = logging.getLogger(__name__)

ALL_SERVICES = ["onedrive", "mail", "calendar", "sharepoint"]


class MicrosoftConnector(Connector):
    """Microsoft 365 connector for Omni.

    Syncs OneDrive files, Outlook mail, Outlook calendar events,
    and SharePoint document libraries via the Microsoft Graph API.
    """

    @property
    def name(self) -> str:
        return "microsoft"

    @property
    def version(self) -> str:
        return "1.0.0"

    @property
    def sync_modes(self) -> list[str]:
        return ["full", "incremental"]

    async def sync(
        self,
        source_config: dict[str, Any],
        credentials: dict[str, Any],
        state: dict[str, Any] | None,
        ctx: SyncContext,
    ) -> None:
        try:
            auth = MSGraphAuth.from_credentials(credentials)
        except ValueError as e:
            await ctx.fail(str(e))
            return

        client = GraphClient(auth)

        try:
            await client.test_connection()
        except AuthenticationError as e:
            await ctx.fail(f"Authentication failed: {e}")
            return
        except GraphAPIError as e:
            await ctx.fail(f"Connection test failed: {e}")
            return

        enabled = source_config.get("services", ALL_SERVICES)
        state = state or {}

        logger.info("Starting Microsoft sync (services=%s)", enabled)

        try:
            merged_state: dict[str, Any] = {}

            syncers = self._build_syncers(enabled, source_config)
            for syncer_name, syncer in syncers:
                if ctx.is_cancelled():
                    await ctx.fail("Cancelled by user")
                    return

                logger.info("Running %s syncer", syncer_name)
                try:
                    result_state = await syncer.sync(client, ctx, state)
                    merged_state.update(result_state)
                except GraphAPIError as e:
                    logger.error("Error in %s syncer: %s", syncer_name, e)
                    await ctx.emit_error(f"{syncer_name}:*", str(e))

            await ctx.complete(new_state=merged_state)
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
        finally:
            await client.close()

    def _build_syncers(
        self,
        enabled: list[str],
        source_config: dict[str, Any],
    ) -> list[tuple[str, Any]]:
        syncers: list[tuple[str, Any]] = []
        if "onedrive" in enabled:
            syncers.append(("onedrive", OneDriveSyncer()))
        if "mail" in enabled:
            syncers.append(("mail", MailSyncer()))
        if "calendar" in enabled:
            syncers.append(("calendar", CalendarSyncer(source_config)))
        if "sharepoint" in enabled:
            syncers.append(("sharepoint", SharePointSyncer()))
        return syncers

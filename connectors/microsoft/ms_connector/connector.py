"""Main MicrosoftConnector class."""

import logging
from typing import Any

from omni_connector import Connector, SearchOperator, SyncContext
from omni_connector.models import ActionDefinition, ActionParameter, ActionResponse

from .auth import MSGraphAuth
from .graph_client import AuthenticationError, GraphAPIError, GraphClient
from .syncers.calendar import CalendarSyncer
from .syncers.mail import MailSyncer
from .syncers.onedrive import OneDriveSyncer
from .syncers.sharepoint import SharePointSyncer

logger = logging.getLogger(__name__)

SOURCE_TYPE_TO_SYNCER = {
    "one_drive": "onedrive",
    "share_point": "sharepoint",
    "outlook": "mail",
    "outlook_calendar": "calendar",
}


class MicrosoftConnector(Connector):
    """Microsoft 365 connector for Omni.

    Syncs OneDrive files, Outlook mail, Outlook calendar events,
    and SharePoint document libraries via the Microsoft Graph API.
    Each source type maps to exactly one syncer.
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

    @property
    def search_operators(self) -> list[SearchOperator]:
        return [
            SearchOperator(
                operator="from", attribute_key="sender", value_type="person"
            ),
        ]

    @property
    def actions(self) -> list[ActionDefinition]:
        return [
            ActionDefinition(
                name="search_users",
                description="Search Microsoft 365 users by name or email",
                parameters={
                    "query": ActionParameter(
                        type="string", required=True, description="Search query"
                    ),
                },
            )
        ]

    async def execute_action(
        self,
        action: str,
        params: dict[str, Any],
        credentials: dict[str, Any],
    ) -> ActionResponse:
        if action != "search_users":
            return ActionResponse.not_supported(action)

        query = params.get("query", "").strip()
        if not query:
            return ActionResponse.success({"users": []})

        try:
            raw_creds = credentials.get("credentials", credentials)
            auth = MSGraphAuth.from_credentials(raw_creds)
            client = GraphClient(auth)
            try:
                users = await client.search_users(query, limit=20)
                return ActionResponse.success({"users": users})
            finally:
                await client.close()
        except Exception as e:
            logger.exception("search_users action failed")
            return ActionResponse.failure(str(e))

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

        graph_base_url = source_config.get("graph_base_url")
        client = (
            GraphClient(auth, base_url=graph_base_url)
            if graph_base_url
            else GraphClient(auth)
        )

        try:
            await client.test_connection()
        except AuthenticationError as e:
            await ctx.fail(f"Authentication failed: {e}")
            return
        except GraphAPIError as e:
            await ctx.fail(f"Connection test failed: {e}")
            return

        syncer_key = SOURCE_TYPE_TO_SYNCER.get(ctx.source_type or "")
        if syncer_key is None:
            await ctx.fail(f"Unknown source type: {ctx.source_type}")
            return

        syncer = self._create_syncer(syncer_key, source_config)
        state = state or {}

        logger.info("Starting Microsoft sync (syncer=%s)", syncer_key)

        try:
            result_state = await syncer.sync(
                client, ctx, state, source_config=source_config
            )
            await ctx.complete(new_state=result_state)
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

    def _create_syncer(self, syncer_key: str, source_config: dict[str, Any]) -> Any:
        if syncer_key == "onedrive":
            return OneDriveSyncer()
        elif syncer_key == "mail":
            return MailSyncer()
        elif syncer_key == "calendar":
            return CalendarSyncer(source_config)
        elif syncer_key == "sharepoint":
            return SharePointSyncer()
        raise ValueError(f"Unknown syncer key: {syncer_key}")

"""Main MicrosoftConnector class."""

import logging
from datetime import datetime, timedelta, timezone
from typing import Any
from starlette.responses import Response

from omni_connector import Connector, SearchOperator, SyncContext
from omni_connector.models import (
    ActionDefinition,
    ActionResponse,
    OAuthManifestConfig,
    OAuthScopeSet,
    Source,
)

from .auth import MSGraphAuth, parse_ms_credentials
from .graph_client import AuthenticationError, GraphAPIError, GraphClient
from .mappers import serialize_event
from .syncers.calendar import CalendarSyncer
from .syncers.mail import MailSyncer
from .syncers.onedrive import OneDriveSyncer
from .syncers.sharepoint import SharePointSyncer
from .syncers.teams import TeamsSyncer

logger = logging.getLogger(__name__)

SOURCE_TYPE_TO_SYNCER = {
    "one_drive": "onedrive",
    "share_point": "sharepoint",
    "outlook": "mail",
    "outlook_calendar": "calendar",
    "ms_teams": "teams",
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
    def display_name(self) -> str:
        return "Microsoft"

    @property
    def version(self) -> str:
        return "1.0.0"

    @property
    def source_types(self) -> list[str]:
        return ["one_drive", "share_point", "outlook", "outlook_calendar", "ms_teams"]

    @property
    def description(self) -> str:
        return "Connect to OneDrive, SharePoint, Outlook mail, calendar, and Teams"

    @property
    def sync_modes(self) -> list[str]:
        return ["full", "incremental"]

    @property
    def search_operators(self) -> list[SearchOperator]:
        return [
            SearchOperator(
                operator="from", attribute_key="sender", value_type="person"
            ),
            SearchOperator(operator="to", attribute_key="to", value_type="person"),
            SearchOperator(operator="cc", attribute_key="cc", value_type="person"),
        ]

    def oauth_config(self) -> OAuthManifestConfig | None:
        # Endpoints below default to the `organizations` tenant; the web layer
        # overrides them with tenant-specific URLs from connector_configs at
        # flow time (admin's tenant_id is materialized into the stored
        # auth/token endpoints during setup).
        return OAuthManifestConfig(
            provider="microsoft",
            auth_endpoint="https://login.microsoftonline.com/organizations/oauth2/v2.0/authorize",
            token_endpoint="https://login.microsoftonline.com/organizations/oauth2/v2.0/token",
            userinfo_endpoint="https://graph.microsoft.com/v1.0/me",
            userinfo_email_field="userPrincipalName",
            identity_scopes=[
                "openid",
                "profile",
                "email",
                "offline_access",
                "User.Read",
            ],
            scopes={
                "one_drive": OAuthScopeSet(
                    read=["Files.Read.All"],
                    write=["Files.ReadWrite.All"],
                ),
                "share_point": OAuthScopeSet(
                    read=["Sites.Read.All", "Files.Read.All"],
                    write=["Sites.ReadWrite.All", "Files.ReadWrite.All"],
                ),
                "outlook": OAuthScopeSet(
                    read=["Mail.Read"],
                    write=["Mail.ReadWrite", "Mail.Send"],
                ),
                "outlook_calendar": OAuthScopeSet(
                    read=["Calendars.Read"],
                    write=["Calendars.ReadWrite"],
                ),
                "ms_teams": OAuthScopeSet(
                    read=[
                        "Channel.ReadBasic.All",
                        "ChannelMessage.Read.All",
                        "Team.ReadBasic.All",
                    ],
                    write=["ChannelMessage.Send"],
                ),
            },
            scope_separator=" ",
            extra_auth_params={},
        )

    @property
    def actions(self) -> list[ActionDefinition]:
        return [
            ActionDefinition(
                name="search_users",
                description="Search Microsoft 365 users by name or email",
                input_schema={
                    "type": "object",
                    "properties": {
                        "query": {"type": "string", "description": "Search query"},
                    },
                    "required": ["query"],
                },
                admin_only=True,
            ),
            ActionDefinition(
                name="fetch_file",
                description="Download a file from OneDrive or SharePoint",
                mode="read",
                input_schema={
                    "type": "object",
                    "properties": {
                        "file_id": {
                            "type": "string",
                            "description": "External file ID (e.g. onedrive:driveId:itemId)",
                        },
                    },
                    "required": ["file_id"],
                },
            ),
            ActionDefinition(
                name="list_events",
                description=(
                    "List the caller's Outlook calendar events in a time range. "
                    "Defaults to the next 7 days starting today (UTC)."
                ),
                mode="read",
                source_types=["outlook_calendar"],
                required_scopes=["Calendars.Read"],
                input_schema={
                    "type": "object",
                    "properties": {
                        "start": {
                            "type": "string",
                            "description": "Range start as ISO 8601 datetime or date "
                            "(e.g. 2026-09-16T09:00:00 or 2026-09-16). "
                            "Naive values are interpreted as UTC.",
                        },
                        "end": {
                            "type": "string",
                            "description": "Range end as ISO 8601 datetime or date. "
                            "Naive values are interpreted as UTC.",
                        },
                        "top": {
                            "type": "integer",
                            "description": "Maximum number of events to return (1-50, default 25)",
                        },
                    },
                },
            ),
            ActionDefinition(
                name="create_event",
                description=(
                    "Create an event on the caller's Outlook calendar. "
                    "Times are ISO 8601; naive values are interpreted as UTC."
                ),
                mode="write",
                source_types=["outlook_calendar"],
                required_scopes=["Calendars.ReadWrite"],
                input_schema={
                    "type": "object",
                    "properties": {
                        "subject": {"type": "string"},
                        "start": {
                            "type": "string",
                            "description": "Start as ISO 8601 datetime",
                        },
                        "end": {
                            "type": "string",
                            "description": "End as ISO 8601 datetime",
                        },
                        "attendees": {
                            "type": "array",
                            "items": {"type": "string"},
                            "description": "Attendee email addresses",
                        },
                        "location": {"type": "string"},
                        "body": {
                            "type": "string",
                            "description": "Plain-text event description",
                        },
                        "online_meeting": {
                            "type": "boolean",
                            "description": "Create as a Teams online meeting",
                        },
                    },
                    "required": ["subject", "start", "end"],
                },
            ),
        ]

    async def execute_action(
        self,
        action: str,
        params: dict[str, Any],
        credentials: dict[str, Any],
        source: Source | None = None,
        actor_email: str | None = None,
    ) -> Response:
        if action == "search_users":
            return await self._action_search_users(params, credentials)
        elif action == "fetch_file":
            return await self._action_fetch_file(params, credentials)
        elif action == "list_events":
            return await self._action_list_events(params, credentials, source)
        elif action == "create_event":
            return await self._action_create_event(params, credentials, source)
        return ActionResponse.not_supported(action).to_response(status_code=404)

    @staticmethod
    def _action_graph_client(
        credentials: dict[str, Any], source: Source | None
    ) -> GraphClient:
        auth = MSGraphAuth.from_credentials(
            parse_ms_credentials(credentials.get("credentials", credentials))
        )
        graph_base_url = (source.config if source else {}).get("graph_base_url")
        if graph_base_url:
            return GraphClient(auth, base_url=graph_base_url)
        return GraphClient(auth)

    @staticmethod
    def _parse_action_datetime(
        value: str, field: str
    ) -> datetime:
        try:
            parsed = datetime.fromisoformat(value.strip().replace("Z", "+00:00"))
        except (ValueError, AttributeError) as e:
            raise ValueError(
                f"Invalid {field}: {value!r}. Expected an ISO 8601 datetime."
            ) from e
        if parsed.tzinfo is None:
            return parsed.replace(tzinfo=timezone.utc)
        return parsed.astimezone(timezone.utc)

    async def _action_list_events(
        self,
        params: dict[str, Any],
        credentials: dict[str, Any],
        source: Source | None,
    ) -> Response:
        try:
            start_raw = params.get("start")
            end_raw = params.get("end")
            if start_raw is not None:
                start = self._parse_action_datetime(start_raw, "start")
            else:
                start = datetime.now(timezone.utc).replace(
                    hour=0, minute=0, second=0, microsecond=0
                )
            if end_raw is not None:
                end = self._parse_action_datetime(end_raw, "end")
            else:
                end = start + timedelta(days=7)
            if end <= start:
                return ActionResponse.failure(
                    "end must be after start"
                ).to_response(status_code=400)
            top_raw = params.get("top", 25)
            try:
                top = max(1, min(int(top_raw), 50))
            except (TypeError, ValueError):
                return ActionResponse.failure(
                    f"Invalid top: {top_raw!r}. Expected an integer."
                ).to_response(status_code=400)
        except ValueError as e:
            return ActionResponse.failure(str(e)).to_response(status_code=400)

        client = self._action_graph_client(credentials, source)
        try:
            events = await client.list_calendar_events(
                start.isoformat(), end.isoformat(), top=top
            )
            return ActionResponse.success(
                {
                    "events": [serialize_event(e) for e in events],
                    "count": len(events),
                    "range": {
                        "start": start.isoformat(),
                        "end": end.isoformat(),
                    },
                }
            ).to_response()
        except AuthenticationError as e:
            logger.warning("list_events action failed authentication: %s", e)
            return ActionResponse.failure(str(e)).to_response(status_code=401)
        except GraphAPIError as e:
            logger.exception("list_events action failed")
            return ActionResponse.failure(e.diagnostic()).to_response(status_code=502)
        finally:
            await client.close()

    async def _action_create_event(
        self,
        params: dict[str, Any],
        credentials: dict[str, Any],
        source: Source | None,
    ) -> Response:
        subject = (params.get("subject") or "").strip()
        start_raw = params.get("start")
        end_raw = params.get("end")
        if not subject or not start_raw or not end_raw:
            return ActionResponse.failure(
                "Missing required parameters: subject, start, end"
            ).to_response(status_code=400)

        try:
            start = self._parse_action_datetime(start_raw, "start")
            end = self._parse_action_datetime(end_raw, "end")
        except ValueError as e:
            return ActionResponse.failure(str(e)).to_response(status_code=400)
        if end <= start:
            return ActionResponse.failure("end must be after start").to_response(
                status_code=400
            )

        attendees_raw = params.get("attendees") or []
        if not isinstance(attendees_raw, list):
            return ActionResponse.failure(
                "attendees must be a list of email addresses"
            ).to_response(status_code=400)
        attendees: list[dict[str, Any]] = []
        for attendee in attendees_raw:
            address = str(attendee).strip()
            if "@" not in address:
                return ActionResponse.failure(
                    f"Invalid attendee email: {attendee!r}"
                ).to_response(status_code=400)
            attendees.append(
                {"emailAddress": {"address": address}, "type": "required"}
            )

        event: dict[str, Any] = {
            "subject": subject,
            "start": {
                "dateTime": start.strftime("%Y-%m-%dT%H:%M:%S"),
                "timeZone": "UTC",
            },
            "end": {
                "dateTime": end.strftime("%Y-%m-%dT%H:%M:%S"),
                "timeZone": "UTC",
            },
        }
        if attendees:
            event["attendees"] = attendees
        location = (params.get("location") or "").strip()
        if location:
            event["location"] = {"displayName": location}
        body = (params.get("body") or "").strip()
        if body:
            event["body"] = {"contentType": "text", "content": body}
        if params.get("online_meeting"):
            event["isOnlineMeeting"] = True

        client = self._action_graph_client(credentials, source)
        try:
            created = await client.create_event(event)
            return ActionResponse.success(
                {"event": serialize_event(created)}
            ).to_response()
        except AuthenticationError as e:
            logger.warning("create_event action failed authentication: %s", e)
            return ActionResponse.failure(str(e)).to_response(status_code=401)
        except GraphAPIError as e:
            logger.exception("create_event action failed")
            return ActionResponse.failure(e.diagnostic()).to_response(status_code=502)
        finally:
            await client.close()

    async def _action_search_users(
        self,
        params: dict[str, Any],
        credentials: dict[str, Any],
    ) -> Response:
        query = params.get("query", "").strip()
        if not query:
            return ActionResponse.success({"users": []}).to_response()

        try:
            raw_creds = credentials.get("credentials", credentials)
            auth = MSGraphAuth.from_credentials(parse_ms_credentials(raw_creds))
            client = GraphClient(auth)
            try:
                users = await client.search_users(query, limit=20)
                return ActionResponse.success({"users": users}).to_response()
            finally:
                await client.close()
        except Exception as e:
            logger.exception("search_users action failed")
            return ActionResponse.failure(str(e)).to_response(status_code=500)

    async def _action_fetch_file(
        self,
        params: dict[str, Any],
        credentials: dict[str, Any],
    ) -> Response:
        file_id = params.get("file_id", "").strip()
        if not file_id:
            return ActionResponse.failure(
                "Missing required parameter: file_id"
            ).to_response(status_code=400)

        # Parse external_id: onedrive:{drive_id}:{item_id} or sharepoint:{site_id}:{drive_id}:{item_id}
        parts = file_id.split(":")
        if parts[0] == "onedrive" and len(parts) == 3:
            drive_id = parts[1]
            item_id = parts[2]
        elif parts[0] == "sharepoint" and len(parts) == 4:
            drive_id = parts[2]
            item_id = parts[3]
        else:
            return ActionResponse.failure(
                f"Invalid file_id format: {file_id}. "
                "Expected onedrive:{{drive_id}}:{{item_id}} or "
                "sharepoint:{{site_id}}:{{drive_id}}:{{item_id}}"
            ).to_response(status_code=400)

        try:
            raw_creds = credentials.get("credentials", credentials)
            auth = MSGraphAuth.from_credentials(parse_ms_credentials(raw_creds))
            client = GraphClient(auth)
            try:
                metadata = await client.get_drive_item_metadata(drive_id, item_id)
                file_name = metadata.get("name", "download")
                mime_type = metadata.get("file", {}).get(
                    "mimeType", "application/octet-stream"
                )

                data = await client.get_binary(
                    f"/drives/{drive_id}/items/{item_id}/content"
                )

                logger.info(
                    "fetch_file: returning '%s' (%d bytes, %s)",
                    file_name,
                    len(data),
                    mime_type,
                )

                return Response(
                    content=data,
                    media_type=mime_type,
                    headers={
                        "X-File-Name": file_name,
                        "Content-Length": str(len(data)),
                    },
                )
            finally:
                await client.close()
        except Exception as e:
            logger.exception("fetch_file action failed")
            return ActionResponse.failure(str(e)).to_response(status_code=500)

    async def sync(
        self,
        source_config: dict[str, Any],
        credentials: dict[str, Any],
        checkpoint: dict[str, Any] | None,
        ctx: SyncContext,
    ) -> None:
        try:
            auth = MSGraphAuth.from_credentials(parse_ms_credentials(credentials))
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

        # Sync group memberships and build ID→email caches for permission resolution
        fetched_groups = await self._sync_groups(client, ctx)
        group_cache = {
            g["id"]: (g.get("mail") or "").lower()
            for g in fetched_groups
            if g.get("mail")
        }

        try:
            all_users = await client.list_users()
        except Exception as e:
            logger.warning("Failed to list users for cache: %s", e)
            all_users = []
        user_cache = {
            u["id"]: (u.get("mail") or u.get("userPrincipalName") or "").lower()
            for u in all_users
            if u.get("mail") or u.get("userPrincipalName")
        }

        syncer = self._create_syncer(syncer_key, source_config)
        checkpoint = checkpoint or {}

        logger.info("Starting Microsoft sync (syncer=%s)", syncer_key)

        try:
            result_checkpoint = await syncer.sync(
                client,
                ctx,
                checkpoint,
                source_config=source_config,
                user_cache=user_cache,
                group_cache=group_cache,
            )
            await ctx.complete(checkpoint=result_checkpoint)
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

    async def _sync_groups(
        self, client: GraphClient, ctx: SyncContext
    ) -> list[dict[str, Any]]:
        """Sync Entra ID group memberships. Returns fetched groups for cache building."""
        try:
            groups = await client.list_groups()
        except Exception as e:
            logger.warning("Failed to list groups: %s. Skipping group sync.", e)
            return []

        logger.info("Found %d groups, syncing memberships", len(groups))
        total_members = 0

        for group in groups:
            group_id = group["id"]
            group_email = (group.get("mail") or "").lower()
            group_name = group.get("displayName")

            if not group_email:
                continue

            try:
                members = await client.list_group_members(group_id)
            except Exception as e:
                logger.warning(
                    "Failed to list members for group %s: %s", group_email, e
                )
                continue

            member_emails = [
                (m.get("mail") or m.get("userPrincipalName") or "").lower()
                for m in members
            ]
            member_emails = [e for e in member_emails if e]

            total_members += len(member_emails)

            try:
                await ctx.emit_group_membership(
                    group_email=group_email,
                    member_emails=member_emails,
                    group_name=group_name,
                )
            except Exception as e:
                logger.warning(
                    "Failed to emit group membership event for %s: %s",
                    group_email,
                    e,
                )

        logger.info(
            "Group sync complete: %d groups, %d total memberships",
            len(groups),
            total_members,
        )
        return groups

    def _create_syncer(self, syncer_key: str, source_config: dict[str, Any]) -> Any:
        if syncer_key == "onedrive":
            return OneDriveSyncer()
        elif syncer_key == "mail":
            return MailSyncer()
        elif syncer_key == "calendar":
            return CalendarSyncer()
        elif syncer_key == "sharepoint":
            return SharePointSyncer()
        elif syncer_key == "teams":
            return TeamsSyncer()
        raise ValueError(f"Unknown syncer key: {syncer_key}")

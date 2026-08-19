"""Salesforce connector for Omni."""

from __future__ import annotations

import asyncio
import logging
from collections.abc import AsyncIterator, Callable, Mapping
from dataclasses import dataclass, replace
from datetime import UTC, datetime, timedelta

from fastapi.responses import JSONResponse
from omni_connector import (
    ActionDefinition,
    Connector,
    PersonSyncRecord,
    SearchOperator,
    SyncContext,
    SyncMode,
)
from omni_connector.models import Source

from .actions import ACTION_DEFINITIONS, execute_action
from .client import (
    AuthenticationError,
    DeletedRecord,
    SalesforceClient,
    SalesforceClientError,
)
from .config import (
    CHECKPOINT_INTERVAL,
    DELTA_OVERLAP_SECONDS,
    PAGE_SIZE,
    SalesforceObjectConfig,
    enabled_object_configs,
    schema_fingerprint,
)
from .mappers import (
    RecordModel,
    attributes_for,
    generate_content,
    map_record_to_document,
)
from .models import (
    AccountRecord,
    CaseRecord,
    ContactRecord,
    GroupMemberRecord,
    GroupRecord,
    LeadRecord,
    OpportunityRecord,
    RecordCursor,
    RoleRecord,
    SalesforceAuth,
    SalesforceCheckpoint,
    SalesforceSourceConfig,
    ShareRecord,
    TaskRecord,
    UserRecord,
)
from .pagination import (
    cursor_from_record,
    delta_scan_soql,
    full_scan_soql,
    iter_query_pages,
)
from .permissions import (
    RecordGrants,
    SalesforceDirectory,
    build_directory,
)

logger = logging.getLogger(__name__)

USER_ID_PREFIX = "005"

USER_FIELDS = (
    "Id",
    "Name",
    "FirstName",
    "LastName",
    "Email",
    "Title",
    "Department",
    "ManagerId",
    "UserRoleId",
    "IsActive",
    "EmployeeNumber",
    "SystemModstamp",
)
GROUP_FIELDS = ("Id", "Name", "Type")
GROUP_MEMBER_FIELDS = ("Id", "GroupId", "UserOrGroupId")
ROLE_FIELDS = ("Id", "Name", "ParentRoleId")

_RECORD_PARSERS: dict[str, Callable[[Mapping[str, object]], RecordModel]] = {
    "Account": AccountRecord.from_record,
    "Contact": ContactRecord.from_record,
    "Opportunity": OpportunityRecord.from_record,
    "Lead": LeadRecord.from_record,
    "Case": CaseRecord.from_record,
    "Task": TaskRecord.from_record,
}


@dataclass(frozen=True)
class PeopleSnapshot:
    """Previous people state, used to emit only changed person/group events."""

    user_modstamps: dict[str, str]  # user id -> SystemModstamp ISO
    active_emails: frozenset[str]
    memberships: dict[str, frozenset[str]]  # group email -> member emails
    group_names: dict[str, str]


class SalesforceConnector(Connector):
    """Salesforce CRM connector for Omni."""

    @property
    def name(self) -> str:
        return "salesforce"

    @property
    def display_name(self) -> str:
        return "Salesforce"

    @property
    def version(self) -> str:
        return "1.0.0"

    @property
    def source_types(self) -> list[str]:
        return ["salesforce"]

    @property
    def description(self) -> str:
        return "Index Salesforce accounts, contacts, opportunities, leads, cases and tasks"

    @property
    def sync_modes(self) -> list[str]:
        return ["full", "incremental", "realtime"]

    @property
    def search_operators(self) -> list[SearchOperator]:
        return [
            SearchOperator(operator="owner", attribute_key="owner_email", value_type="person"),
            SearchOperator(operator="status", attribute_key="status", value_type="text"),
            SearchOperator(operator="priority", attribute_key="priority", value_type="text"),
            SearchOperator(operator="stage", attribute_key="stage", value_type="text"),
            SearchOperator(operator="account", attribute_key="account_name", value_type="text"),
            SearchOperator(operator="industry", attribute_key="industry", value_type="text"),
            SearchOperator(operator="lead_source", attribute_key="lead_source", value_type="text"),
        ]

    @property
    def actions(self) -> list[ActionDefinition]:
        return list(ACTION_DEFINITIONS)

    async def execute_action(
        self,
        action: str,
        params: Mapping[str, object],
        credentials: Mapping[str, object],
        source: Source | None = None,
        actor_email: str | None = None,
    ) -> JSONResponse:
        return await execute_action(action, params, credentials)

    async def sync(
        self,
        source_config: Mapping[str, object],
        credentials: Mapping[str, object],
        checkpoint: Mapping[str, object] | None,
        ctx: SyncContext,
    ) -> None:
        try:
            auth = SalesforceAuth.from_mapping(credentials)
        except ValueError as e:
            await ctx.fail(str(e))
            return

        config = SalesforceSourceConfig.from_mapping(source_config)
        client = SalesforceClient(auth, instance_url=config.instance_url)

        try:
            await client.test_connection()
        except AuthenticationError as e:
            await ctx.fail(f"Authentication failed: {e}")
            return
        except SalesforceClientError as e:
            await ctx.fail(f"Connection test failed: {e}")
            return

        logger.info(
            "Starting Salesforce %s sync for %s",
            ctx.sync_mode.value,
            client.instance_url,
        )

        # Schema fingerprint: when the synced field/object set changes, saved
        # watermarks no longer cover everything the index expects, so a full
        # resync is forced.
        fingerprint = schema_fingerprint(config.enabled_objects, config.public_read_objects)
        run_checkpoint = SalesforceCheckpoint.from_mapping(checkpoint)
        if ctx.connector_state.get("schema_fingerprint") != fingerprint:
            logger.info("Schema fingerprint changed; forcing full resync")
            run_checkpoint = SalesforceCheckpoint()
            await ctx.save_connector_state({"schema_fingerprint": fingerprint})

        # Persist the run-scoped checkpoint before any work, so a crash before
        # the first page cannot resume from stale watermarks.
        await ctx.save_checkpoint(run_checkpoint.to_json())

        try:
            if ctx.sync_mode == SyncMode.REALTIME:
                await self._realtime_sync(client, config, run_checkpoint, ctx)
                return

            run_checkpoint = await self._run_scheduled_sync(client, config, run_checkpoint, ctx)

            await ctx.complete(checkpoint=run_checkpoint.to_json())
            logger.info(
                "Sync completed: %d scanned, %d emitted",
                ctx.documents_scanned,
                ctx.documents_emitted,
            )
        except AuthenticationError as e:
            logger.error("Authentication error during sync: %s", e)
            await ctx.fail(f"Authentication failed: {e}")
        except SalesforceClientError as e:
            logger.error("Salesforce API error during sync: %s", e)
            await ctx.fail(str(e))
        except Exception as e:
            logger.exception("Sync failed with unexpected error")
            await ctx.fail(str(e))

    async def _run_scheduled_sync(
        self,
        client: SalesforceClient,
        config: SalesforceSourceConfig,
        checkpoint: SalesforceCheckpoint,
        ctx: SyncContext,
    ) -> SalesforceCheckpoint:
        """One-shot sync: people, shares, records (full or delta), deletes."""
        configs = enabled_object_configs(config.enabled_objects)

        directory, _ = await self._sync_people(client, config, ctx, previous=None)
        if ctx.is_cancelled():
            return checkpoint

        share_grants = await self._sync_shares(client, configs, directory, ctx)
        if ctx.is_cancelled():
            return checkpoint

        pass_started_at = datetime.now(UTC)
        checkpoint = await self._sync_records(
            client=client,
            configs=configs,
            directory=directory,
            share_grants=share_grants,
            source_config=config,
            checkpoint=checkpoint,
            ctx=ctx,
            incremental=ctx.sync_mode == SyncMode.INCREMENTAL,
            pass_started_at=pass_started_at,
        )
        if ctx.is_cancelled():
            return checkpoint

        await self._sync_deletes(
            client=client,
            configs=configs,
            checkpoint=checkpoint,
            ctx=ctx,
            window_end=pass_started_at,
        )
        return checkpoint

    async def _sync_people(
        self,
        client: SalesforceClient,
        config: SalesforceSourceConfig,
        ctx: SyncContext,
        previous: PeopleSnapshot | None,
    ) -> tuple[SalesforceDirectory, PeopleSnapshot]:
        """Query users/groups/roles and emit person and group-membership events.

        With ``previous`` set, only changed people and memberships are emitted;
        otherwise everything is emitted (full/incremental runs).
        """
        users: list[UserRecord] = []
        groups: list[GroupRecord] = []
        group_members: list[GroupMemberRecord] = []
        roles: list[RoleRecord] = []

        if config.sync_users:
            async for page in iter_query_pages(
                client, f"SELECT {', '.join(USER_FIELDS)} FROM User"
            ):
                users.extend(UserRecord.from_record(r) for r in page.records)
        if config.sync_groups:
            async for page in iter_query_pages(
                client,
                f"SELECT {', '.join(GROUP_FIELDS)} FROM Group "
                "WHERE Type IN ('Public', 'Queue', 'Regular')",
            ):
                groups.extend(GroupRecord.from_record(r) for r in page.records)
            async for page in iter_query_pages(
                client, f"SELECT {', '.join(GROUP_MEMBER_FIELDS)} FROM GroupMember"
            ):
                group_members.extend(GroupMemberRecord.from_record(r) for r in page.records)
            async for page in iter_query_pages(
                client, f"SELECT {', '.join(ROLE_FIELDS)} FROM UserRole"
            ):
                roles.extend(RoleRecord.from_record(r) for r in page.records)

        directory = build_directory(users, groups, group_members, roles)
        snapshot = self._snapshot(directory, users)

        await self._emit_people(client, directory, users, snapshot, previous, ctx)
        return directory, snapshot

    def _snapshot(self, directory: SalesforceDirectory, users: list[UserRecord]) -> PeopleSnapshot:
        user_modstamps: dict[str, str] = {}
        active_emails: set[str] = set()
        for user in users:
            if user.system_modstamp is not None:
                user_modstamps[user.id] = user.system_modstamp.isoformat()
            if user.email and user.is_active:
                active_emails.add(user.email)
        memberships: dict[str, frozenset[str]] = {}
        group_names: dict[str, str] = {}
        for group_email, member_emails, name in directory.group_memberships():
            memberships[group_email] = frozenset(member_emails)
            if name:
                group_names[group_email] = name
        return PeopleSnapshot(
            user_modstamps=user_modstamps,
            active_emails=frozenset(active_emails),
            memberships=memberships,
            group_names=group_names,
        )

    async def _emit_people(
        self,
        client: SalesforceClient,
        directory: SalesforceDirectory,
        users: list[UserRecord],
        snapshot: PeopleSnapshot,
        previous: PeopleSnapshot | None,
        ctx: SyncContext,
    ) -> None:
        changed_users = (
            users
            if previous is None
            else [
                u
                for u in users
                if snapshot.user_modstamps.get(u.id) != previous.user_modstamps.get(u.id)
            ]
        )
        for user in changed_users:
            if not user.email:
                continue
            if user.is_active:
                await ctx.emit_person_sync(
                    PersonSyncRecord(
                        external_id=user.id,
                        email=user.email,
                        display_name=user.name,
                        given_name=user.first_name,
                        surname=user.last_name,
                        job_title=user.title,
                        department=user.department,
                        manager_external_id=user.manager_id,
                        employee_id=user.employee_number,
                        source_updated_at=(
                            user.system_modstamp.isoformat()
                            if user.system_modstamp is not None
                            else None
                        ),
                    )
                )
            else:
                await ctx.emit_person_deleted(user.email)

        if previous is not None:
            for email in previous.active_emails - snapshot.active_emails:
                await ctx.emit_person_deleted(email)

        for group_email, member_emails, name in directory.group_memberships():
            if previous is not None and previous.memberships.get(group_email) == frozenset(
                member_emails
            ):
                continue
            await ctx.emit_group_membership(
                group_email=group_email,
                member_emails=sorted(member_emails),
                group_name=name,
            )

    async def _sync_shares(
        self,
        client: SalesforceClient,
        configs: tuple[SalesforceObjectConfig, ...],
        directory: SalesforceDirectory,
        ctx: SyncContext,
    ) -> dict[str, RecordGrants]:
        """Query share rows for every share-enabled object and resolve grants."""
        grants_by_parent: dict[str, RecordGrants] = {}
        for config in configs:
            if config.share_object is None or config.share_parent_field is None:
                continue
            if ctx.is_cancelled():
                return grants_by_parent
            soql = (
                f"SELECT Id, {config.share_parent_field}, UserOrGroupId, "
                f"AccessLevel, RowCause FROM {config.share_object} "
                "WHERE RowCause != 'Owner' ORDER BY Id"
            )
            try:
                async for page in iter_query_pages(client, soql):
                    for raw in page.records:
                        share = ShareRecord.from_record(raw, config.share_parent_field)
                        grants = directory.share_grants((share,))
                        if not grants.users and not grants.groups:
                            continue
                        grants_by_parent[share.parent_id] = grants_by_parent.get(
                            share.parent_id, RecordGrants()
                        ).merge(grants)
            except SalesforceClientError as e:
                logger.warning("Failed to sync %s shares: %s", config.share_object, e)
                await ctx.emit_error(f"{config.name}:*", f"Failed to fetch shares: {e}")
        return grants_by_parent

    async def _sync_records(
        self,
        *,
        client: SalesforceClient,
        configs: tuple[SalesforceObjectConfig, ...],
        directory: SalesforceDirectory,
        share_grants: dict[str, RecordGrants],
        source_config: SalesforceSourceConfig,
        checkpoint: SalesforceCheckpoint,
        ctx: SyncContext,
        incremental: bool,
        pass_started_at: datetime,
    ) -> SalesforceCheckpoint:
        for config in configs:
            if ctx.is_cancelled():
                return checkpoint
            if checkpoint.records_synced.get(config.name):
                continue

            parser = _RECORD_PARSERS[config.name]
            fields = config.all_fields()
            cursor = checkpoint.record_cursors.get(config.name)
            watermark = checkpoint.watermarks.get(config.name)
            delta = incremental and watermark is not None
            if delta:
                assert watermark is not None
                soql = delta_scan_soql(config.name, fields, cursor, watermark)
            else:
                soql = full_scan_soql(config.name, fields, cursor)

            emitted_since_checkpoint = 0
            while True:
                if ctx.is_cancelled():
                    return checkpoint
                page = await client.query(soql)
                for raw in page.records:
                    record = parser(raw)
                    await self._emit_record(
                        client=client,
                        config=config,
                        record=record,
                        directory=directory,
                        share_grants=share_grants,
                        source_config=source_config,
                        ctx=ctx,
                        emit_updated=delta,
                    )
                    cursor = cursor_from_record(raw)
                    emitted_since_checkpoint += 1
                    if emitted_since_checkpoint >= CHECKPOINT_INTERVAL:
                        checkpoint = self._with_cursor(checkpoint, config.name, cursor)
                        await ctx.save_checkpoint(checkpoint.to_json())
                        emitted_since_checkpoint = 0

                if len(page.records) < PAGE_SIZE:
                    break
                if delta:
                    assert watermark is not None
                    soql = delta_scan_soql(config.name, fields, cursor, watermark)
                else:
                    soql = full_scan_soql(config.name, fields, cursor)

            checkpoint = self._with_cursor(checkpoint, config.name, cursor)
            checkpoint = replace(
                checkpoint,
                records_synced={**checkpoint.records_synced, config.name: True},
                watermarks={
                    **checkpoint.watermarks,
                    config.name: (
                        pass_started_at - timedelta(seconds=DELTA_OVERLAP_SECONDS)
                    ).isoformat(),
                },
            )
            await ctx.save_checkpoint(checkpoint.to_json())
            logger.info(
                "Finished syncing %s (%s): watermark %s",
                config.name,
                "delta" if delta else "full",
                checkpoint.watermarks[config.name],
            )
        return checkpoint

    @staticmethod
    def _with_cursor(
        checkpoint: SalesforceCheckpoint,
        object_type: str,
        cursor: RecordCursor | None,
    ) -> SalesforceCheckpoint:
        return replace(
            checkpoint,
            record_cursors={
                **checkpoint.record_cursors,
                object_type: cursor or RecordCursor(),
            },
        )

    async def _emit_record(
        self,
        *,
        client: SalesforceClient,
        config: SalesforceObjectConfig,
        record: RecordModel,
        directory: SalesforceDirectory,
        share_grants: dict[str, RecordGrants],
        source_config: SalesforceSourceConfig,
        ctx: SyncContext,
        emit_updated: bool,
    ) -> None:
        await ctx.increment_scanned()
        owner_id = getattr(record, "owner_id", None)
        owner_email = (
            directory.email_for_user(owner_id)
            if isinstance(owner_id, str) and owner_id.startswith(USER_ID_PREFIX)
            else None
        )
        grants = directory.owner_grants(owner_id, source_config.grant_access_using_hierarchies)
        share_grant = share_grants.get(record.id)
        if share_grant is not None:
            grants = grants.merge(share_grant)
        public = config.name in source_config.public_read_objects or config.public_read_default
        permissions = grants.to_permissions(public)

        content = generate_content(config.name, record)
        content_id = await ctx.content_storage.save(content, "text/plain")
        attributes = attributes_for(config.name, record, owner_email)
        document = map_record_to_document(
            object_type=config.name,
            record=record,
            content_id=content_id,
            instance_url=client.instance_url,
            owner_email=owner_email,
            permissions=permissions,
            attributes=attributes,
        )
        if emit_updated:
            await ctx.emit_updated(document)
        else:
            await ctx.emit(document)

    async def _sync_deletes(
        self,
        *,
        client: SalesforceClient,
        configs: tuple[SalesforceObjectConfig, ...],
        checkpoint: SalesforceCheckpoint,
        ctx: SyncContext,
        window_end: datetime,
    ) -> None:
        """Emit tombstones for records deleted since the previous watermark."""
        for config in configs:
            if ctx.is_cancelled():
                return
            previous_watermark = checkpoint.watermarks.get(config.name)
            if previous_watermark is None:
                continue
            try:
                window_start = datetime.fromisoformat(previous_watermark) - timedelta(
                    seconds=DELTA_OVERLAP_SECONDS
                )
            except ValueError:
                continue
            # getDeleted only covers the last 30 days.
            window_start = max(window_start, datetime.now(UTC) - timedelta(days=30))
            async for deleted in self._iter_deleted(client, config.name, window_start, window_end):
                await ctx.emit_deleted(f"{config.name}:{deleted.id}")
            checkpoint = replace(
                checkpoint,
                deleted_through=window_end.isoformat(),
            )
            await ctx.save_checkpoint(checkpoint.to_json())

    async def _iter_deleted(
        self,
        client: SalesforceClient,
        object_type: str,
        start: datetime,
        end: datetime,
    ) -> AsyncIterator[DeletedRecord]:
        result = await client.get_deleted(object_type, start, end)
        while True:
            for deleted in result.deleted_records:
                yield deleted
            if result.next_records_url is None:
                return
            result = await client.get_deleted_more(result.next_records_url)

    async def _realtime_sync(
        self,
        client: SalesforceClient,
        config: SalesforceSourceConfig,
        checkpoint: SalesforceCheckpoint,
        ctx: SyncContext,
    ) -> None:
        """Long-lived polling sync. The connector-manager supervises this slot
        and restarts it if it dies; it returns only when cancelled."""
        configs = enabled_object_configs(config.enabled_objects)
        poll_seconds = max(config.realtime_poll_seconds, 10)

        directory: SalesforceDirectory | None = None
        share_grants: dict[str, RecordGrants] = {}
        previous: PeopleSnapshot | None = None
        last_people_refresh: datetime | None = None
        people_refresh_interval = timedelta(seconds=max(poll_seconds * 10, 300))

        if not checkpoint.watermarks:
            # No baseline yet: run a full pass so polling has watermarks to
            # work from.
            logger.info("Realtime sync: no watermarks, running baseline full sync")
            directory, previous = await self._sync_people(client, config, ctx, None)
            share_grants = await self._sync_shares(client, configs, directory, ctx)
            checkpoint = await self._sync_records(
                client=client,
                configs=configs,
                directory=directory,
                share_grants=share_grants,
                source_config=config,
                checkpoint=checkpoint,
                ctx=ctx,
                incremental=False,
                pass_started_at=datetime.now(UTC),
            )
            last_people_refresh = datetime.now(UTC)

        while True:
            if ctx.is_cancelled():
                return
            now = datetime.now(UTC)

            if directory is None or (
                last_people_refresh is not None
                and now - last_people_refresh >= people_refresh_interval
            ):
                directory, previous = await self._sync_people(client, config, ctx, previous)
                share_grants = await self._sync_shares(client, configs, directory, ctx)
                last_people_refresh = now
                if ctx.is_cancelled():
                    return

            for obj_config in configs:
                watermark = checkpoint.watermarks.get(obj_config.name)
                if watermark is None:
                    continue
                try:
                    since = datetime.fromisoformat(watermark) - timedelta(
                        seconds=DELTA_OVERLAP_SECONDS
                    )
                except ValueError:
                    continue
                await self._poll_object_changes(
                    client=client,
                    config=obj_config,
                    directory=directory,
                    share_grants=share_grants,
                    source_config=config,
                    checkpoint=checkpoint,
                    ctx=ctx,
                    since=since,
                    until=now,
                )

            checkpoint = replace(
                checkpoint,
                deleted_through=now.isoformat(),
                watermarks={
                    **checkpoint.watermarks,
                    **{c.name: now.isoformat() for c in configs},
                },
            )
            await ctx.save_checkpoint(checkpoint.to_json())
            await ctx.increment_scanned()

            await asyncio.sleep(poll_seconds)

    async def _poll_object_changes(
        self,
        *,
        client: SalesforceClient,
        config: SalesforceObjectConfig,
        directory: SalesforceDirectory,
        share_grants: dict[str, RecordGrants],
        source_config: SalesforceSourceConfig,
        checkpoint: SalesforceCheckpoint,
        ctx: SyncContext,
        since: datetime,
        until: datetime,
    ) -> None:
        if ctx.is_cancelled():
            return
        try:
            updated = await client.get_updated(config.name, since, until)
            parser = _RECORD_PARSERS[config.name]
            fields = config.all_fields()
            ids = list(updated.ids)
            for offset in range(0, len(ids), 200):
                batch = ids[offset : offset + 200]
                in_clause = ", ".join(f"'{i}'" for i in batch)
                page = await client.query(
                    f"SELECT {', '.join(fields)} FROM {config.name} WHERE Id IN ({in_clause})"
                )
                for raw in page.records:
                    record = parser(raw)
                    await self._emit_record(
                        client=client,
                        config=config,
                        record=record,
                        directory=directory,
                        share_grants=share_grants,
                        source_config=source_config,
                        ctx=ctx,
                        emit_updated=True,
                    )
        except SalesforceClientError as e:
            logger.warning("Realtime poll failed for %s: %s", config.name, e)
            await ctx.emit_error(f"{config.name}:*", f"Realtime poll failed: {e}")

        try:
            async for deleted in self._iter_deleted(client, config.name, since, until):
                await ctx.emit_deleted(f"{config.name}:{deleted.id}")
        except SalesforceClientError as e:
            logger.warning("Realtime delete poll failed for %s: %s", config.name, e)
            await ctx.emit_error(f"{config.name}:*", f"Realtime delete poll failed: {e}")

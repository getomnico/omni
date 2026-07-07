"""Main ClickUpConnector class."""

import logging
from dataclasses import dataclass
from typing import Mapping

from omni_connector import (
    ActionDefinition,
    ActionResponse,
    Connector,
    HttpMcpServer,
    OAuthCredentialReadyRequest,
    OAuthManifestConfig,
    OAuthScopeSet,
    SearchOperator,
    SyncContext,
    SyncMode,
)
from .client import AuthenticationError, ClickUpClient, ClickUpError
from .config import (
    CHECKPOINT_INTERVAL,
    TASKS_PER_PAGE,
    CLICKUP_MCP_URL,
    CLICKUP_OAUTH_AUTH_ENDPOINT,
    CLICKUP_OAUTH_REGISTRATION_ENDPOINT,
    CLICKUP_OAUTH_RESOURCE,
    CLICKUP_OAUTH_SCOPES,
    CLICKUP_OAUTH_TOKEN_ENDPOINT,
    CLICKUP_OAUTH_USERINFO_ENDPOINT,
)
from .mappers import (
    HierarchyLookup,
    generate_doc_content,
    generate_task_content,
    map_doc_to_document,
    map_task_to_document,
)
from .models import (
    ROLE_GUEST,
    ClickUpSourceConfig,
    ClickUpSpace,
    ClickUpSyncCheckpoint,
    WorkspaceProgress,
    WorkspaceSyncPhase,
    WorkspaceSyncState,
    parse_member,
    parse_space,
)

logger = logging.getLogger(__name__)


@dataclass(frozen=True)
class WorkspaceTaskSyncResult:
    checkpoint: ClickUpSyncCheckpoint
    latest_task_updated_ts: int
    emitted_since_checkpoint: int


@dataclass(frozen=True)
class WorkspaceDocSyncResult:
    checkpoint: ClickUpSyncCheckpoint
    latest_doc_updated_ts: int
    emitted_since_checkpoint: int


def _timestamp_ms(value: object) -> int:
    if isinstance(value, bool):
        return 0
    if isinstance(value, int):
        return max(value, 0)
    if isinstance(value, str):
        try:
            return max(int(value), 0)
        except ValueError:
            return 0
    return 0


class ClickUpConnector(Connector):
    """ClickUp connector for Omni."""

    @property
    def name(self) -> str:
        return "clickup"

    @property
    def display_name(self) -> str:
        return "ClickUp"

    @property
    def version(self) -> str:
        return "1.0.0"

    @property
    def source_types(self) -> list[str]:
        return ["clickup"]

    @property
    def description(self) -> str:
        return "Connect to ClickUp tasks and docs"

    @property
    def sync_modes(self) -> list[str]:
        return ["full", "incremental"]

    @property
    def mcp_server(self) -> HttpMcpServer:
        return HttpMcpServer(url=CLICKUP_MCP_URL)

    @property
    def actions(self) -> list[ActionDefinition]:
        return [
            ActionDefinition(
                name="search_spaces",
                description="Search ClickUp spaces available to this source",
                input_schema={
                    "type": "object",
                    "properties": {
                        "query": {
                            "type": "string",
                            "description": "Optional search query to filter spaces by name or ID",
                        }
                    },
                },
                mode="read",
                source_types=["clickup"],
                admin_only=True,
            )
        ]

    @property
    def search_operators(self) -> list[SearchOperator]:
        return [
            SearchOperator(
                operator="status", attribute_key="status", value_type="text"
            ),
            SearchOperator(
                operator="priority", attribute_key="priority", value_type="text"
            ),
            SearchOperator(
                operator="assignee", attribute_key="assignee", value_type="person"
            ),
            SearchOperator(operator="tag", attribute_key="tags", value_type="text"),
            SearchOperator(
                operator="space", attribute_key="space_name", value_type="text"
            ),
            SearchOperator(
                operator="list", attribute_key="list_name", value_type="text"
            ),
        ]

    def oauth_config(self) -> OAuthManifestConfig | None:
        return OAuthManifestConfig(
            provider="clickup",
            auth_endpoint=CLICKUP_OAUTH_AUTH_ENDPOINT,
            token_endpoint=CLICKUP_OAUTH_TOKEN_ENDPOINT,
            userinfo_endpoint=CLICKUP_OAUTH_USERINFO_ENDPOINT,
            userinfo_email_field="user.email",
            identity_scopes=[],
            scopes={
                "clickup": OAuthScopeSet(
                    read=[CLICKUP_OAUTH_SCOPES[0]],
                    write=CLICKUP_OAUTH_SCOPES,
                )
            },
            extra_auth_params={"resource": CLICKUP_OAUTH_RESOURCE},
            scope_separator=" ",
            registration_endpoint=CLICKUP_OAUTH_REGISTRATION_ENDPOINT,
            token_endpoint_auth_method="none",
            resource=CLICKUP_OAUTH_RESOURCE,
        )

    def _extract_credentials_blob(
        self, credentials: Mapping[str, object]
    ) -> Mapping[str, object]:
        raw = credentials.get("credentials", credentials)
        if isinstance(raw, Mapping):
            return raw
        return {}

    def _mcp_access_token(self, credentials: Mapping[str, object]) -> str | None:
        raw = self._extract_credentials_blob(credentials)
        token = raw.get("access_token")
        if isinstance(token, str) and token:
            return token
        return None

    def prepare_mcp_headers(self, credentials: Mapping[str, object]) -> dict[str, str]:
        token = self._mcp_access_token(credentials)
        if not token:
            raise ValueError("Missing ClickUp OAuth access_token for MCP")
        return {"Authorization": f"Bearer {token}"}

    def _rest_api_token(self, credentials: Mapping[str, object]) -> str | None:
        raw = self._extract_credentials_blob(credentials)
        token = raw.get("token") or raw.get("access_token")
        if isinstance(token, str) and token:
            return token
        return None

    async def bootstrap_mcp(self, credentials: Mapping[str, object]) -> None:
        if not self._mcp_access_token(credentials):
            logger.warning(
                "Skipping ClickUp MCP bootstrap: no OAuth access_token present in credential payload"
            )
            return
        logger.info(
            "Bootstrapping ClickUp MCP catalog with authenticated OAuth credentials"
        )
        await super().bootstrap_mcp(dict(credentials))

    async def oauth_credential_ready(
        self,
        request: OAuthCredentialReadyRequest,
    ) -> bool:
        if not self._mcp_access_token(dict(request.credentials)):
            logger.debug(
                "ClickUp oauth_credential_ready: no OAuth access_token present"
            )
            return False
        logger.info(
            "ClickUp OAuth credential ready: refreshing MCP catalog for source %s",
            request.source_id,
        )
        await self.bootstrap_mcp(dict(request.credentials))
        return True

    async def execute_action(
        self,
        action: str,
        params: Mapping[str, object],
        credentials: Mapping[str, object],
    ):
        if action == "search_spaces":
            return await self._search_spaces(params, credentials)
        return await super().execute_action(action, dict(params), dict(credentials))

    async def _search_spaces(
        self, params: Mapping[str, object], credentials: Mapping[str, object]
    ):
        token = self._rest_api_token(credentials)
        if not token:
            return ActionResponse.failure(
                "Missing ClickUp token for searching spaces"
            ).to_response(status_code=400)

        query = str(params.get("query") or "").strip().lower()
        api_url = (
            params.get("api_url") if isinstance(params.get("api_url"), str) else None
        )
        client = ClickUpClient(token=token, base_url=api_url)
        spaces: list[dict[str, str]] = []

        try:
            workspaces = await client.get_workspaces()
            for workspace in workspaces:
                team_id = str(workspace["id"])
                team_name = str(workspace.get("name") or team_id)
                for space in await client.list_spaces(team_id):
                    space_id = str(space.get("id", ""))
                    space_name = str(space.get("name", ""))
                    if (
                        query
                        and query not in space_id.lower()
                        and query not in space_name.lower()
                        and query not in team_name.lower()
                    ):
                        continue
                    spaces.append(
                        {
                            "id": space_id,
                            "name": space_name,
                            "workspace_id": team_id,
                            "workspace_name": team_name,
                        }
                    )
        except AuthenticationError as e:
            return ActionResponse.failure(f"Authentication failed: {e}").to_response(
                status_code=401
            )
        except ClickUpError as e:
            return ActionResponse.failure(
                f"Failed to search ClickUp spaces: {e}"
            ).to_response(status_code=502)
        finally:
            await client.close()

        return ActionResponse.success({"spaces": spaces}).to_response()

    async def sync(
        self,
        source_config: Mapping[str, object],
        credentials: Mapping[str, object],
        checkpoint: Mapping[str, object] | None,
        ctx: SyncContext,
    ) -> None:
        token = credentials.get("token")
        if not isinstance(token, str) or not token:
            await ctx.fail("Missing 'token' in credentials")
            return

        config = ClickUpSourceConfig.from_mapping(source_config)
        sync_checkpoint = ClickUpSyncCheckpoint.from_mapping(checkpoint).for_mode(
            ctx.sync_mode.value,
            is_resume=ctx.is_resume,
        )
        emitted_since_checkpoint = 0
        client = ClickUpClient(token=token, base_url=config.api_url)

        try:
            workspaces = await client.get_workspaces()
        except AuthenticationError as e:
            await ctx.fail(f"Authentication failed: {e}")
            return
        except ClickUpError as e:
            await ctx.fail(f"Connection test failed: {e}")
            return

        if not workspaces:
            await ctx.fail("No workspaces found for the provided token")
            return

        logger.info(
            "Starting ClickUp %s sync across %d workspace(s)",
            ctx.sync_mode.value,
            len(workspaces),
        )
        if config.space_filters:
            logger.info(
                "ClickUp sync limited to %d selected space(s)",
                len(config.space_filters),
            )

        try:
            # Persist a run-scoped checkpoint immediately. This prevents a resumed
            # full sync from falling back to the source's previous completed
            # incremental checkpoint if the connector dies before the first page.
            await ctx.save_checkpoint(sync_checkpoint.to_json())

            for workspace in workspaces:
                if ctx.is_cancelled():
                    await ctx.fail("Cancelled by user")
                    return

                team_id = str(workspace["id"])
                team_name = workspace.get("name", team_id)
                base_state = sync_checkpoint.workspaces.get(
                    team_id, WorkspaceSyncState()
                )

                logger.info("Syncing workspace '%s' (id=%s)", team_name, team_id)

                # Build hierarchy lookup for space/folder/list names.
                hierarchy, spaces = await self._build_hierarchy(client, team_id)

                # Sync group memberships before documents.
                await self._sync_group_memberships(workspace, spaces, ctx)

                task_result = await self._sync_workspace_tasks(
                    client=client,
                    team_id=team_id,
                    hierarchy=hierarchy,
                    allowed_space_ids=config.space_filters,
                    base_state=base_state,
                    checkpoint=sync_checkpoint,
                    ctx=ctx,
                    emitted_since_checkpoint=emitted_since_checkpoint,
                )
                sync_checkpoint = task_result.checkpoint
                emitted_since_checkpoint = task_result.emitted_since_checkpoint
                if ctx.is_cancelled():
                    return

                if config.include_docs:
                    doc_result = await self._sync_workspace_docs(
                        client=client,
                        team_id=team_id,
                        hierarchy=hierarchy,
                        allowed_space_ids=config.space_filters,
                        base_state=base_state,
                        checkpoint=sync_checkpoint,
                        latest_task_updated_ts=task_result.latest_task_updated_ts,
                        ctx=ctx,
                        emitted_since_checkpoint=emitted_since_checkpoint,
                    )
                    sync_checkpoint = doc_result.checkpoint
                    emitted_since_checkpoint = doc_result.emitted_since_checkpoint
                    latest_doc_updated_ts = doc_result.latest_doc_updated_ts
                    if ctx.is_cancelled():
                        return
                else:
                    latest_doc_updated_ts = base_state.last_doc_updated_ts

                completed_state = base_state.completed(
                    latest_task_updated_ts=task_result.latest_task_updated_ts,
                    latest_doc_updated_ts=latest_doc_updated_ts,
                )
                sync_checkpoint = sync_checkpoint.with_workspace(
                    team_id, completed_state
                )
                await ctx.save_checkpoint(sync_checkpoint.to_json())
                emitted_since_checkpoint = 0

            await ctx.complete(checkpoint=sync_checkpoint.to_json())
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

    async def _sync_workspace_tasks(
        self,
        *,
        client: ClickUpClient,
        team_id: str,
        hierarchy: HierarchyLookup,
        allowed_space_ids: set[str],
        base_state: WorkspaceSyncState,
        checkpoint: ClickUpSyncCheckpoint,
        ctx: SyncContext,
        emitted_since_checkpoint: int,
    ) -> WorkspaceTaskSyncResult:
        progress = base_state.in_progress
        if progress is not None and progress.phase in {
            WorkspaceSyncPhase.DOCS,
            WorkspaceSyncPhase.COMPLETE,
        }:
            return WorkspaceTaskSyncResult(
                checkpoint=checkpoint,
                latest_task_updated_ts=progress.latest_task_updated_ts,
                emitted_since_checkpoint=emitted_since_checkpoint,
            )

        resume_tasks = (
            progress is not None and progress.phase == WorkspaceSyncPhase.TASKS
        )
        page = progress.task_page if resume_tasks else 0
        start_offset = progress.task_offset if resume_tasks else 0
        latest_task_updated_ts = (
            progress.latest_task_updated_ts
            if resume_tasks
            else base_state.last_task_updated_ts
        )
        date_updated_gt = (
            base_state.last_task_updated_ts
            if ctx.sync_mode == SyncMode.INCREMENTAL
            else None
        )

        while True:
            tasks = await client.list_tasks_page(
                team_id, page, date_updated_gt=date_updated_gt
            )
            if not tasks:
                break

            offset = (
                start_offset
                if page == (progress.task_page if resume_tasks else 0)
                else 0
            )
            for index in range(offset, len(tasks)):
                if ctx.is_cancelled():
                    await ctx.fail("Cancelled by user")
                    return WorkspaceTaskSyncResult(
                        checkpoint=checkpoint,
                        latest_task_updated_ts=latest_task_updated_ts,
                        emitted_since_checkpoint=emitted_since_checkpoint,
                    )

                task = tasks[index]
                if allowed_space_ids and not self._task_in_allowed_space(
                    task, hierarchy, allowed_space_ids
                ):
                    continue

                await ctx.increment_scanned()
                try:
                    comments = await client.get_task_comments(task["id"])
                    content = generate_task_content(task, comments, hierarchy)
                    content_id = await ctx.content_storage.save(content, "text/plain")
                    doc = map_task_to_document(
                        task, comments, content_id, team_id, hierarchy
                    )
                    await ctx.emit(doc)
                    emitted_since_checkpoint += 1

                    latest_task_updated_ts = max(
                        latest_task_updated_ts,
                        _timestamp_ms(task.get("date_updated")),
                    )
                    if emitted_since_checkpoint >= CHECKPOINT_INTERVAL:
                        checkpoint = await self._save_workspace_progress(
                            ctx=ctx,
                            checkpoint=checkpoint,
                            team_id=team_id,
                            base_state=base_state,
                            progress=WorkspaceProgress(
                                phase=WorkspaceSyncPhase.TASKS,
                                task_page=page,
                                task_offset=index + 1,
                                latest_task_updated_ts=latest_task_updated_ts,
                                latest_doc_updated_ts=base_state.last_doc_updated_ts,
                            ),
                        )
                        emitted_since_checkpoint = 0
                except Exception as e:
                    eid = f"clickup:task:{task.get('id', '?')}"
                    logger.warning("Error processing %s: %s", eid, e)
                    await ctx.emit_error(eid, str(e))

            next_page = page + 1
            checkpoint = await self._save_workspace_progress(
                ctx=ctx,
                checkpoint=checkpoint,
                team_id=team_id,
                base_state=base_state,
                progress=WorkspaceProgress(
                    phase=WorkspaceSyncPhase.TASKS,
                    task_page=next_page,
                    task_offset=0,
                    latest_task_updated_ts=latest_task_updated_ts,
                    latest_doc_updated_ts=base_state.last_doc_updated_ts,
                ),
            )
            emitted_since_checkpoint = 0
            if len(tasks) < TASKS_PER_PAGE:
                break
            page = next_page
            start_offset = 0

        return WorkspaceTaskSyncResult(
            checkpoint=checkpoint,
            latest_task_updated_ts=latest_task_updated_ts,
            emitted_since_checkpoint=emitted_since_checkpoint,
        )

    async def _sync_workspace_docs(
        self,
        *,
        client: ClickUpClient,
        team_id: str,
        hierarchy: HierarchyLookup,
        allowed_space_ids: set[str],
        base_state: WorkspaceSyncState,
        checkpoint: ClickUpSyncCheckpoint,
        latest_task_updated_ts: int,
        ctx: SyncContext,
        emitted_since_checkpoint: int,
    ) -> WorkspaceDocSyncResult:
        progress = base_state.in_progress
        resume_docs = progress is not None and progress.phase == WorkspaceSyncPhase.DOCS
        cursor = progress.doc_cursor if resume_docs else None
        start_offset = progress.doc_offset if resume_docs else 0
        latest_doc_updated_ts = (
            progress.latest_doc_updated_ts
            if resume_docs
            else base_state.last_doc_updated_ts
        )
        date_updated_gt = (
            base_state.last_doc_updated_ts
            if ctx.sync_mode == SyncMode.INCREMENTAL
            else None
        )

        while True:
            docs, next_cursor = await client.list_docs_page(team_id, cursor)
            if not docs:
                break

            offset = (
                start_offset
                if cursor == (progress.doc_cursor if resume_docs else None)
                else 0
            )
            for index in range(offset, len(docs)):
                if ctx.is_cancelled():
                    await ctx.fail("Cancelled by user")
                    return WorkspaceDocSyncResult(
                        checkpoint=checkpoint,
                        latest_doc_updated_ts=latest_doc_updated_ts,
                        emitted_since_checkpoint=emitted_since_checkpoint,
                    )

                clickup_doc = docs[index]
                if allowed_space_ids and not self._doc_in_allowed_space(
                    clickup_doc, hierarchy, allowed_space_ids
                ):
                    continue

                doc_updated_ts = _timestamp_ms(clickup_doc.get("date_updated"))
                if date_updated_gt is not None and doc_updated_ts <= date_updated_gt:
                    continue

                await ctx.increment_scanned()
                try:
                    pages = await client.get_doc_pages(team_id, clickup_doc["id"])
                    content = generate_doc_content(clickup_doc, pages)
                    content_id = await ctx.content_storage.save(content, "text/plain")
                    doc = map_doc_to_document(clickup_doc, content, content_id, team_id)
                    await ctx.emit(doc)
                    emitted_since_checkpoint += 1
                    latest_doc_updated_ts = max(latest_doc_updated_ts, doc_updated_ts)

                    if emitted_since_checkpoint >= CHECKPOINT_INTERVAL:
                        checkpoint = await self._save_workspace_progress(
                            ctx=ctx,
                            checkpoint=checkpoint,
                            team_id=team_id,
                            base_state=base_state,
                            progress=WorkspaceProgress(
                                phase=WorkspaceSyncPhase.DOCS,
                                task_page=0,
                                task_offset=0,
                                doc_cursor=cursor,
                                doc_offset=index + 1,
                                latest_task_updated_ts=latest_task_updated_ts,
                                latest_doc_updated_ts=latest_doc_updated_ts,
                            ),
                        )
                        emitted_since_checkpoint = 0
                except Exception as e:
                    eid = f"clickup:doc:{clickup_doc.get('id', '?')}"
                    logger.warning("Error processing %s: %s", eid, e)
                    await ctx.emit_error(eid, str(e))

            checkpoint = await self._save_workspace_progress(
                ctx=ctx,
                checkpoint=checkpoint,
                team_id=team_id,
                base_state=base_state,
                progress=WorkspaceProgress(
                    phase=WorkspaceSyncPhase.DOCS,
                    doc_cursor=next_cursor,
                    doc_offset=0,
                    latest_task_updated_ts=latest_task_updated_ts,
                    latest_doc_updated_ts=latest_doc_updated_ts,
                ),
            )
            emitted_since_checkpoint = 0
            if not next_cursor:
                break
            cursor = next_cursor
            start_offset = 0

        return WorkspaceDocSyncResult(
            checkpoint=checkpoint,
            latest_doc_updated_ts=latest_doc_updated_ts,
            emitted_since_checkpoint=emitted_since_checkpoint,
        )

    async def _save_workspace_progress(
        self,
        *,
        ctx: SyncContext,
        checkpoint: ClickUpSyncCheckpoint,
        team_id: str,
        base_state: WorkspaceSyncState,
        progress: WorkspaceProgress,
    ) -> ClickUpSyncCheckpoint:
        checkpoint = checkpoint.with_workspace(
            team_id, base_state.with_progress(progress)
        )
        await ctx.save_checkpoint(checkpoint.to_json())
        return checkpoint

    def _task_in_allowed_space(
        self,
        task: Mapping[str, object],
        hierarchy: HierarchyLookup,
        allowed_space_ids: set[str],
    ) -> bool:
        task_list = task.get("list") or {}
        list_id = str(task_list.get("id", ""))
        space_id = hierarchy.get(list_id).get("space_id", "")
        return bool(space_id and space_id in allowed_space_ids)

    def _doc_in_allowed_space(
        self,
        clickup_doc: Mapping[str, object],
        hierarchy: HierarchyLookup,
        allowed_space_ids: set[str],
    ) -> bool:
        parent = clickup_doc.get("parent")
        if not isinstance(parent, dict):
            return False

        parent_id = str(parent.get("id", ""))
        parent_type = str(parent.get("type", ""))
        if not parent_id:
            return False

        # ClickUp Docs parent types: 4=Space, 5=Folder, 6=List, 12=Workspace.
        if parent_type == "4":
            space_id = parent_id
        elif parent_type == "5":
            space_id = hierarchy.get_folder_space_id(parent_id)
        elif parent_type == "6":
            space_id = hierarchy.get(parent_id).get("space_id", "")
        else:
            space_id = ""

        return bool(space_id and space_id in allowed_space_ids)

    async def _build_hierarchy(
        self, client: ClickUpClient, team_id: str
    ) -> tuple[HierarchyLookup, list[ClickUpSpace]]:
        """Pre-fetch workspace hierarchy to build a list_id → names lookup."""
        hierarchy = HierarchyLookup()
        parsed_spaces: list[ClickUpSpace] = []
        try:
            raw_spaces = await client.list_spaces(team_id)
            for raw_space in raw_spaces:
                space = parse_space(raw_space)
                parsed_spaces.append(space)
                hierarchy.register_space(space.id, space.private, team_id)

                # Lists inside folders
                folders = await client.list_folders(space.id)
                for folder in folders:
                    folder_id = str(folder["id"])
                    hierarchy.register_folder(folder_id, space.id)
                    folder_name = folder.get("name", "")
                    lists = await client.list_lists_in_folder(folder_id)
                    for lst in lists:
                        hierarchy.register_list(
                            lst["id"],
                            lst.get("name", ""),
                            space.name,
                            folder_name,
                            space_id=space.id,
                        )

                # Folderless lists
                folderless = await client.list_folderless_lists(space.id)
                for lst in folderless:
                    hierarchy.register_list(
                        lst["id"],
                        lst.get("name", ""),
                        space.name,
                        space_id=space.id,
                    )
        except ClickUpError as e:
            logger.warning(
                "Failed to build full hierarchy for workspace %s: %s", team_id, e
            )

        return hierarchy, parsed_spaces

    async def _sync_group_memberships(
        self,
        workspace: Mapping[str, object],
        spaces: list[ClickUpSpace],
        ctx: SyncContext,
    ) -> None:
        """Emit group membership events for workspace and private spaces."""
        team_id = str(workspace["id"])
        team_name = workspace.get("name", team_id)

        # Workspace-level group: all non-guest members
        workspace_emails: list[str] = []
        for raw_member in workspace.get("members", []):
            member = parse_member(raw_member)
            if member.role == ROLE_GUEST:
                continue
            if not member.email:
                logger.warning(
                    "Workspace member %s (id=%s) has no email, skipping",
                    member.username,
                    member.user_id,
                )
                continue
            workspace_emails.append(member.email.lower())

        if workspace_emails:
            await ctx.emit_group_membership(
                group_email=f"clickup:workspace:{team_id}",
                member_emails=workspace_emails,
                group_name=team_name,
            )

        # Private space groups
        for space in spaces:
            if not space.private:
                continue

            space_emails: list[str] = []
            for member in space.members:
                if not member.email:
                    logger.warning(
                        "Space '%s' member %s (id=%s) has no email, skipping",
                        space.name,
                        member.username,
                        member.user_id,
                    )
                    continue
                space_emails.append(member.email.lower())

            if space_emails:
                await ctx.emit_group_membership(
                    group_email=f"clickup:space:{space.id}",
                    member_emails=space_emails,
                    group_name=space.name,
                )

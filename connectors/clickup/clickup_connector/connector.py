"""Main ClickUpConnector class."""

import json
import logging
import os
from pathlib import Path
from typing import Any

from omni_connector import (
    ActionDefinition,
    ActionResponse,
    Connector,
    HttpMcpServer,
    OAuthManifestConfig,
    OAuthScopeSet,
    SearchOperator,
    SyncContext,
)
from omni_connector.mcp_adapter import McpAdapter

from .client import AuthenticationError, ClickUpClient, ClickUpError
from .config import (
    CHECKPOINT_INTERVAL,
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
from .models import ROLE_GUEST, ClickUpSpace, parse_member, parse_space

logger = logging.getLogger(__name__)


class ClickUpConnector(Connector):
    """ClickUp connector for Omni."""

    def __init__(self) -> None:
        super().__init__()
        self._mcp_catalog_loaded = False
        self._mcp_catalog_cache_path = Path(
            os.environ.get(
                "CLICKUP_MCP_CATALOG_CACHE",
                "/tmp/omni-clickup-mcp-catalog.json",
            )
        )

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
    def mcp_adapter(self) -> McpAdapter | None:
        adapter = super().mcp_adapter
        if adapter is not None and not self._mcp_catalog_loaded:
            self._load_mcp_catalog_cache(adapter)
        return adapter

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
            SearchOperator(operator="status", attribute_key="status", value_type="text"),
            SearchOperator(operator="priority", attribute_key="priority", value_type="text"),
            SearchOperator(operator="assignee", attribute_key="assignee", value_type="person"),
            SearchOperator(operator="tag", attribute_key="tags", value_type="text"),
            SearchOperator(operator="space", attribute_key="space_name", value_type="text"),
            SearchOperator(operator="list", attribute_key="list_name", value_type="text"),
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
            client_secret_required=False,
            pkce_required=True,
            resource=CLICKUP_OAUTH_RESOURCE,
        )

    def _extract_credentials_blob(self, credentials: dict[str, Any]) -> dict[str, Any]:
        raw = credentials.get("credentials", credentials)
        if isinstance(raw, dict):
            return raw
        return {}

    def _mcp_access_token(self, credentials: dict[str, Any]) -> str | None:
        raw = self._extract_credentials_blob(credentials)
        token = raw.get("access_token")
        if isinstance(token, str) and token:
            return token
        return None

    def prepare_mcp_headers(self, credentials: dict[str, Any]) -> dict[str, str]:
        token = self._mcp_access_token(credentials)
        if not token:
            raise ValueError("Missing ClickUp OAuth access_token for MCP")
        return {"Authorization": f"Bearer {token}"}

    def _rest_api_token(self, credentials: dict[str, Any]) -> str | None:
        raw = self._extract_credentials_blob(credentials)
        token = raw.get("token") or raw.get("access_token")
        if isinstance(token, str) and token:
            return token
        return None

    async def bootstrap_mcp(self, credentials: dict[str, Any]) -> None:
        adapter = self.mcp_adapter
        if adapter is None:
            return
        if not self._mcp_access_token(credentials):
            logger.debug("Skipping ClickUp MCP bootstrap: no OAuth access_token present")
            return
        auth = self._prepare_mcp_auth(credentials)
        logger.info("Bootstrapping ClickUp MCP catalog with authenticated OAuth credentials")
        await adapter.discover(**auth)
        self._save_mcp_catalog_cache(adapter)

    def _load_mcp_catalog_cache(self, adapter: McpAdapter) -> None:
        self._mcp_catalog_loaded = True
        try:
            if not self._mcp_catalog_cache_path.exists():
                return
            catalog = json.loads(self._mcp_catalog_cache_path.read_text())
            adapter.import_catalog(catalog)
            logger.info(
                "Loaded ClickUp MCP catalog cache from %s",
                self._mcp_catalog_cache_path,
            )
        except Exception:
            logger.warning("Failed to load ClickUp MCP catalog cache", exc_info=True)

    def _save_mcp_catalog_cache(self, adapter: McpAdapter) -> None:
        try:
            self._mcp_catalog_cache_path.parent.mkdir(parents=True, exist_ok=True)
            self._mcp_catalog_cache_path.write_text(json.dumps(adapter.export_catalog()))
        except Exception:
            logger.warning("Failed to save ClickUp MCP catalog cache", exc_info=True)

    async def execute_action(
        self,
        action: str,
        params: dict[str, Any],
        credentials: dict[str, Any],
    ):
        if action == "search_spaces":
            return await self._search_spaces(params, credentials)
        return await super().execute_action(action, params, credentials)

    async def _search_spaces(self, params: dict[str, Any], credentials: dict[str, Any]):
        token = self._rest_api_token(credentials)
        if not token:
            return ActionResponse.failure("Missing ClickUp token for searching spaces").to_response(
                status_code=400
            )

        query = str(params.get("query") or "").strip().lower()
        api_url = params.get("api_url") if isinstance(params.get("api_url"), str) else None
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
            return ActionResponse.failure(f"Failed to search ClickUp spaces: {e}").to_response(
                status_code=502
            )
        finally:
            await client.close()

        return ActionResponse.success({"spaces": spaces}).to_response()

    async def sync(
        self,
        source_config: dict[str, Any],
        credentials: dict[str, Any],
        checkpoint: dict[str, Any] | None,
        ctx: SyncContext,
    ) -> None:
        token = credentials.get("token")
        if not token:
            await ctx.fail("Missing 'token' in credentials")
            return

        include_docs = source_config.get("include_docs", True)
        space_filters = source_config.get("space_filters") or []
        allowed_space_ids = {
            str(space_id)
            for space_id in space_filters
            if isinstance(space_id, str) and space_id.strip()
        }
        client = ClickUpClient(token=token, base_url=source_config.get("api_url"))

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

        logger.info("Starting ClickUp sync across %d workspace(s)", len(workspaces))
        if allowed_space_ids:
            logger.info("ClickUp sync limited to %d selected space(s)", len(allowed_space_ids))

        checkpoint = checkpoint or {}
        workspace_states: dict[str, Any] = checkpoint.get("workspaces", {})
        new_workspace_states: dict[str, Any] = {}
        docs_since_checkpoint = 0

        try:
            for workspace in workspaces:
                if ctx.is_cancelled():
                    await ctx.fail("Cancelled by user")
                    return

                team_id = str(workspace["id"])
                team_name = workspace.get("name", team_id)
                prev_state = workspace_states.get(team_id, {})
                latest_updated_ts: int = 0

                logger.info("Syncing workspace '%s' (id=%s)", team_name, team_id)

                # Build hierarchy lookup for space/folder/list names
                hierarchy, spaces = await self._build_hierarchy(client, team_id)

                # Sync group memberships before documents
                await self._sync_group_memberships(workspace, spaces, ctx)

                # Use previous timestamp for incremental sync (checkpoint-driven)
                date_updated_gt: int | None = prev_state.get("last_updated_ts") or None

                # Sync tasks
                try:
                    async for task in client.list_tasks(team_id, date_updated_gt=date_updated_gt):
                        if ctx.is_cancelled():
                            await ctx.fail("Cancelled by user")
                            return

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
                            docs_since_checkpoint += 1

                            task_updated = int(task.get("date_updated", 0))
                            if task_updated > latest_updated_ts:
                                latest_updated_ts = task_updated
                        except Exception as e:
                            eid = f"clickup:task:{task.get('id', '?')}"
                            logger.warning("Error processing %s: %s", eid, e)
                            await ctx.emit_error(eid, str(e))
                except ClickUpError as e:
                    logger.error("Error fetching tasks for workspace %s: %s", team_id, e)
                    await ctx.emit_error(f"clickup:task:{team_id}:*", str(e))

                # Sync docs (optional)
                if include_docs:
                    try:
                        async for clickup_doc in client.list_docs(team_id):
                            if ctx.is_cancelled():
                                await ctx.fail("Cancelled by user")
                                return

                            if allowed_space_ids and not self._doc_in_allowed_space(
                                clickup_doc, hierarchy, allowed_space_ids
                            ):
                                continue

                            await ctx.increment_scanned()
                            try:
                                pages = await client.get_doc_pages(team_id, clickup_doc["id"])
                                content = generate_doc_content(clickup_doc, pages)
                                content_id = await ctx.content_storage.save(content, "text/plain")
                                doc = map_doc_to_document(clickup_doc, content, content_id, team_id)
                                await ctx.emit(doc)
                                docs_since_checkpoint += 1
                            except Exception as e:
                                eid = f"clickup:doc:{clickup_doc.get('id', '?')}"
                                logger.warning("Error processing %s: %s", eid, e)
                                await ctx.emit_error(eid, str(e))
                    except ClickUpError as e:
                        logger.error("Error fetching docs for workspace %s: %s", team_id, e)
                        await ctx.emit_error(f"clickup:doc:{team_id}:*", str(e))

                new_workspace_states[team_id] = {
                    "last_updated_ts": latest_updated_ts or prev_state.get("last_updated_ts", 0),
                }

                if docs_since_checkpoint >= CHECKPOINT_INTERVAL:
                    await ctx.save_checkpoint({"workspaces": new_workspace_states})
                    docs_since_checkpoint = 0

            await ctx.complete(checkpoint={"workspaces": new_workspace_states})
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

    def _task_in_allowed_space(
        self,
        task: dict[str, Any],
        hierarchy: HierarchyLookup,
        allowed_space_ids: set[str],
    ) -> bool:
        task_list = task.get("list") or {}
        list_id = str(task_list.get("id", ""))
        space_id = hierarchy.get(list_id).get("space_id", "")
        return bool(space_id and space_id in allowed_space_ids)

    def _doc_in_allowed_space(
        self,
        clickup_doc: dict[str, Any],
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
            logger.warning("Failed to build full hierarchy for workspace %s: %s", team_id, e)

        return hierarchy, parsed_spaces

    async def _sync_group_memberships(
        self,
        workspace: dict[str, Any],
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

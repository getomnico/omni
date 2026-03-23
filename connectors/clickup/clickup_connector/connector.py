"""Main ClickUpConnector class."""

import logging
from typing import Any

from omni_connector import Connector, SearchOperator, SyncContext

from .client import AuthenticationError, ClickUpClient, ClickUpError
from .config import CHECKPOINT_INTERVAL
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

    async def sync(
        self,
        source_config: dict[str, Any],
        credentials: dict[str, Any],
        state: dict[str, Any] | None,
        ctx: SyncContext,
    ) -> None:
        token = credentials.get("token")
        if not token:
            await ctx.fail("Missing 'token' in credentials")
            return

        include_docs = source_config.get("include_docs", True)
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

        state = state or {}
        workspace_states: dict[str, Any] = state.get("workspaces", {})
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

                # Use previous timestamp for incremental sync (state-driven)
                date_updated_gt: int | None = prev_state.get("last_updated_ts") or None

                # Sync tasks
                try:
                    async for task in client.list_tasks(
                        team_id, date_updated_gt=date_updated_gt
                    ):
                        if ctx.is_cancelled():
                            await ctx.fail("Cancelled by user")
                            return

                        await ctx.increment_scanned()
                        try:
                            comments = await client.get_task_comments(task["id"])
                            content = generate_task_content(task, comments, hierarchy)
                            content_id = await ctx.content_storage.save(
                                content, "text/plain"
                            )
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
                    logger.error(
                        "Error fetching tasks for workspace %s: %s", team_id, e
                    )
                    await ctx.emit_error(f"clickup:task:{team_id}:*", str(e))

                # Sync docs (optional)
                if include_docs:
                    try:
                        async for clickup_doc in client.list_docs(team_id):
                            if ctx.is_cancelled():
                                await ctx.fail("Cancelled by user")
                                return

                            await ctx.increment_scanned()
                            try:
                                pages = await client.get_doc_pages(
                                    team_id, clickup_doc["id"]
                                )
                                content = generate_doc_content(clickup_doc, pages)
                                content_id = await ctx.content_storage.save(
                                    content, "text/plain"
                                )
                                doc = map_doc_to_document(
                                    clickup_doc, content, content_id, team_id
                                )
                                await ctx.emit(doc)
                                docs_since_checkpoint += 1
                            except Exception as e:
                                eid = f"clickup:doc:{clickup_doc.get('id', '?')}"
                                logger.warning("Error processing %s: %s", eid, e)
                                await ctx.emit_error(eid, str(e))
                    except ClickUpError as e:
                        logger.error(
                            "Error fetching docs for workspace %s: %s", team_id, e
                        )
                        await ctx.emit_error(f"clickup:doc:{team_id}:*", str(e))

                new_workspace_states[team_id] = {
                    "last_updated_ts": latest_updated_ts
                    or prev_state.get("last_updated_ts", 0),
                }

                if docs_since_checkpoint >= CHECKPOINT_INTERVAL:
                    await ctx.save_state({"workspaces": new_workspace_states})
                    docs_since_checkpoint = 0

            await ctx.complete(new_state={"workspaces": new_workspace_states})
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
                    folder_name = folder.get("name", "")
                    lists = await client.list_lists_in_folder(folder["id"])
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

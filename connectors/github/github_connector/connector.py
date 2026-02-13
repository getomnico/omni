"""Main GitHubConnector class."""

import logging
from typing import Any

from omni_connector import Connector, SyncContext

from .client import AuthenticationError, GitHubClient, GitHubError
from .config import CHECKPOINT_INTERVAL
from .mappers import (
    generate_discussion_content,
    generate_issue_content,
    generate_pr_content,
    generate_repo_content,
    map_discussion_to_document,
    map_issue_to_document,
    map_pr_to_document,
    map_repo_to_document,
)

logger = logging.getLogger(__name__)


class GitHubConnector(Connector):
    """GitHub connector for Omni."""

    @property
    def name(self) -> str:
        return "github"

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
        token = credentials.get("token")
        if not token:
            await ctx.fail("Missing 'token' in credentials")
            return

        api_url = source_config.get("api_url")
        include_discussions = source_config.get("include_discussions", True)
        include_forks = source_config.get("include_forks", False)

        client = GitHubClient(token=token, base_url=api_url)

        try:
            username = await client.validate_token()
        except AuthenticationError as e:
            await ctx.fail(f"Authentication failed: {e}")
            return
        except GitHubError as e:
            await ctx.fail(f"Connection test failed: {e}")
            return

        logger.info("Starting GitHub sync as user '%s'", username)

        state = state or {}
        repo_states: dict[str, Any] = state.get("repos", {})
        new_repo_states: dict[str, Any] = {}
        docs_since_checkpoint = 0

        try:
            repos = await self._resolve_repos(
                client, source_config, username, include_forks
            )

            for repo in repos:
                if ctx.is_cancelled():
                    await ctx.fail("Cancelled by user")
                    return

                full_name = repo.full_name
                is_private = repo.private
                prev = repo_states.get(full_name, {})
                owner, name = full_name.split("/", 1)

                new_state_entry: dict[str, str] = {}

                # Sync repo document
                docs_since_checkpoint = await self._sync_repo(
                    client,
                    repo,
                    owner,
                    name,
                    ctx,
                    docs_since_checkpoint,
                    new_repo_states,
                )

                # Sync issues
                since_issues = prev.get("issues_updated_at")
                latest_issue_ts = since_issues
                try:
                    async for issue in client.list_issues(
                        owner, name, since=since_issues
                    ):
                        if ctx.is_cancelled():
                            await ctx.fail("Cancelled by user")
                            return
                        await ctx.increment_scanned()
                        try:
                            comments = await client.list_issue_comments(
                                owner, name, issue.number
                            )
                            content = generate_issue_content(issue, comments)
                            content_id = await ctx.content_storage.save(
                                content, "text/plain"
                            )
                            doc = map_issue_to_document(
                                issue, comments, content_id, full_name, is_private
                            )
                            await ctx.emit(doc)
                            docs_since_checkpoint += 1
                            ts = str(issue.updated_at) if issue.updated_at else None
                            if ts and (not latest_issue_ts or ts > latest_issue_ts):
                                latest_issue_ts = ts
                        except Exception as e:
                            eid = f"github:issue:{full_name}#{issue.number}"
                            logger.warning("Error processing %s: %s", eid, e)
                            await ctx.emit_error(eid, str(e))
                except GitHubError as e:
                    logger.error("Error fetching issues for %s: %s", full_name, e)
                    await ctx.emit_error(f"github:issue:{full_name}:*", str(e))

                if latest_issue_ts:
                    new_state_entry["issues_updated_at"] = latest_issue_ts

                # Sync pull requests
                since_prs = prev.get("prs_updated_at")
                latest_pr_ts = since_prs
                try:
                    async for pr in client.list_pull_requests(
                        owner, name, since=since_prs
                    ):
                        if ctx.is_cancelled():
                            await ctx.fail("Cancelled by user")
                            return
                        await ctx.increment_scanned()
                        try:
                            issue_comments = await client.list_pr_issue_comments(
                                owner, name, pr.number
                            )
                            review_comments = await client.list_pr_review_comments(
                                owner, name, pr.number
                            )
                            content = generate_pr_content(
                                pr, issue_comments, review_comments
                            )
                            content_id = await ctx.content_storage.save(
                                content, "text/plain"
                            )
                            doc = map_pr_to_document(
                                pr,
                                issue_comments,
                                review_comments,
                                content_id,
                                full_name,
                                is_private,
                            )
                            await ctx.emit(doc)
                            docs_since_checkpoint += 1
                            ts = str(pr.updated_at) if pr.updated_at else None
                            if ts and (not latest_pr_ts or ts > latest_pr_ts):
                                latest_pr_ts = ts
                        except Exception as e:
                            eid = f"github:pr:{full_name}#{pr.number}"
                            logger.warning("Error processing %s: %s", eid, e)
                            await ctx.emit_error(eid, str(e))
                except GitHubError as e:
                    logger.error("Error fetching PRs for %s: %s", full_name, e)
                    await ctx.emit_error(f"github:pr:{full_name}:*", str(e))

                if latest_pr_ts:
                    new_state_entry["prs_updated_at"] = latest_pr_ts

                # Sync discussions
                if include_discussions:
                    since_disc = prev.get("discussions_updated_at")
                    latest_disc_ts = since_disc
                    try:
                        async for disc in client.list_discussions(
                            owner, name, since=since_disc
                        ):
                            if ctx.is_cancelled():
                                await ctx.fail("Cancelled by user")
                                return
                            await ctx.increment_scanned()
                            try:
                                content = generate_discussion_content(disc)
                                content_id = await ctx.content_storage.save(
                                    content, "text/plain"
                                )
                                doc = map_discussion_to_document(
                                    disc, content_id, full_name, is_private
                                )
                                await ctx.emit(doc)
                                docs_since_checkpoint += 1
                                ts = disc.get("updatedAt")
                                if ts and (not latest_disc_ts or ts > latest_disc_ts):
                                    latest_disc_ts = ts
                            except Exception as e:
                                num = disc.get("number", "?")
                                eid = f"github:discussion:{full_name}#{num}"
                                logger.warning("Error processing %s: %s", eid, e)
                                await ctx.emit_error(eid, str(e))
                    except GitHubError as e:
                        logger.error(
                            "Error fetching discussions for %s: %s", full_name, e
                        )
                        await ctx.emit_error(f"github:discussion:{full_name}:*", str(e))

                    if latest_disc_ts:
                        new_state_entry["discussions_updated_at"] = latest_disc_ts

                if new_state_entry:
                    new_repo_states[full_name] = new_state_entry

                if docs_since_checkpoint >= CHECKPOINT_INTERVAL:
                    await ctx.save_state({"repos": new_repo_states})
                    docs_since_checkpoint = 0

            await ctx.complete(new_state={"repos": new_repo_states})
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

    async def _sync_repo(
        self,
        client: GitHubClient,
        repo: Any,
        owner: str,
        name: str,
        ctx: SyncContext,
        docs_since_checkpoint: int,
        new_repo_states: dict[str, Any],
    ) -> int:
        """Sync a single repository document. Returns updated docs_since_checkpoint."""
        await ctx.increment_scanned()
        try:
            readme = await client.get_readme(owner, name)
            content = generate_repo_content(repo, readme)
            content_id = await ctx.content_storage.save(content, "text/plain")
            doc = map_repo_to_document(repo, readme, content_id)
            await ctx.emit(doc)
            docs_since_checkpoint += 1
        except Exception as e:
            eid = f"github:repo:{repo.full_name}"
            logger.warning("Error processing %s: %s", eid, e)
            await ctx.emit_error(eid, str(e))
        return docs_since_checkpoint

    async def _resolve_repos(
        self,
        client: GitHubClient,
        source_config: dict[str, Any],
        username: str,
        include_forks: bool,
    ) -> list[Any]:
        """Determine which repos to sync based on source_config."""
        repos: list[Any] = []
        seen: set[str] = set()

        explicit_repos = source_config.get("repos", [])
        if explicit_repos:
            for repo_spec in explicit_repos:
                parts = repo_spec.split("/", 1)
                if len(parts) == 2:
                    try:
                        repo = await client.get_repo(parts[0], parts[1])
                        if repo.full_name not in seen:
                            seen.add(repo.full_name)
                            repos.append(repo)
                    except GitHubError as e:
                        logger.warning("Failed to fetch repo %s: %s", repo_spec, e)

        orgs = source_config.get("orgs", [])
        for org in orgs:
            async for repo in client.list_repos_for_org(org):
                if repo.full_name not in seen:
                    seen.add(repo.full_name)
                    repos.append(repo)

        users = source_config.get("users", [])
        for user in users:
            async for repo in client.list_repos_for_user(user):
                if repo.full_name not in seen:
                    seen.add(repo.full_name)
                    repos.append(repo)

        if not explicit_repos and not orgs and not users:
            async for repo in client.list_repos_for_authenticated_user():
                if repo.full_name not in seen:
                    seen.add(repo.full_name)
                    repos.append(repo)

        if not include_forks:
            repos = [r for r in repos if not r.fork]

        logger.info("Resolved %d repositories to sync", len(repos))
        return repos

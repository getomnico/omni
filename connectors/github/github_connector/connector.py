"""Main GitHubConnector class."""

from __future__ import annotations

import logging
from collections.abc import Mapping
from typing import TYPE_CHECKING, Any

from omni_connector import (
    Connector,
    OAuthCredentialReadyRequest,
    OAuthManifestConfig,
    OAuthScopeSet,
    SearchOperator,
    StdioMcpServer,
    SyncContext,
)

from .client import AuthenticationError, GitHubClient, GitHubError, GitHubRepo
from .config import CHECKPOINT_INTERVAL, GITHUB_MCP_COMMAND, MCP_TOOLSETS
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
from .models import GitHubCredentials, GitHubSourceConfig
from .skills import connector_skills

if TYPE_CHECKING:
    from omni_connector import ConnectorSkillDefinition, Source

logger = logging.getLogger(__name__)


class GitHubConnector(Connector):
    """GitHub connector for Omni."""

    @property
    def name(self) -> str:
        return "github"

    @property
    def display_name(self) -> str:
        return "GitHub"

    @property
    def version(self) -> str:
        return "1.0.0"

    @property
    def source_types(self) -> list[str]:
        return ["github"]

    @property
    def description(self) -> str:
        return "Connect to GitHub repositories, issues, PRs, and discussions"

    @property
    def sync_modes(self) -> list[str]:
        return ["full", "incremental"]

    @property
    def search_operators(self) -> list[SearchOperator]:
        return [
            SearchOperator(
                operator="status", attribute_key="status", value_type="text"
            ),
            SearchOperator(operator="label", attribute_key="labels", value_type="text"),
            SearchOperator(
                operator="lang", attribute_key="language", value_type="text"
            ),
            SearchOperator(
                operator="assignee", attribute_key="assignee", value_type="person"
            ),
        ]

    @property
    def mcp_server(self) -> StdioMcpServer:
        return StdioMcpServer(
            command=GITHUB_MCP_COMMAND,
            args=[
                "stdio",
                "--toolsets",
                ",".join(MCP_TOOLSETS),
            ],
        )

    @property
    def skills(self) -> list[ConnectorSkillDefinition]:
        return connector_skills()

    def oauth_config(self) -> OAuthManifestConfig | None:
        return OAuthManifestConfig(
            provider="github",
            auth_endpoint="https://github.com/login/oauth/authorize",
            token_endpoint="https://github.com/login/oauth/access_token",
            userinfo_endpoint="https://api.github.com/user/emails",
            userinfo_email_field="email",
            identity_scopes=["read:user", "user:email"],
            scopes={
                "github": OAuthScopeSet(
                    read=["repo", "read:org"],
                    write=["repo"],
                )
            },
            scope_separator=" ",
        )

    def _mcp_token(self, credentials: Mapping[str, Any]) -> str | None:
        """Extract the GitHub token from an Omni credential payload.

        Accepts the flat shape used by PAT credentials (``{"token": ...}``)
        and the ServiceCredential shape used by per-user OAuth credentials
        (``{"credentials": {"access_token": ...}}``).
        """
        raw = credentials.get("credentials", credentials)
        if not isinstance(raw, Mapping):
            return None
        for key in ("token", "access_token"):
            value = raw.get(key)
            if isinstance(value, str) and value:
                return value
        return None

    def prepare_mcp_env(self, credentials: dict[str, Any]) -> dict[str, str]:
        token = self._mcp_token(credentials)
        if token is None:
            raise ValueError(
                "Missing GitHub token for MCP: credential payload has no "
                "'token' or 'access_token'"
            )
        return {"GITHUB_PERSONAL_ACCESS_TOKEN": token}

    async def bootstrap_mcp(self, credentials: dict[str, Any]) -> None:
        if self._mcp_token(credentials) is None:
            logger.warning(
                "Skipping GitHub MCP bootstrap: no token in credential payload"
            )
            return
        await super().bootstrap_mcp(credentials)

    async def oauth_credential_ready(
        self, request: OAuthCredentialReadyRequest
    ) -> bool:
        if self._mcp_token(dict(request.credentials)) is None:
            logger.debug(
                "GitHub oauth_credential_ready: no token in credential payload"
            )
            return False
        logger.info(
            "GitHub OAuth credential ready: refreshing MCP catalog for source %s",
            request.source_id,
        )
        await self.bootstrap_mcp(dict(request.credentials))
        return True

    def validate_mcp_action(
        self,
        action: str,
        params: dict[str, Any],
        source: Source | None,
    ) -> None:
        """Keep MCP tools inside the repositories configured on the source.

        Sync honors ``repos``/``orgs``/``users`` in the source config; MCP
        tools would otherwise operate on any repository the resolved token can
        reach. When no explicit scope is configured (discovery mode), no
        constraint can be derived and the call is allowed.
        """
        if source is None:
            return
        try:
            config = GitHubSourceConfig(**(source.config or {}))
        except Exception:
            logger.warning(
                "Could not parse GitHub source config for MCP scoping; allowing action %s",
                action,
            )
            return

        repo_scope = {r.lower() for r in config.repos if r}
        repo_owners = {full.split("/", 1)[0].lower() for full in repo_scope}
        org_user_owners = {o.lower() for o in (*config.orgs, *config.users) if o}
        if not repo_scope and not org_user_owners:
            return

        owner = params.get("owner")
        if not (isinstance(owner, str) and owner):
            return
        owner_l = owner.lower()

        if owner_l in org_user_owners:
            # Sync indexes everything under this org/user; any repo is in scope.
            return
        if owner_l in repo_owners:
            repo = params.get("repo")
            if (
                isinstance(repo, str)
                and repo
                and f"{owner_l}/{repo.lower()}" not in repo_scope
            ):
                raise ValueError(
                    f"Repository '{owner}/{repo}' is outside the sources "
                    "configured scope"
                )
            return
        raise ValueError(
            f"Repository owner '{owner}' is outside the sources configured scope"
        )
    async def sync(
        self,
        source_config: dict[str, Any],
        credentials: dict[str, Any],
        checkpoint: dict[str, Any] | None,
        ctx: SyncContext,
    ) -> None:
        try:
            creds = GitHubCredentials(**credentials)
        except Exception:
            await ctx.fail("Missing 'token' or 'access_token' in credentials")
            return

        config = GitHubSourceConfig(**source_config)
        client = GitHubClient(token=creds.effective_token, base_url=config.api_url)

        try:
            username = await client.validate_token()
        except AuthenticationError as e:
            await ctx.fail(f"Authentication failed: {e}")
            return
        except GitHubError as e:
            await ctx.fail(f"Connection test failed: {e}")
            return

        logger.info("Starting GitHub sync as user '%s'", username)

        checkpoint = checkpoint or {}
        repo_checkpoints: dict[str, Any] = checkpoint.get("repos", {})
        new_repo_checkpoints: dict[str, Any] = {}
        docs_since_checkpoint = 0

        try:
            repos = await self._resolve_repos(client, config, username)

            await self._sync_permissions(client, repos, ctx)

            for repo in repos:
                if ctx.is_cancelled():
                    await ctx.fail("Cancelled by user")
                    return

                full_name = repo.full_name
                is_private = repo.private
                prev = repo_checkpoints.get(full_name, {})
                owner, name = full_name.split("/", 1)

                new_checkpoint_entry: dict[str, str] = {}

                # Sync repo document
                docs_since_checkpoint = await self._sync_repo(
                    client,
                    repo,
                    owner,
                    name,
                    ctx,
                    docs_since_checkpoint,
                    new_repo_checkpoints,
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
                    new_checkpoint_entry["issues_updated_at"] = latest_issue_ts

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
                    new_checkpoint_entry["prs_updated_at"] = latest_pr_ts

                # Sync discussions
                if config.include_discussions:
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
                        new_checkpoint_entry["discussions_updated_at"] = latest_disc_ts

                if new_checkpoint_entry:
                    new_repo_checkpoints[full_name] = new_checkpoint_entry

                if docs_since_checkpoint >= CHECKPOINT_INTERVAL:
                    await ctx.save_checkpoint({"repos": new_repo_checkpoints})
                    docs_since_checkpoint = 0

            await ctx.complete(checkpoint={"repos": new_repo_checkpoints})
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
        repo: GitHubRepo,
        owner: str,
        name: str,
        ctx: SyncContext,
        docs_since_checkpoint: int,
        new_repo_checkpoints: dict[str, Any],
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

    async def _sync_permissions(
        self,
        client: GitHubClient,
        repos: list[GitHubRepo],
        ctx: SyncContext,
    ) -> None:
        """Emit group membership events for private repos based on collaborators."""
        repo_collaborators: dict[str, list[str]] = {}
        all_logins: set[str] = set()

        for repo in repos:
            if not repo.private:
                continue
            owner, name = repo.full_name.split("/", 1)
            logins: list[str] = []
            try:
                async for collab in client.list_collaborators(owner, name):
                    logins.append(collab.login)
                    all_logins.add(collab.login)
            except GitHubError as e:
                logger.warning(
                    "Failed to fetch collaborators for %s: %s", repo.full_name, e
                )
                continue
            repo_collaborators[repo.full_name] = logins

        # Resolve login → email (deduplicated)
        login_to_email: dict[str, str] = {}
        for login in all_logins:
            email = await client.get_user_email(login)
            if email:
                login_to_email[login] = email.lower()
            else:
                logger.warning(
                    "No public email for GitHub user '%s', skipping from permissions",
                    login,
                )

        for repo_full_name, logins in repo_collaborators.items():
            emails = [
                login_to_email[login] for login in logins if login in login_to_email
            ]
            if emails:
                await ctx.emit_group_membership(
                    group_email=f"github:repo:{repo_full_name}",
                    member_emails=emails,
                    group_name=repo_full_name,
                )
            else:
                logger.warning(
                    "No resolvable emails for any collaborator of %s", repo_full_name
                )

    async def _resolve_repos(
        self,
        client: GitHubClient,
        config: GitHubSourceConfig,
        username: str,
    ) -> list[GitHubRepo]:
        """Determine which repos to sync based on config."""
        repos: list[GitHubRepo] = []
        seen: set[str] = set()

        if config.repos:
            for repo_spec in config.repos:
                parts = repo_spec.split("/", 1)
                if len(parts) == 2:
                    try:
                        repo = await client.get_repo(parts[0], parts[1])
                        if repo.full_name not in seen:
                            seen.add(repo.full_name)
                            repos.append(repo)
                    except GitHubError as e:
                        logger.warning("Failed to fetch repo %s: %s", repo_spec, e)

        for org in config.orgs:
            async for org_repo in client.list_repos_for_org(org):
                if org_repo.full_name not in seen:
                    seen.add(org_repo.full_name)
                    repos.append(org_repo)

        for user in config.users:
            async for user_repo in client.list_repos_for_user(user):
                if user_repo.full_name not in seen:
                    seen.add(user_repo.full_name)
                    repos.append(user_repo)

        if not config.repos and not config.orgs and not config.users:
            async for auth_repo in client.list_repos_for_authenticated_user():
                if auth_repo.full_name not in seen:
                    seen.add(auth_repo.full_name)
                    repos.append(auth_repo)

        if not config.include_forks:
            repos = [r for r in repos if not r.fork]

        logger.info("Resolved %d repositories to sync", len(repos))
        return repos

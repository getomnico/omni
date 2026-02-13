"""GitHub API client wrapper using githubkit."""

import logging
from collections.abc import AsyncIterator
from typing import Any

from githubkit import GitHub, TokenAuthStrategy
from githubkit.exception import RequestError, RequestFailed

from .config import DISCUSSIONS_QUERY, ITEMS_PER_PAGE, MAX_COMMENT_COUNT

logger = logging.getLogger(__name__)


class GitHubError(Exception):
    """Base exception for GitHub API errors."""

    pass


class AuthenticationError(GitHubError):
    """Invalid or expired token (401)."""

    pass


class GitHubClient:
    """Thin wrapper around githubkit exposing the operations we need."""

    def __init__(self, token: str, base_url: str | None = None):
        kwargs: dict[str, Any] = {"auth": TokenAuthStrategy(token)}
        if base_url:
            kwargs["base_url"] = base_url
        self._github = GitHub(**kwargs)

    async def validate_token(self) -> str:
        """Validate the token by fetching the authenticated user. Returns username."""
        try:
            resp = await self._github.rest.users.async_get_authenticated()
            return resp.parsed_data.login
        except RequestFailed as e:
            if e.response.status_code == 401:
                raise AuthenticationError("Invalid or expired token") from e
            raise GitHubError(f"Token validation failed: {e}") from e
        except RequestError as e:
            raise GitHubError(f"Token validation failed: {e}") from e

    async def list_repos_for_org(self, org: str) -> AsyncIterator[Any]:
        """List all repositories for an organization."""
        try:
            async for repo in self._github.paginate(
                self._github.rest.repos.async_list_for_org,
                org=org,
                per_page=ITEMS_PER_PAGE,
                map_func=lambda r: r.parsed_data,
            ):
                yield repo
        except RequestFailed as e:
            raise GitHubError(f"Failed to list repos for org {org}: {e}") from e

    async def list_repos_for_user(self, username: str) -> AsyncIterator[Any]:
        """List all repositories for a user."""
        try:
            async for repo in self._github.paginate(
                self._github.rest.repos.async_list_for_user,
                username=username,
                per_page=ITEMS_PER_PAGE,
                map_func=lambda r: r.parsed_data,
            ):
                yield repo
        except RequestFailed as e:
            raise GitHubError(f"Failed to list repos for user {username}: {e}") from e

    async def list_repos_for_authenticated_user(self) -> AsyncIterator[Any]:
        """List all repositories accessible to the authenticated user."""
        try:
            async for repo in self._github.paginate(
                self._github.rest.repos.async_list_for_authenticated_user,
                per_page=ITEMS_PER_PAGE,
                map_func=lambda r: r.parsed_data,
            ):
                yield repo
        except RequestFailed as e:
            raise GitHubError(
                f"Failed to list repos for authenticated user: {e}"
            ) from e

    async def get_repo(self, owner: str, repo: str) -> Any:
        """Get a single repository by owner/name."""
        try:
            resp = await self._github.rest.repos.async_get(owner=owner, repo=repo)
            return resp.parsed_data
        except RequestFailed as e:
            raise GitHubError(f"Failed to get repo {owner}/{repo}: {e}") from e

    async def get_readme(self, owner: str, repo: str) -> str | None:
        """Get README content for a repository. Returns None if not found."""
        try:
            resp = await self._github.rest.repos.async_get_readme(
                owner=owner, repo=repo
            )
            content = resp.parsed_data.content
            if content:
                import base64

                return base64.b64decode(content).decode("utf-8", errors="replace")
            return None
        except RequestFailed:
            return None

    async def list_issues(
        self, owner: str, repo: str, since: str | None = None
    ) -> AsyncIterator[Any]:
        """List issues (excluding PRs) for a repository."""
        kwargs: dict[str, Any] = {
            "owner": owner,
            "repo": repo,
            "state": "all",
            "sort": "updated",
            "direction": "desc",
            "per_page": ITEMS_PER_PAGE,
        }
        if since:
            kwargs["since"] = since
        try:
            async for issue in self._github.paginate(
                self._github.rest.issues.async_list_for_repo,
                map_func=lambda r: r.parsed_data,
                **kwargs,
            ):
                if not issue.pull_request:
                    yield issue
        except RequestFailed as e:
            raise GitHubError(f"Failed to list issues for {owner}/{repo}: {e}") from e

    async def list_issue_comments(
        self, owner: str, repo: str, number: int
    ) -> list[Any]:
        """List comments on an issue, capped at MAX_COMMENT_COUNT."""
        comments: list[Any] = []
        try:
            async for comment in self._github.paginate(
                self._github.rest.issues.async_list_comments,
                owner=owner,
                repo=repo,
                issue_number=number,
                per_page=ITEMS_PER_PAGE,
                map_func=lambda r: r.parsed_data,
            ):
                comments.append(comment)
                if len(comments) >= MAX_COMMENT_COUNT:
                    break
        except RequestFailed as e:
            logger.warning("Failed to fetch comments for issue #%d: %s", number, e)
        return comments

    async def list_pull_requests(
        self, owner: str, repo: str, since: str | None = None
    ) -> AsyncIterator[Any]:
        """List pull requests for a repository."""
        kwargs: dict[str, Any] = {
            "owner": owner,
            "repo": repo,
            "state": "all",
            "sort": "updated",
            "direction": "desc",
            "per_page": ITEMS_PER_PAGE,
        }
        try:
            async for pr in self._github.paginate(
                self._github.rest.pulls.async_list,
                map_func=lambda r: r.parsed_data,
                **kwargs,
            ):
                if since and pr.updated_at and str(pr.updated_at) < since:
                    return
                yield pr
        except RequestFailed as e:
            raise GitHubError(f"Failed to list PRs for {owner}/{repo}: {e}") from e

    async def list_pr_review_comments(
        self, owner: str, repo: str, number: int
    ) -> list[Any]:
        """List review comments on a pull request, capped at MAX_COMMENT_COUNT."""
        comments: list[Any] = []
        try:
            async for comment in self._github.paginate(
                self._github.rest.pulls.async_list_review_comments,
                owner=owner,
                repo=repo,
                pull_number=number,
                per_page=ITEMS_PER_PAGE,
                map_func=lambda r: r.parsed_data,
            ):
                comments.append(comment)
                if len(comments) >= MAX_COMMENT_COUNT:
                    break
        except RequestFailed as e:
            logger.warning("Failed to fetch review comments for PR #%d: %s", number, e)
        return comments

    async def list_pr_issue_comments(
        self, owner: str, repo: str, number: int
    ) -> list[Any]:
        """List issue-style comments on a pull request (conversation comments)."""
        comments: list[Any] = []
        try:
            async for comment in self._github.paginate(
                self._github.rest.issues.async_list_comments,
                owner=owner,
                repo=repo,
                issue_number=number,
                per_page=ITEMS_PER_PAGE,
                map_func=lambda r: r.parsed_data,
            ):
                comments.append(comment)
                if len(comments) >= MAX_COMMENT_COUNT:
                    break
        except RequestFailed as e:
            logger.warning("Failed to fetch issue comments for PR #%d: %s", number, e)
        return comments

    async def list_discussions(
        self, owner: str, repo: str, since: str | None = None
    ) -> AsyncIterator[dict[str, Any]]:
        """List discussions for a repository using GraphQL."""
        cursor = None
        while True:
            try:
                result = await self._github.async_graphql(
                    DISCUSSIONS_QUERY,
                    variables={"owner": owner, "name": repo, "cursor": cursor},
                )
            except Exception as e:
                logger.warning(
                    "Failed to fetch discussions for %s/%s: %s", owner, repo, e
                )
                return

            repo_data = result.get("repository")
            if not repo_data:
                return
            discussions_data = repo_data.get("discussions", {})
            nodes = discussions_data.get("nodes", [])

            for discussion in nodes:
                if since and discussion.get("updatedAt", "") < since:
                    return
                yield discussion

            page_info = discussions_data.get("pageInfo", {})
            if not page_info.get("hasNextPage"):
                return
            cursor = page_info.get("endCursor")

    async def close(self) -> None:
        """No-op: githubkit manages its own httpx client lifecycle internally."""

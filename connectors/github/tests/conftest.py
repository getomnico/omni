"""Integration test fixtures for the GitHub connector.

Session-scoped: harness, mock GitHub API server, connector server, connector-manager.
Function-scoped: seed helper, source_id, httpx client.
"""

from __future__ import annotations

import base64
import logging
import socket
import threading
import time
from typing import Any

import httpx
import pytest
import pytest_asyncio
import uvicorn
from starlette.applications import Starlette
from starlette.requests import Request
from starlette.responses import JSONResponse
from starlette.routing import Route

from omni_connector.testing import OmniTestHarness, SeedHelper

logger = logging.getLogger(__name__)


# ---------------------------------------------------------------------------
# Mock data templates
# ---------------------------------------------------------------------------


def _user_payload(login: str = "testbot", uid: int = 1) -> dict[str, Any]:
    return {
        "login": login,
        "id": uid,
        "node_id": f"MDQ6VXNlcg=={uid}",
        "avatar_url": f"https://avatars.githubusercontent.com/u/{uid}?v=4",
        "gravatar_id": "",
        "url": f"https://api.github.com/users/{login}",
        "html_url": f"https://github.com/{login}",
        "followers_url": f"https://api.github.com/users/{login}/followers",
        "following_url": f"https://api.github.com/users/{login}/following{{/other_user}}",
        "gists_url": f"https://api.github.com/users/{login}/gists{{/gist_id}}",
        "starred_url": f"https://api.github.com/users/{login}/starred{{/owner}}{{/repo}}",
        "subscriptions_url": f"https://api.github.com/users/{login}/subscriptions",
        "organizations_url": f"https://api.github.com/users/{login}/orgs",
        "repos_url": f"https://api.github.com/users/{login}/repos",
        "events_url": f"https://api.github.com/users/{login}/events{{/privacy}}",
        "received_events_url": f"https://api.github.com/users/{login}/received_events",
        "type": "User",
        "site_admin": False,
        "user_view_type": "public",
        "name": login,
        "company": None,
        "blog": "",
        "location": None,
        "email": None,
        "hireable": None,
        "bio": None,
        "twitter_username": None,
        "notification_email": None,
        "public_repos": 10,
        "public_gists": 0,
        "followers": 0,
        "following": 0,
        "created_at": "2020-01-01T00:00:00Z",
        "updated_at": "2024-01-01T00:00:00Z",
    }


def _owner_payload(login: str = "octocat", uid: int = 1) -> dict[str, Any]:
    return {
        "login": login,
        "id": uid,
        "node_id": f"MDQ6VXNlcg=={uid}",
        "avatar_url": f"https://avatars.githubusercontent.com/u/{uid}?v=4",
        "gravatar_id": "",
        "url": f"https://api.github.com/users/{login}",
        "html_url": f"https://github.com/{login}",
        "followers_url": f"https://api.github.com/users/{login}/followers",
        "following_url": f"https://api.github.com/users/{login}/following{{/other_user}}",
        "gists_url": f"https://api.github.com/users/{login}/gists{{/gist_id}}",
        "starred_url": f"https://api.github.com/users/{login}/starred{{/owner}}{{/repo}}",
        "subscriptions_url": f"https://api.github.com/users/{login}/subscriptions",
        "organizations_url": f"https://api.github.com/users/{login}/orgs",
        "repos_url": f"https://api.github.com/users/{login}/repos",
        "events_url": f"https://api.github.com/users/{login}/events{{/privacy}}",
        "received_events_url": f"https://api.github.com/users/{login}/received_events",
        "type": "User",
        "user_view_type": "public",
        "site_admin": False,
    }


def _repo_payload(
    owner: str,
    name: str,
    *,
    repo_id: int = 1,
    private: bool = False,
    description: str = "",
    language: str = "Python",
    topics: list[str] | None = None,
    fork: bool = False,
) -> dict[str, Any]:
    full_name = f"{owner}/{name}"
    base_url = f"https://api.github.com/repos/{full_name}"
    return {
        "id": repo_id,
        "node_id": f"R_kg{repo_id}",
        "name": name,
        "full_name": full_name,
        "private": private,
        "owner": _owner_payload(owner),
        "html_url": f"https://github.com/{full_name}",
        "description": description,
        "fork": fork,
        "url": base_url,
        "forks_url": f"{base_url}/forks",
        "keys_url": f"{base_url}/keys{{/key_id}}",
        "collaborators_url": f"{base_url}/collaborators{{/collaborator}}",
        "teams_url": f"{base_url}/teams",
        "hooks_url": f"{base_url}/hooks",
        "issue_events_url": f"{base_url}/issues/events{{/number}}",
        "events_url": f"{base_url}/events",
        "assignees_url": f"{base_url}/assignees{{/user}}",
        "branches_url": f"{base_url}/branches{{/branch}}",
        "tags_url": f"{base_url}/tags",
        "blobs_url": f"{base_url}/git/blobs{{/sha}}",
        "git_tags_url": f"{base_url}/git/tags{{/sha}}",
        "git_refs_url": f"{base_url}/git/refs{{/sha}}",
        "trees_url": f"{base_url}/git/trees{{/sha}}",
        "statuses_url": f"{base_url}/statuses/{'{sha}'}",
        "languages_url": f"{base_url}/languages",
        "stargazers_url": f"{base_url}/stargazers",
        "contributors_url": f"{base_url}/contributors",
        "subscribers_url": f"{base_url}/subscribers",
        "subscription_url": f"{base_url}/subscription",
        "commits_url": f"{base_url}/commits{{/sha}}",
        "git_commits_url": f"{base_url}/git/commits{{/sha}}",
        "comments_url": f"{base_url}/comments{{/number}}",
        "issue_comment_url": f"{base_url}/issues/comments{{/number}}",
        "contents_url": f"{base_url}/contents/{{+path}}",
        "compare_url": f"{base_url}/compare/{{base}}...{{head}}",
        "merges_url": f"{base_url}/merges",
        "archive_url": f"{base_url}/{{archive_format}}{{/ref}}",
        "downloads_url": f"{base_url}/downloads",
        "issues_url": f"{base_url}/issues{{/number}}",
        "pulls_url": f"{base_url}/pulls{{/number}}",
        "milestones_url": f"{base_url}/milestones{{/number}}",
        "notifications_url": f"{base_url}/notifications{{?since,all,participating}}",
        "labels_url": f"{base_url}/labels{{/name}}",
        "releases_url": f"{base_url}/releases{{/id}}",
        "deployments_url": f"{base_url}/deployments",
        "created_at": "2024-01-01T00:00:00Z",
        "updated_at": "2024-06-01T00:00:00Z",
        "pushed_at": "2024-06-01T00:00:00Z",
        "git_url": f"git://github.com/{full_name}.git",
        "ssh_url": f"git@github.com:{full_name}.git",
        "clone_url": f"https://github.com/{full_name}.git",
        "svn_url": f"https://github.com/{full_name}",
        "homepage": None,
        "size": 100,
        "stargazers_count": 0,
        "watchers_count": 0,
        "language": language,
        "has_issues": True,
        "has_projects": True,
        "has_downloads": True,
        "has_wiki": True,
        "has_pages": False,
        "has_discussions": True,
        "forks_count": 0,
        "mirror_url": None,
        "archived": False,
        "disabled": False,
        "open_issues_count": 0,
        "license": None,
        "allow_forking": True,
        "is_template": False,
        "web_commit_signoff_required": False,
        "topics": topics or [],
        "visibility": "private" if private else "public",
        "forks": 0,
        "open_issues": 0,
        "watchers": 0,
        "default_branch": "main",
        "permissions": {
            "admin": True,
            "maintain": True,
            "push": True,
            "triage": True,
            "pull": True,
        },
    }


def _issue_payload(
    owner: str,
    name: str,
    number: int,
    title: str = "Test issue",
    body: str = "Issue body",
    state: str = "open",
) -> dict[str, Any]:
    full_name = f"{owner}/{name}"
    base_url = f"https://api.github.com/repos/{full_name}"
    return {
        "url": f"{base_url}/issues/{number}",
        "repository_url": base_url,
        "labels_url": f"{base_url}/issues/{number}/labels{{/name}}",
        "comments_url": f"{base_url}/issues/{number}/comments",
        "events_url": f"{base_url}/issues/{number}/events",
        "html_url": f"https://github.com/{full_name}/issues/{number}",
        "id": number * 100,
        "node_id": f"I_kg{number}",
        "number": number,
        "title": title,
        "user": _owner_payload("reporter", 2),
        "labels": [],
        "state": state,
        "locked": False,
        "assignee": None,
        "assignees": [],
        "milestone": None,
        "comments": 0,
        "created_at": "2024-03-01T00:00:00Z",
        "updated_at": "2024-03-15T00:00:00Z",
        "closed_at": None,
        "author_association": "NONE",
        "active_lock_reason": None,
        "body": body,
        "reactions": {
            "url": f"{base_url}/issues/{number}/reactions",
            "total_count": 0,
            "+1": 0,
            "-1": 0,
            "laugh": 0,
            "hooray": 0,
            "confused": 0,
            "heart": 0,
            "rocket": 0,
            "eyes": 0,
        },
        "timeline_url": f"{base_url}/issues/{number}/timeline",
        "performed_via_github_app": None,
        "state_reason": None,
    }


def _pr_payload(
    owner: str,
    name: str,
    number: int,
    title: str = "Test PR",
    body: str = "PR body",
    state: str = "open",
    merged_at: str | None = None,
) -> dict[str, Any]:
    full_name = f"{owner}/{name}"
    base_url = f"https://api.github.com/repos/{full_name}"
    return {
        "url": f"{base_url}/pulls/{number}",
        "id": number * 100,
        "node_id": f"PR_kg{number}",
        "html_url": f"https://github.com/{full_name}/pull/{number}",
        "diff_url": f"https://github.com/{full_name}/pull/{number}.diff",
        "patch_url": f"https://github.com/{full_name}/pull/{number}.patch",
        "issue_url": f"{base_url}/issues/{number}",
        "number": number,
        "state": state,
        "locked": False,
        "title": title,
        "user": _owner_payload("dev1", 3),
        "body": body,
        "created_at": "2024-03-20T00:00:00Z",
        "updated_at": "2024-04-01T00:00:00Z",
        "closed_at": "2024-04-01T00:00:00Z" if state == "closed" else None,
        "merged_at": merged_at,
        "merge_commit_sha": "abc123" if merged_at else None,
        "assignee": None,
        "assignees": [],
        "requested_reviewers": [],
        "requested_teams": [],
        "labels": [],
        "milestone": None,
        "draft": False,
        "commits_url": f"{base_url}/pulls/{number}/commits",
        "review_comments_url": f"{base_url}/pulls/{number}/comments",
        "review_comment_url": f"{base_url}/pulls/comments{{/number}}",
        "comments_url": f"{base_url}/issues/{number}/comments",
        "statuses_url": f"{base_url}/statuses/abc123",
        "head": {
            "label": f"{owner}:feature",
            "ref": "feature",
            "sha": "abc123",
            "user": _owner_payload(owner),
            "repo": _repo_payload(owner, name, repo_id=1),
        },
        "base": {
            "label": f"{owner}:main",
            "ref": "main",
            "sha": "def456",
            "user": _owner_payload(owner),
            "repo": _repo_payload(owner, name, repo_id=1),
        },
        "author_association": "OWNER",
        "auto_merge": None,
        "active_lock_reason": None,
        "_links": {
            "self": {"href": f"{base_url}/pulls/{number}"},
            "html": {"href": f"https://github.com/{full_name}/pull/{number}"},
            "issue": {"href": f"{base_url}/issues/{number}"},
            "comments": {"href": f"{base_url}/issues/{number}/comments"},
            "review_comments": {"href": f"{base_url}/pulls/{number}/comments"},
            "review_comment": {"href": f"{base_url}/pulls/comments{{/number}}"},
            "commits": {"href": f"{base_url}/pulls/{number}/commits"},
            "statuses": {"href": f"{base_url}/statuses/abc123"},
        },
    }


# ---------------------------------------------------------------------------
# Mock GitHub API
# ---------------------------------------------------------------------------


class MockGitHubAPI:
    """Controllable mock of the GitHub REST + GraphQL API."""

    def __init__(self) -> None:
        self.repos: dict[str, dict[str, Any]] = {}
        self.issues: dict[str, list[dict[str, Any]]] = {}
        self.pull_requests: dict[str, list[dict[str, Any]]] = {}
        self.discussions: dict[str, list[dict[str, Any]]] = {}
        self.issue_comments: dict[str, list[dict[str, Any]]] = {}
        self.review_comments: dict[str, list[dict[str, Any]]] = {}
        self.readmes: dict[str, str] = {}
        self.should_fail_auth: bool = False
        self.authenticated_user: str = "testbot"

    def reset(self) -> None:
        self.repos.clear()
        self.issues.clear()
        self.pull_requests.clear()
        self.discussions.clear()
        self.issue_comments.clear()
        self.review_comments.clear()
        self.readmes.clear()
        self.should_fail_auth = False

    def add_repo(
        self,
        owner: str,
        name: str,
        *,
        private: bool = False,
        description: str = "",
        language: str = "Python",
        topics: list[str] | None = None,
        fork: bool = False,
    ) -> None:
        full_name = f"{owner}/{name}"
        self.repos[full_name] = _repo_payload(
            owner,
            name,
            repo_id=len(self.repos) + 1,
            private=private,
            description=description,
            language=language,
            topics=topics,
            fork=fork,
        )

    def add_issue(
        self,
        owner: str,
        name: str,
        number: int,
        title: str = "Test issue",
        body: str = "Issue body",
        state: str = "open",
    ) -> None:
        full_name = f"{owner}/{name}"
        self.issues.setdefault(full_name, []).append(
            _issue_payload(owner, name, number, title, body, state)
        )

    def add_pull_request(
        self,
        owner: str,
        name: str,
        number: int,
        title: str = "Test PR",
        body: str = "PR body",
        state: str = "open",
        merged_at: str | None = None,
    ) -> None:
        full_name = f"{owner}/{name}"
        self.pull_requests.setdefault(full_name, []).append(
            _pr_payload(owner, name, number, title, body, state, merged_at)
        )

    def add_discussion(
        self,
        owner: str,
        name: str,
        number: int,
        title: str = "Test discussion",
        body: str = "Discussion body",
    ) -> None:
        full_name = f"{owner}/{name}"
        self.discussions.setdefault(full_name, []).append(
            {
                "number": number,
                "title": title,
                "body": body,
                "url": f"https://github.com/{full_name}/discussions/{number}",
                "createdAt": "2024-02-01T00:00:00Z",
                "updatedAt": "2024-02-10T00:00:00Z",
                "author": {"login": "user1"},
                "category": {"name": "General"},
                "answerChosenAt": None,
                "labels": {"nodes": []},
                "comments": {"nodes": []},
            }
        )

    def add_readme(self, owner: str, name: str, content: str) -> None:
        self.readmes[f"{owner}/{name}"] = content

    def create_app(self) -> Starlette:
        mock = self

        async def get_user(request: Request) -> JSONResponse:
            if mock.should_fail_auth:
                return JSONResponse({"message": "Bad credentials"}, status_code=401)
            return JSONResponse(_user_payload(mock.authenticated_user))

        async def get_repo(request: Request) -> JSONResponse:
            if mock.should_fail_auth:
                return JSONResponse({"message": "Bad credentials"}, status_code=401)
            owner = request.path_params["owner"]
            repo = request.path_params["repo"]
            full_name = f"{owner}/{repo}"
            if full_name not in mock.repos:
                return JSONResponse({"message": "Not Found"}, status_code=404)
            return JSONResponse(mock.repos[full_name])

        async def get_readme(request: Request) -> JSONResponse:
            if mock.should_fail_auth:
                return JSONResponse({"message": "Bad credentials"}, status_code=401)
            owner = request.path_params["owner"]
            repo = request.path_params["repo"]
            key = f"{owner}/{repo}"
            if key not in mock.readmes:
                return JSONResponse({"message": "Not Found"}, status_code=404)
            encoded = base64.b64encode(mock.readmes[key].encode()).decode()
            return JSONResponse(
                {
                    "type": "file",
                    "encoding": "base64",
                    "size": len(mock.readmes[key]),
                    "name": "README.md",
                    "path": "README.md",
                    "content": encoded,
                    "sha": "abc123",
                    "url": f"https://api.github.com/repos/{key}/contents/README.md",
                    "git_url": f"https://api.github.com/repos/{key}/git/blobs/abc123",
                    "html_url": f"https://github.com/{key}/blob/main/README.md",
                    "download_url": f"https://raw.githubusercontent.com/{key}/main/README.md",
                    "_links": {
                        "self": f"https://api.github.com/repos/{key}/contents/README.md",
                        "git": f"https://api.github.com/repos/{key}/git/blobs/abc123",
                        "html": f"https://github.com/{key}/blob/main/README.md",
                    },
                }
            )

        def _paginated(request: Request, items: list) -> JSONResponse:
            """Return items on page 1, empty list on subsequent pages."""
            page = int(request.query_params.get("page", "1"))
            return JSONResponse(items if page <= 1 else [])

        async def list_repos_for_user(request: Request) -> JSONResponse:
            if mock.should_fail_auth:
                return JSONResponse({"message": "Bad credentials"}, status_code=401)
            return _paginated(request, list(mock.repos.values()))

        async def list_issues(request: Request) -> JSONResponse:
            if mock.should_fail_auth:
                return JSONResponse({"message": "Bad credentials"}, status_code=401)
            owner = request.path_params["owner"]
            repo = request.path_params["repo"]
            full_name = f"{owner}/{repo}"
            return _paginated(request, mock.issues.get(full_name, []))

        async def list_pulls(request: Request) -> JSONResponse:
            if mock.should_fail_auth:
                return JSONResponse({"message": "Bad credentials"}, status_code=401)
            owner = request.path_params["owner"]
            repo = request.path_params["repo"]
            full_name = f"{owner}/{repo}"
            return _paginated(request, mock.pull_requests.get(full_name, []))

        async def list_issue_comments(request: Request) -> JSONResponse:
            if mock.should_fail_auth:
                return JSONResponse({"message": "Bad credentials"}, status_code=401)
            owner = request.path_params["owner"]
            repo = request.path_params["repo"]
            number = request.path_params["number"]
            key = f"{owner}/{repo}#{number}"
            return _paginated(request, mock.issue_comments.get(key, []))

        async def list_pr_review_comments(request: Request) -> JSONResponse:
            if mock.should_fail_auth:
                return JSONResponse({"message": "Bad credentials"}, status_code=401)
            owner = request.path_params["owner"]
            repo = request.path_params["repo"]
            number = request.path_params["number"]
            key = f"{owner}/{repo}#{number}"
            return _paginated(request, mock.review_comments.get(key, []))

        async def graphql(request: Request) -> JSONResponse:
            if mock.should_fail_auth:
                return JSONResponse({"message": "Bad credentials"}, status_code=401)
            body = await request.json()
            variables = body.get("variables", {})
            owner = variables.get("owner", "")
            name = variables.get("name", "")
            full_name = f"{owner}/{name}"
            discussions = mock.discussions.get(full_name, [])
            return JSONResponse(
                {
                    "data": {
                        "repository": {
                            "discussions": {
                                "pageInfo": {"hasNextPage": False, "endCursor": None},
                                "nodes": discussions,
                            }
                        }
                    }
                }
            )

        routes = [
            Route("/user", get_user),
            Route("/users/{username}/repos", list_repos_for_user),
            Route("/user/repos", list_repos_for_user),
            Route("/repos/{owner}/{repo}", get_repo),
            Route("/repos/{owner}/{repo}/readme", get_readme),
            Route("/repos/{owner}/{repo}/issues", list_issues),
            Route(
                "/repos/{owner}/{repo}/issues/{number}/comments", list_issue_comments
            ),
            Route("/repos/{owner}/{repo}/pulls", list_pulls),
            Route(
                "/repos/{owner}/{repo}/pulls/{number}/comments", list_pr_review_comments
            ),
            Route("/graphql", graphql, methods=["POST"]),
        ]
        return Starlette(routes=routes)


# ---------------------------------------------------------------------------
# Helpers
# ---------------------------------------------------------------------------


def _free_port() -> int:
    with socket.socket(socket.AF_INET, socket.SOCK_STREAM) as s:
        s.bind(("", 0))
        return s.getsockname()[1]


def _wait_for_port(port: int, host: str = "localhost", timeout: float = 10) -> None:
    """Block until a TCP port is accepting connections."""
    deadline = time.monotonic() + timeout
    while time.monotonic() < deadline:
        try:
            with socket.create_connection((host, port), timeout=1):
                return
        except OSError:
            time.sleep(0.1)
    raise TimeoutError(f"Port {port} not open after {timeout}s")


# ---------------------------------------------------------------------------
# Session-scoped fixtures
# ---------------------------------------------------------------------------


@pytest.fixture(scope="session")
def mock_github_api() -> MockGitHubAPI:
    return MockGitHubAPI()


@pytest.fixture(scope="session")
def mock_github_server(mock_github_api: MockGitHubAPI) -> str:
    """Start mock GitHub API server in a daemon thread. Returns base URL."""
    port = _free_port()
    app = mock_github_api.create_app()
    config = uvicorn.Config(app, host="0.0.0.0", port=port, log_level="warning")
    server = uvicorn.Server(config)

    thread = threading.Thread(target=server.run, daemon=True)
    thread.start()

    _wait_for_port(port)
    return f"http://localhost:{port}"


@pytest.fixture(scope="session")
def connector_port() -> int:
    return _free_port()


@pytest.fixture(scope="session")
def connector_server(connector_port: int) -> str:
    """Start the GitHub connector as a uvicorn server in a daemon thread. Returns base URL."""
    import os

    os.environ.setdefault("CONNECTOR_MANAGER_URL", "http://localhost:0")

    from github_connector import GitHubConnector
    from omni_connector.server import create_app

    app = create_app(GitHubConnector())
    config = uvicorn.Config(
        app, host="0.0.0.0", port=connector_port, log_level="warning"
    )
    server = uvicorn.Server(config)

    thread = threading.Thread(target=server.run, daemon=True)
    thread.start()

    _wait_for_port(connector_port)
    return f"http://localhost:{connector_port}"


@pytest_asyncio.fixture(scope="session")
async def harness(
    connector_server: str,
    connector_port: int,
) -> OmniTestHarness:
    """Session-scoped OmniTestHarness with all infrastructure started."""
    import os

    h = OmniTestHarness()
    await h.start_infra()
    await h.start_connector_manager(
        {
            "CONNECTOR_GITHUB_URL": f"http://host.docker.internal:{connector_port}",
        }
    )

    os.environ["CONNECTOR_MANAGER_URL"] = h.connector_manager_url

    yield h
    await h.teardown()


# ---------------------------------------------------------------------------
# Function-scoped fixtures
# ---------------------------------------------------------------------------


@pytest_asyncio.fixture
async def seed(harness: OmniTestHarness) -> SeedHelper:
    return harness.seed()


@pytest_asyncio.fixture
async def source_id(
    seed: SeedHelper,
    mock_github_server: str,
    mock_github_api: MockGitHubAPI,
) -> str:
    """Create a GitHub source with credentials pointing to the mock server."""
    mock_github_api.reset()
    sid = await seed.create_source(
        source_type="github",
        config={"api_url": mock_github_server, "include_discussions": True},
    )
    await seed.create_credentials(sid, {"token": "test-token-abc123"})
    return sid


@pytest_asyncio.fixture
async def cm_client(harness: OmniTestHarness) -> httpx.AsyncClient:
    """Async httpx client pointed at the connector-manager."""
    async with httpx.AsyncClient(
        base_url=harness.connector_manager_url, timeout=30
    ) as client:
        yield client

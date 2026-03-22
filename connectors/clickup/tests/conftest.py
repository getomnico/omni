"""Integration test fixtures for the ClickUp connector.

Session-scoped: harness, mock ClickUp API server, connector server, connector-manager.
Function-scoped: seed helper, source_id, httpx client.
"""

from __future__ import annotations

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


_DEFAULT_MEMBERS: list[dict[str, Any]] = [
    {"user": {"id": 1, "username": "alice", "email": "alice@test.com"}, "role": 1},
    {"user": {"id": 2, "username": "bob", "email": "bob@test.com"}, "role": 3},
]


def _workspace_payload(
    team_id: str = "team_1",
    name: str = "Test Workspace",
    members: list[dict[str, Any]] | None = None,
) -> dict[str, Any]:
    return {
        "id": team_id,
        "name": name,
        "color": "#000000",
        "avatar": None,
        "members": members if members is not None else _DEFAULT_MEMBERS,
    }


def _space_payload(
    space_id: str,
    name: str = "Engineering",
    private: bool = False,
    members: list[dict[str, Any]] | None = None,
) -> dict[str, Any]:
    return {
        "id": space_id,
        "name": name,
        "color": "#000000",
        "avatar": None,
        "private": private,
        "members": members or [],
    }


def _folder_payload(folder_id: str, name: str = "Sprint 1") -> dict[str, Any]:
    return {"id": folder_id, "name": name, "lists": []}


def _list_payload(
    list_id: str,
    name: str = "Backlog",
    folder_id: str | None = None,
    space_id: str | None = None,
) -> dict[str, Any]:
    return {
        "id": list_id,
        "name": name,
        "folder": {"id": folder_id} if folder_id else None,
        "space": {"id": space_id} if space_id else None,
    }


def _task_payload(
    task_id: str,
    name: str = "Test task",
    description: str = "Task description",
    status: str = "open",
    priority: str = "normal",
    list_id: str = "list_1",
    date_created: str = "1709280000000",
    date_updated: str = "1709884800000",
    parent: str | None = None,
    assignees: list[dict[str, Any]] | None = None,
    tags: list[dict[str, Any]] | None = None,
    custom_fields: list[dict[str, Any]] | None = None,
) -> dict[str, Any]:
    return {
        "id": task_id,
        "name": name,
        "description": description,
        "text_content": description,
        "status": {"status": status, "type": "open"},
        "priority": {"priority": priority} if priority else None,
        "creator": {"id": 1, "username": "creator", "email": "creator@test.com"},
        "assignees": assignees or [],
        "tags": tags or [],
        "custom_fields": custom_fields or [],
        "list": {"id": list_id},
        "url": f"https://app.clickup.com/t/{task_id}",
        "date_created": date_created,
        "date_updated": date_updated,
        "due_date": None,
        "parent": parent,
    }


def _comment_payload(
    comment_id: str,
    text: str = "Test comment",
    username: str = "commenter",
) -> dict[str, Any]:
    return {
        "id": comment_id,
        "comment_text": text,
        "user": {"id": 1, "username": username, "email": f"{username}@test.com"},
        "date": "1709884800000",
    }


# ---------------------------------------------------------------------------
# Mock ClickUp API
# ---------------------------------------------------------------------------


class MockClickUpAPI:
    """Controllable mock of the ClickUp REST API."""

    def __init__(self) -> None:
        self.workspaces: list[dict[str, Any]] = []
        self.spaces: dict[str, list[dict[str, Any]]] = {}
        self.folders: dict[str, list[dict[str, Any]]] = {}
        self.lists_in_folder: dict[str, list[dict[str, Any]]] = {}
        self.folderless_lists: dict[str, list[dict[str, Any]]] = {}
        self.tasks: dict[str, list[dict[str, Any]]] = {}
        self.comments: dict[str, list[dict[str, Any]]] = {}
        self.should_fail_auth: bool = False

    def reset(self) -> None:
        self.workspaces.clear()
        self.spaces.clear()
        self.folders.clear()
        self.lists_in_folder.clear()
        self.folderless_lists.clear()
        self.tasks.clear()
        self.comments.clear()
        self.should_fail_auth = False

    def add_workspace(
        self,
        team_id: str = "team_1",
        name: str = "Test Workspace",
        members: list[dict[str, Any]] | None = None,
    ) -> None:
        self.workspaces.append(_workspace_payload(team_id, name, members))

    def add_space(
        self,
        team_id: str,
        space_id: str,
        name: str = "Engineering",
        private: bool = False,
        members: list[dict[str, Any]] | None = None,
    ) -> None:
        self.spaces.setdefault(team_id, []).append(
            _space_payload(space_id, name, private, members)
        )

    def add_folder(self, space_id: str, folder_id: str, name: str = "Sprint 1") -> None:
        self.folders.setdefault(space_id, []).append(_folder_payload(folder_id, name))

    def add_list_in_folder(
        self, folder_id: str, list_id: str, name: str = "Backlog"
    ) -> None:
        self.lists_in_folder.setdefault(folder_id, []).append(
            _list_payload(list_id, name, folder_id=folder_id)
        )

    def add_folderless_list(
        self, space_id: str, list_id: str, name: str = "Backlog"
    ) -> None:
        self.folderless_lists.setdefault(space_id, []).append(
            _list_payload(list_id, name, space_id=space_id)
        )

    def add_task(self, team_id: str, **kwargs: Any) -> None:
        task = _task_payload(**kwargs)
        self.tasks.setdefault(team_id, []).append(task)

    def add_comment(self, task_id: str, **kwargs: Any) -> None:
        comment = _comment_payload(**kwargs)
        self.comments.setdefault(task_id, []).append(comment)

    def create_app(self) -> Starlette:
        mock = self

        async def get_teams(request: Request) -> JSONResponse:
            if mock.should_fail_auth:
                return JSONResponse({"err": "Token invalid"}, status_code=401)
            return JSONResponse({"teams": mock.workspaces})

        async def get_spaces(request: Request) -> JSONResponse:
            if mock.should_fail_auth:
                return JSONResponse({"err": "Token invalid"}, status_code=401)
            team_id = request.path_params["team_id"]
            return JSONResponse({"spaces": mock.spaces.get(team_id, [])})

        async def get_folders(request: Request) -> JSONResponse:
            if mock.should_fail_auth:
                return JSONResponse({"err": "Token invalid"}, status_code=401)
            space_id = request.path_params["space_id"]
            return JSONResponse({"folders": mock.folders.get(space_id, [])})

        async def get_lists_in_folder(request: Request) -> JSONResponse:
            if mock.should_fail_auth:
                return JSONResponse({"err": "Token invalid"}, status_code=401)
            folder_id = request.path_params["folder_id"]
            return JSONResponse({"lists": mock.lists_in_folder.get(folder_id, [])})

        async def get_folderless_lists(request: Request) -> JSONResponse:
            if mock.should_fail_auth:
                return JSONResponse({"err": "Token invalid"}, status_code=401)
            space_id = request.path_params["space_id"]
            return JSONResponse({"lists": mock.folderless_lists.get(space_id, [])})

        async def get_team_tasks(request: Request) -> JSONResponse:
            if mock.should_fail_auth:
                return JSONResponse({"err": "Token invalid"}, status_code=401)
            team_id = request.path_params["team_id"]
            all_tasks = mock.tasks.get(team_id, [])

            date_updated_gt = request.query_params.get("date_updated_gt")
            if date_updated_gt:
                threshold = int(date_updated_gt)
                all_tasks = [
                    t for t in all_tasks if int(t.get("date_updated", 0)) > threshold
                ]

            page = int(request.query_params.get("page", "0"))
            start = page * 100
            end = start + 100
            return JSONResponse({"tasks": all_tasks[start:end]})

        async def get_task_comments(request: Request) -> JSONResponse:
            if mock.should_fail_auth:
                return JSONResponse({"err": "Token invalid"}, status_code=401)
            task_id = request.path_params["task_id"]
            return JSONResponse({"comments": mock.comments.get(task_id, [])})

        routes = [
            Route("/api/v2/team", get_teams),
            Route("/api/v2/team/{team_id}/space", get_spaces),
            Route("/api/v2/space/{space_id}/folder", get_folders),
            Route("/api/v2/folder/{folder_id}/list", get_lists_in_folder),
            Route("/api/v2/space/{space_id}/list", get_folderless_lists),
            Route("/api/v2/team/{team_id}/task", get_team_tasks),
            Route("/api/v2/task/{task_id}/comment", get_task_comments),
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
def mock_clickup_api() -> MockClickUpAPI:
    return MockClickUpAPI()


@pytest.fixture(scope="session")
def mock_clickup_server(mock_clickup_api: MockClickUpAPI) -> str:
    """Start mock ClickUp API server in a daemon thread. Returns base URL."""
    port = _free_port()
    app = mock_clickup_api.create_app()
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
    """Start the ClickUp connector as a uvicorn server in a daemon thread. Returns base URL."""
    import os

    os.environ.setdefault("CONNECTOR_MANAGER_URL", "http://localhost:0")

    from clickup_connector import ClickUpConnector
    from omni_connector.server import create_app

    app = create_app(ClickUpConnector())
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
            "CLICKUP_CONNECTOR_URL": f"http://host.docker.internal:{connector_port}",
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
    mock_clickup_server: str,
    mock_clickup_api: MockClickUpAPI,
) -> str:
    """Create a ClickUp source with credentials pointing to the mock server."""
    mock_clickup_api.reset()
    sid = await seed.create_source(
        source_type="clickup",
        config={"api_url": mock_clickup_server, "include_docs": False},
    )
    await seed.create_credentials(sid, {"token": "pk_test_token_abc123"})
    return sid


@pytest_asyncio.fixture
async def cm_client(harness: OmniTestHarness) -> httpx.AsyncClient:
    """Async httpx client pointed at the connector-manager."""
    async with httpx.AsyncClient(
        base_url=harness.connector_manager_url, timeout=30
    ) as client:
        yield client

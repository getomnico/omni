"""Integration test fixtures for the Notion connector.

Session-scoped: harness, mock Notion API server, connector server, connector-manager.
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
# Mock data payload helpers
# ---------------------------------------------------------------------------


def _rich_text(text: str) -> list[dict[str, Any]]:
    return [{"type": "text", "text": {"content": text}, "plain_text": text}]


def _page_payload(
    page_id: str,
    title: str,
    parent: dict[str, Any] | None = None,
    properties: dict[str, Any] | None = None,
) -> dict[str, Any]:
    if parent is None:
        parent = {"type": "workspace", "workspace": True}

    base_properties: dict[str, Any] = {
        "title": {"id": "title", "type": "title", "title": _rich_text(title)},
    }
    if properties:
        base_properties.update(properties)

    return {
        "object": "page",
        "id": page_id,
        "created_time": "2024-01-15T10:00:00.000Z",
        "last_edited_time": "2024-06-01T12:00:00.000Z",
        "created_by": {"object": "user", "id": "user-001"},
        "last_edited_by": {"object": "user", "id": "user-001"},
        "cover": None,
        "icon": None,
        "parent": parent,
        "archived": False,
        "in_trash": False,
        "properties": base_properties,
        "url": f"https://www.notion.so/{page_id.replace('-', '')}",
    }


def _database_payload(
    db_id: str,
    title: str,
    properties_schema: dict[str, Any],
    description: str = "",
) -> dict[str, Any]:
    return {
        "object": "database",
        "id": db_id,
        "created_time": "2024-01-10T08:00:00.000Z",
        "last_edited_time": "2024-06-01T12:00:00.000Z",
        "created_by": {"object": "user", "id": "user-001"},
        "last_edited_by": {"object": "user", "id": "user-001"},
        "title": _rich_text(title),
        "description": _rich_text(description) if description else [],
        "icon": None,
        "cover": None,
        "properties": properties_schema,
        "parent": {"type": "workspace", "workspace": True},
        "url": f"https://www.notion.so/{db_id.replace('-', '')}",
        "archived": False,
        "in_trash": False,
        "is_inline": False,
    }


def _block_payload(
    block_id: str,
    block_type: str,
    text: str,
    has_children: bool = False,
) -> dict[str, Any]:
    block: dict[str, Any] = {
        "object": "block",
        "id": block_id,
        "parent": {"type": "page_id", "page_id": "parent-page"},
        "created_time": "2024-01-15T10:00:00.000Z",
        "last_edited_time": "2024-01-15T10:00:00.000Z",
        "created_by": {"object": "user", "id": "user-001"},
        "last_edited_by": {"object": "user", "id": "user-001"},
        "has_children": has_children,
        "archived": False,
        "in_trash": False,
        "type": block_type,
        block_type: {"rich_text": _rich_text(text)},
    }
    return block


# ---------------------------------------------------------------------------
# Mock Notion API
# ---------------------------------------------------------------------------


class MockNotionAPI:
    """Controllable mock of the Notion API v1 endpoints."""

    def __init__(self) -> None:
        self.pages: dict[str, dict[str, Any]] = {}
        self.databases: dict[str, dict[str, Any]] = {}
        self.database_pages: dict[str, list[dict[str, Any]]] = {}
        self.blocks: dict[str, list[dict[str, Any]]] = {}
        self.users: list[dict[str, Any]] = []
        self.should_fail_auth: bool = False

    def reset(self) -> None:
        self.pages.clear()
        self.databases.clear()
        self.database_pages.clear()
        self.blocks.clear()
        self.users.clear()
        self.should_fail_auth = False

    def add_page(
        self,
        page_id: str,
        title: str,
        blocks: list[dict[str, Any]],
        parent: dict[str, Any] | None = None,
    ) -> None:
        self.pages[page_id] = _page_payload(page_id, title, parent=parent)
        self.blocks[page_id] = blocks

    def add_database(
        self,
        db_id: str,
        title: str,
        properties_schema: dict[str, Any],
        description: str = "",
    ) -> None:
        self.databases[db_id] = _database_payload(
            db_id, title, properties_schema, description
        )

    def add_user(
        self,
        user_id: str,
        name: str,
        email: str | None = None,
        user_type: str = "person",
    ) -> None:
        user: dict[str, Any] = {
            "object": "user",
            "id": user_id,
            "type": user_type,
            "name": name,
        }
        if user_type == "person":
            user["person"] = {"email": email}
        elif user_type == "bot":
            user["bot"] = {}
        self.users.append(user)

    def add_database_entry(
        self,
        db_id: str,
        page_id: str,
        title: str,
        properties: dict[str, Any],
        blocks: list[dict[str, Any]],
    ) -> None:
        parent = {"type": "database_id", "database_id": db_id}
        page = _page_payload(page_id, title, parent=parent, properties=properties)
        self.database_pages.setdefault(db_id, []).append(page)
        self.blocks[page_id] = blocks

    def create_app(self) -> Starlette:
        mock = self

        async def users_me(request: Request) -> JSONResponse:
            if mock.should_fail_auth:
                return JSONResponse(
                    {
                        "object": "error",
                        "status": 401,
                        "code": "unauthorized",
                        "message": "API token is invalid.",
                    },
                    status_code=401,
                )
            return JSONResponse(
                {
                    "object": "user",
                    "id": "bot-001",
                    "type": "bot",
                    "name": "Test Integration",
                    "bot": {
                        "owner": {"type": "workspace", "workspace": True},
                        "workspace_name": "Test Workspace",
                    },
                }
            )

        async def list_users(request: Request) -> JSONResponse:
            if mock.should_fail_auth:
                return JSONResponse(
                    {
                        "object": "error",
                        "status": 401,
                        "code": "unauthorized",
                        "message": "API token is invalid.",
                    },
                    status_code=401,
                )
            return JSONResponse(
                {
                    "object": "list",
                    "results": mock.users,
                    "has_more": False,
                    "next_cursor": None,
                    "type": "user",
                }
            )

        async def search(request: Request) -> JSONResponse:
            if mock.should_fail_auth:
                return JSONResponse(
                    {
                        "object": "error",
                        "status": 401,
                        "code": "unauthorized",
                        "message": "API token is invalid.",
                    },
                    status_code=401,
                )
            body = await request.json()
            filter_value = body.get("filter", {}).get("value")

            if filter_value == "page":
                results = list(mock.pages.values())
            elif filter_value == "database":
                results = list(mock.databases.values())
            else:
                results = list(mock.pages.values()) + list(mock.databases.values())

            return JSONResponse(
                {
                    "object": "list",
                    "results": results,
                    "has_more": False,
                    "next_cursor": None,
                    "type": "page_or_database",
                }
            )

        async def query_database(request: Request) -> JSONResponse:
            if mock.should_fail_auth:
                return JSONResponse(
                    {
                        "object": "error",
                        "status": 401,
                        "code": "unauthorized",
                        "message": "API token is invalid.",
                    },
                    status_code=401,
                )
            db_id = request.path_params["database_id"]
            pages = mock.database_pages.get(db_id, [])
            return JSONResponse(
                {
                    "object": "list",
                    "results": pages,
                    "has_more": False,
                    "next_cursor": None,
                    "type": "page_or_database",
                }
            )

        async def get_block_children(request: Request) -> JSONResponse:
            if mock.should_fail_auth:
                return JSONResponse(
                    {
                        "object": "error",
                        "status": 401,
                        "code": "unauthorized",
                        "message": "API token is invalid.",
                    },
                    status_code=401,
                )
            block_id = request.path_params["block_id"]
            children = mock.blocks.get(block_id, [])
            return JSONResponse(
                {
                    "object": "list",
                    "results": children,
                    "has_more": False,
                    "next_cursor": None,
                    "type": "block",
                }
            )

        routes = [
            Route("/v1/users/me", users_me),
            Route("/v1/users", list_users),
            Route("/v1/search", search, methods=["POST"]),
            Route(
                "/v1/databases/{database_id}/query", query_database, methods=["POST"]
            ),
            Route("/v1/blocks/{block_id}/children", get_block_children),
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
def mock_notion_api() -> MockNotionAPI:
    return MockNotionAPI()


@pytest.fixture(scope="session")
def mock_notion_server(mock_notion_api: MockNotionAPI) -> str:
    """Start mock Notion API server in a daemon thread. Returns base URL."""
    port = _free_port()
    app = mock_notion_api.create_app()
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
    """Start the Notion connector as a uvicorn server in a daemon thread. Returns base URL."""
    import os

    os.environ.setdefault("CONNECTOR_MANAGER_URL", "http://localhost:0")

    from notion_connector import NotionConnector
    from omni_connector.server import create_app

    app = create_app(NotionConnector())
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
            "NOTION_CONNECTOR_URL": f"http://host.docker.internal:{connector_port}",
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
    mock_notion_server: str,
    mock_notion_api: MockNotionAPI,
) -> str:
    """Create a Notion source with credentials pointing to the mock server."""
    mock_notion_api.reset()
    sid = await seed.create_source(
        source_type="notion",
        config={"api_url": mock_notion_server},
    )
    await seed.create_credentials(sid, {"token": "test-token"}, provider="notion")
    return sid


@pytest_asyncio.fixture
async def cm_client(harness: OmniTestHarness) -> httpx.AsyncClient:
    """Async httpx client pointed at the connector-manager."""
    async with httpx.AsyncClient(
        base_url=harness.connector_manager_url, timeout=30
    ) as client:
        yield client

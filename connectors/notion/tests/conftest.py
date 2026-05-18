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
from omni_connector.testing import OmniTestHarness, SeedHelper
from starlette.applications import Starlette
from starlette.requests import Request
from starlette.responses import JSONResponse
from starlette.routing import Route

logger = logging.getLogger(__name__)


def _disable_docker_auth_helpers_for_tests() -> None:
    """Prevent local Docker credential helpers from breaking testcontainers."""
    try:
        from docker.auth import AuthConfig
    except ImportError:
        return
    AuthConfig.get_all_credentials = lambda self: {}


_disable_docker_auth_helpers_for_tests()


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
    last_edited_time: str = "2024-06-01T12:00:00.000Z",
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
        "last_edited_time": last_edited_time,
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


def _data_source_payload(
    ds_id: str,
    title: str,
    properties_schema: dict[str, Any],
    description: str = "",
    last_edited_time: str = "2024-06-01T12:00:00.000Z",
) -> dict[str, Any]:
    """Payload shape returned by /v1/search for a data source under 2025-09-03+."""
    return {
        "object": "data_source",
        "id": ds_id,
        "created_time": "2024-01-10T08:00:00.000Z",
        "last_edited_time": last_edited_time,
        "created_by": {"object": "user", "id": "user-001"},
        "last_edited_by": {"object": "user", "id": "user-001"},
        "title": _rich_text(title),
        "description": _rich_text(description) if description else [],
        "icon": None,
        "cover": None,
        "properties": properties_schema,
        "parent": {"type": "database_id", "database_id": f"db-{ds_id}"},
        "url": f"https://www.notion.so/{ds_id.replace('-', '')}",
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
    """Controllable mock of the Notion API v1 endpoints (Notion-Version 2025-09-03)."""

    def __init__(self) -> None:
        self.pages: dict[str, dict[str, Any]] = {}
        self.data_sources: dict[str, dict[str, Any]] = {}
        self.data_source_pages: dict[str, list[dict[str, Any]]] = {}
        self.blocks: dict[str, list[dict[str, Any]]] = {}
        self.users: list[dict[str, Any]] = []
        self.should_fail_auth: bool = False
        self.should_forbid_users: bool = False
        self.workspace_name: str | None = "Test Workspace"

    def reset(self) -> None:
        self.pages.clear()
        self.data_sources.clear()
        self.data_source_pages.clear()
        self.blocks.clear()
        self.users.clear()
        self.should_fail_auth = False
        self.should_forbid_users = False
        self.workspace_name = "Test Workspace"

    def add_page(
        self,
        page_id: str,
        title: str,
        blocks: list[dict[str, Any]],
        parent: dict[str, Any] | None = None,
        last_edited_time: str = "2024-06-01T12:00:00.000Z",
    ) -> None:
        self.pages[page_id] = _page_payload(
            page_id, title, parent=parent, last_edited_time=last_edited_time
        )
        self.blocks[page_id] = blocks

    def add_data_source(
        self,
        ds_id: str,
        title: str,
        properties_schema: dict[str, Any],
        description: str = "",
        last_edited_time: str = "2024-06-01T12:00:00.000Z",
    ) -> None:
        self.data_sources[ds_id] = _data_source_payload(
            ds_id, title, properties_schema, description, last_edited_time
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

    def add_data_source_entry(
        self,
        ds_id: str,
        page_id: str,
        title: str,
        properties: dict[str, Any],
        blocks: list[dict[str, Any]],
        last_edited_time: str = "2024-06-01T12:00:00.000Z",
    ) -> None:
        parent = {"type": "data_source_id", "data_source_id": ds_id}
        page = _page_payload(
            page_id,
            title,
            parent=parent,
            properties=properties,
            last_edited_time=last_edited_time,
        )
        self.data_source_pages.setdefault(ds_id, []).append(page)
        self.blocks[page_id] = blocks

    def create_app(self) -> Starlette:
        mock = self

        def _unauth() -> JSONResponse:
            return JSONResponse(
                {
                    "object": "error",
                    "status": 401,
                    "code": "unauthorized",
                    "message": "API token is invalid.",
                },
                status_code=401,
            )

        def _sort_desc(items: list[dict[str, Any]]) -> list[dict[str, Any]]:
            return sorted(
                items, key=lambda x: x.get("last_edited_time", ""), reverse=True
            )

        async def users_me(request: Request) -> JSONResponse:
            if mock.should_fail_auth:
                return _unauth()
            bot_payload: dict[str, Any] = {
                "owner": {"type": "workspace", "workspace": True},
                "workspace_name": mock.workspace_name,
            }
            return JSONResponse(
                {
                    "object": "user",
                    "id": "bot-001",
                    "type": "bot",
                    "name": "Test Integration",
                    "bot": bot_payload,
                }
            )

        async def list_users(request: Request) -> JSONResponse:
            if mock.should_fail_auth:
                return _unauth()
            if mock.should_forbid_users:
                return JSONResponse(
                    {
                        "object": "error",
                        "status": 403,
                        "code": "restricted_resource",
                        "message": (
                            "Insufficient permissions for this endpoint. "
                            "Enable 'Read user information' on the integration."
                        ),
                    },
                    status_code=403,
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
                return _unauth()
            body = await request.json()
            filter_obj = body.get("filter", {}) or {}
            filter_value = filter_obj.get("value")

            if filter_value == "page":
                results = _sort_desc(list(mock.pages.values()))
            elif filter_value == "data_source":
                results = _sort_desc(list(mock.data_sources.values()))
            else:
                # Tighten the contract: reject anything else so connector regressions
                # surface immediately instead of falling through to a mixed result set.
                return JSONResponse(
                    {
                        "object": "error",
                        "status": 400,
                        "code": "validation_error",
                        "message": (
                            "filter.value must be 'page' or 'data_source' "
                            f"(got {filter_value!r})"
                        ),
                    },
                    status_code=400,
                )

            return JSONResponse(
                {
                    "object": "list",
                    "results": results,
                    "has_more": False,
                    "next_cursor": None,
                    "type": "page_or_data_source",
                }
            )

        async def query_data_source(request: Request) -> JSONResponse:
            if mock.should_fail_auth:
                return _unauth()
            ds_id = request.path_params["data_source_id"]
            pages = mock.data_source_pages.get(ds_id, [])
            return JSONResponse(
                {
                    "object": "list",
                    "results": pages,
                    "has_more": False,
                    "next_cursor": None,
                    "type": "page_or_data_source",
                }
            )

        async def retrieve_data_source(request: Request) -> JSONResponse:
            if mock.should_fail_auth:
                return _unauth()
            ds_id = request.path_params["data_source_id"]
            data_source = mock.data_sources.get(ds_id)
            if not data_source:
                return JSONResponse(
                    {
                        "object": "error",
                        "status": 404,
                        "code": "object_not_found",
                        "message": f"Data source not found: {ds_id}",
                    },
                    status_code=404,
                )
            return JSONResponse(data_source)

        async def get_block_children(request: Request) -> JSONResponse:
            if mock.should_fail_auth:
                return _unauth()
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
                "/v1/data_sources/{data_source_id}/query",
                query_data_source,
                methods=["POST"],
            ),
            Route(
                "/v1/data_sources/{data_source_id}",
                retrieve_data_source,
                methods=["GET"],
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


@pytest_asyncio.fixture(scope="session")
async def harness() -> OmniTestHarness:
    """Session-scoped OmniTestHarness with infra + connector-manager started.

    Starts before `connector_server` so the connector can register itself
    against the real CM URL on startup (SDK reads CONNECTOR_MANAGER_URL once
    at create_app and caches it).
    """
    h = OmniTestHarness()
    await h.start_infra()
    await h.start_connector_manager()

    yield h
    await h.teardown()


def _wait_for_registration(cm_url: str, source_type: str, timeout: float = 15) -> None:
    """Poll CM until the connector has registered itself."""
    deadline = time.monotonic() + timeout
    last_err: Exception | None = None
    while time.monotonic() < deadline:
        try:
            with httpx.Client(timeout=2) as client:
                resp = client.get(f"{cm_url}/connectors")
                if resp.status_code == 200:
                    payload = resp.json()
                    for c in payload:
                        if source_type in (c.get("manifest") or {}).get(
                            "source_types", []
                        ):
                            return
        except Exception as e:
            last_err = e
        time.sleep(0.2)
    raise TimeoutError(
        f"Connector for source_type={source_type} did not register within "
        f"{timeout}s (last err: {last_err})"
    )


@pytest.fixture(scope="session")
def connector_server(connector_port: int, harness: OmniTestHarness) -> str:
    """Start the Notion connector as a uvicorn server in a daemon thread.

    Depends on `harness` so we can point CONNECTOR_MANAGER_URL at the real CM
    URL before `create_app` runs (SdkClient reads the env var once at
    construction and caches it).
    """
    import os

    os.environ["CONNECTOR_MANAGER_URL"] = harness.connector_manager_url
    # Connector advertises this hostname in its registration manifest. CM
    # health-checks the URL from inside its container, so localhost would
    # resolve to the container itself — use host.docker.internal instead.
    os.environ["CONNECTOR_HOST_NAME"] = "host.docker.internal"
    os.environ.setdefault("PORT", str(connector_port))

    from omni_connector.server import create_app

    from notion_connector import NotionConnector

    app = create_app(NotionConnector())
    config = uvicorn.Config(
        app, host="0.0.0.0", port=connector_port, log_level="warning"
    )
    server = uvicorn.Server(config)

    thread = threading.Thread(target=server.run, daemon=True)
    thread.start()

    _wait_for_port(connector_port)
    _wait_for_registration(harness.connector_manager_url, "notion")
    return f"http://localhost:{connector_port}"


# ---------------------------------------------------------------------------
# Function-scoped fixtures
# ---------------------------------------------------------------------------


@pytest_asyncio.fixture
async def seed(harness: OmniTestHarness) -> SeedHelper:
    return harness.seed()


@pytest_asyncio.fixture
async def source_id(
    connector_server: str,
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

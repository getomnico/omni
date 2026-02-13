"""Integration test fixtures for the Microsoft connector.

Session-scoped: harness, mock Graph API server, connector server, connector-manager.
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
from starlette.responses import JSONResponse, Response
from starlette.routing import Route

from omni_connector.testing import OmniTestHarness, SeedHelper

logger = logging.getLogger(__name__)


# ---------------------------------------------------------------------------
# Mock Graph API
# ---------------------------------------------------------------------------


class MockGraphAPI:
    """Controllable mock of the Microsoft Graph API v1.0 endpoints."""

    def __init__(self) -> None:
        self.users: list[dict[str, Any]] = []
        self.drive_items: dict[str, list[dict[str, Any]]] = {}
        self.mail_messages: dict[str, list[dict[str, Any]]] = {}
        self.calendar_events: dict[str, list[dict[str, Any]]] = {}
        self.sites: list[dict[str, Any]] = []
        self.site_drive_items: dict[str, list[dict[str, Any]]] = {}
        self.file_contents: dict[str, bytes] = {}

    def reset(self) -> None:
        self.users.clear()
        self.drive_items.clear()
        self.mail_messages.clear()
        self.calendar_events.clear()
        self.sites.clear()
        self.site_drive_items.clear()
        self.file_contents.clear()

    def add_user(self, user: dict[str, Any]) -> None:
        self.users.append(user)

    def add_drive_item(self, user_id: str, item: dict[str, Any]) -> None:
        self.drive_items.setdefault(user_id, []).append(item)

    def add_mail_message(self, user_id: str, message: dict[str, Any]) -> None:
        self.mail_messages.setdefault(user_id, []).append(message)

    def add_calendar_event(self, user_id: str, event: dict[str, Any]) -> None:
        self.calendar_events.setdefault(user_id, []).append(event)

    def add_site(self, site: dict[str, Any]) -> None:
        self.sites.append(site)

    def add_site_drive_item(self, site_id: str, item: dict[str, Any]) -> None:
        self.site_drive_items.setdefault(site_id, []).append(item)

    def set_file_content(self, drive_id: str, item_id: str, content: bytes) -> None:
        self.file_contents[f"{drive_id}:{item_id}"] = content

    def create_app(self, base_url: str) -> Starlette:
        mock = self

        async def organization(request: Request) -> JSONResponse:
            return JSONResponse(
                {"value": [{"id": "org-001", "displayName": "Test Org"}]}
            )

        async def list_users(request: Request) -> JSONResponse:
            return JSONResponse({"value": mock.users})

        async def user_drive_delta(request: Request) -> JSONResponse:
            uid = request.path_params["uid"]
            items = mock.drive_items.get(uid, [])
            delta_link = f"{base_url}/users/{uid}/drive/root/delta?deltatoken=latest"
            return JSONResponse({"value": items, "@odata.deltaLink": delta_link})

        async def drive_item_content(request: Request) -> Response:
            did = request.path_params["did"]
            iid = request.path_params["iid"]
            key = f"{did}:{iid}"
            content = mock.file_contents.get(key, b"file content placeholder")
            return Response(content=content, media_type="application/octet-stream")

        async def mail_delta(request: Request) -> JSONResponse:
            uid = request.path_params["uid"]
            messages = mock.mail_messages.get(uid, [])
            delta_link = (
                f"{base_url}/users/{uid}/mailFolders/inbox/messages/delta"
                f"?deltatoken=latest"
            )
            return JSONResponse({"value": messages, "@odata.deltaLink": delta_link})

        async def calendar_delta(request: Request) -> JSONResponse:
            uid = request.path_params["uid"]
            events = mock.calendar_events.get(uid, [])
            delta_link = f"{base_url}/users/{uid}/calendarView/delta?deltatoken=latest"
            return JSONResponse({"value": events, "@odata.deltaLink": delta_link})

        async def list_sites(request: Request) -> JSONResponse:
            return JSONResponse({"value": mock.sites})

        async def site_drive_delta(request: Request) -> JSONResponse:
            sid = request.path_params["sid"]
            items = mock.site_drive_items.get(sid, [])
            delta_link = f"{base_url}/sites/{sid}/drive/root/delta?deltatoken=latest"
            return JSONResponse({"value": items, "@odata.deltaLink": delta_link})

        routes = [
            Route("/v1.0/organization", organization),
            Route("/v1.0/users", list_users),
            Route("/v1.0/users/{uid}/drive/root/delta", user_drive_delta),
            Route("/v1.0/drives/{did}/items/{iid}/content", drive_item_content),
            Route(
                "/v1.0/users/{uid}/mailFolders/inbox/messages/delta",
                mail_delta,
            ),
            Route("/v1.0/users/{uid}/calendarView/delta", calendar_delta),
            Route("/v1.0/sites", list_sites),
            Route("/v1.0/sites/{sid}/drive/root/delta", site_drive_delta),
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
def mock_graph_api() -> MockGraphAPI:
    return MockGraphAPI()


@pytest.fixture(scope="session")
def mock_graph_server(mock_graph_api: MockGraphAPI) -> str:
    """Start mock Graph API server in a daemon thread. Returns base URL."""
    port = _free_port()
    base_url = f"http://localhost:{port}"
    app = mock_graph_api.create_app(base_url)
    config = uvicorn.Config(app, host="0.0.0.0", port=port, log_level="warning")
    server = uvicorn.Server(config)

    thread = threading.Thread(target=server.run, daemon=True)
    thread.start()

    _wait_for_port(port)
    return base_url


@pytest.fixture(scope="session")
def connector_port() -> int:
    return _free_port()


@pytest.fixture(scope="session")
def connector_server(connector_port: int) -> str:
    """Start the Microsoft connector as a uvicorn server in a daemon thread."""
    import os

    os.environ.setdefault("CONNECTOR_MANAGER_URL", "http://localhost:0")

    from ms_connector import MicrosoftConnector
    from omni_connector.server import create_app

    app = create_app(MicrosoftConnector())
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
            "CONNECTOR_MICROSOFT_URL": f"http://host.docker.internal:{connector_port}",
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
    mock_graph_server: str,
    mock_graph_api: MockGraphAPI,
) -> str:
    """Create a Microsoft source with credentials pointing to the mock server."""
    mock_graph_api.reset()
    sid = await seed.create_source(
        source_type="microsoft",
        config={
            "graph_base_url": f"{mock_graph_server}/v1.0",
            "services": ["onedrive", "mail", "calendar", "sharepoint"],
        },
    )
    await seed.create_credentials(sid, {"token": "test-token"}, provider="microsoft")
    return sid


@pytest_asyncio.fixture
async def cm_client(harness: OmniTestHarness) -> httpx.AsyncClient:
    """Async httpx client pointed at the connector-manager."""
    async with httpx.AsyncClient(
        base_url=harness.connector_manager_url, timeout=30
    ) as client:
        yield client

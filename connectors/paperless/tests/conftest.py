"""Integration test fixtures for the Paperless-ngx connector.

Session-scoped: harness, mock paperless-ngx API server, connector server, connector-manager.
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
# Mock paperless-ngx API
# ---------------------------------------------------------------------------

MOCK_TAGS: list[dict[str, Any]] = [
    {"id": 1, "name": "invoice"},
    {"id": 2, "name": "receipt"},
]

MOCK_CORRESPONDENTS: list[dict[str, Any]] = [
    {"id": 1, "name": "ACME Corp"},
]

MOCK_DOCUMENT_TYPES: list[dict[str, Any]] = [
    {"id": 1, "name": "Invoice"},
]

MOCK_STORAGE_PATHS: list[dict[str, Any]] = []

MOCK_CUSTOM_FIELDS: list[dict[str, Any]] = [
    {"id": 1, "name": "Project"},
]


class MockPaperlessAPI:
    """Controllable mock of the paperless-ngx REST API."""

    def __init__(self) -> None:
        self.documents: list[dict[str, Any]] = []
        self.should_fail_auth: bool = False

    def reset(self) -> None:
        self.documents.clear()
        self.should_fail_auth = False

    def add_document(
        self,
        doc_id: int,
        title: str,
        content: str = "OCR text",
        correspondent: int | None = None,
        document_type: int | None = None,
        tags: list[int] | None = None,
        custom_fields: list[dict[str, Any]] | None = None,
        notes: list[dict[str, Any]] | None = None,
    ) -> None:
        self.documents.append({
            "id": doc_id,
            "title": title,
            "content": content,
            "created": "2024-06-01T10:00:00Z",
            "added": "2024-06-01T10:00:00Z",
            "modified": "2024-06-02T12:00:00Z",
            "original_file_name": f"doc_{doc_id}.pdf",
            "correspondent": correspondent,
            "document_type": document_type,
            "tags": tags or [],
            "storage_path": None,
            "archive_serial_number": doc_id,
            "custom_fields": custom_fields or [],
            "notes": notes or [],
        })

    def create_app(self) -> Starlette:
        mock = self

        def _check_auth(request: Request) -> JSONResponse | None:
            if mock.should_fail_auth:
                return JSONResponse({"detail": "Invalid token."}, status_code=401)
            auth = request.headers.get("Authorization", "")
            if not auth.startswith("Token "):
                return JSONResponse({"detail": "Invalid token."}, status_code=401)
            return None

        def _paginated(items: list[dict[str, Any]], request: Request) -> JSONResponse:
            return JSONResponse({
                "count": len(items),
                "next": None,
                "previous": None,
                "results": items,
            })

        async def api_root(request: Request) -> JSONResponse:
            err = _check_auth(request)
            if err:
                return err
            return JSONResponse({"documents": "/api/documents/"})

        async def list_documents(request: Request) -> JSONResponse:
            err = _check_auth(request)
            if err:
                return err
            return _paginated(mock.documents, request)

        async def list_tags(request: Request) -> JSONResponse:
            err = _check_auth(request)
            if err:
                return err
            return _paginated(MOCK_TAGS, request)

        async def list_correspondents(request: Request) -> JSONResponse:
            err = _check_auth(request)
            if err:
                return err
            return _paginated(MOCK_CORRESPONDENTS, request)

        async def list_document_types(request: Request) -> JSONResponse:
            err = _check_auth(request)
            if err:
                return err
            return _paginated(MOCK_DOCUMENT_TYPES, request)

        async def list_storage_paths(request: Request) -> JSONResponse:
            err = _check_auth(request)
            if err:
                return err
            return _paginated(MOCK_STORAGE_PATHS, request)

        async def list_custom_fields(request: Request) -> JSONResponse:
            err = _check_auth(request)
            if err:
                return err
            return _paginated(MOCK_CUSTOM_FIELDS, request)

        routes = [
            Route("/api/", api_root),
            Route("/api/documents/", list_documents),
            Route("/api/tags/", list_tags),
            Route("/api/correspondents/", list_correspondents),
            Route("/api/document_types/", list_document_types),
            Route("/api/storage_paths/", list_storage_paths),
            Route("/api/custom_fields/", list_custom_fields),
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
def mock_paperless_api() -> MockPaperlessAPI:
    return MockPaperlessAPI()


@pytest.fixture(scope="session")
def mock_paperless_server(mock_paperless_api: MockPaperlessAPI) -> str:
    """Start mock paperless-ngx API server in a daemon thread. Returns base URL."""
    port = _free_port()
    app = mock_paperless_api.create_app()
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
    """Start the Paperless connector as a uvicorn server in a daemon thread."""
    import os

    os.environ.setdefault("CONNECTOR_MANAGER_URL", "http://localhost:0")

    from paperless_connector import PaperlessConnector
    from omni_connector.server import create_app

    app = create_app(PaperlessConnector())
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
            "PAPERLESS_CONNECTOR_URL": f"http://host.docker.internal:{connector_port}",
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
    mock_paperless_server: str,
    mock_paperless_api: MockPaperlessAPI,
) -> str:
    """Create a paperless-ngx source with credentials pointing to the mock server."""
    mock_paperless_api.reset()
    sid = await seed.create_source(
        source_type="paperless_ngx",
        config={"base_url": mock_paperless_server},
    )
    await seed.create_credentials(
        sid, {"api_key": "test-token"}, provider="paperless_ngx"
    )
    return sid


@pytest_asyncio.fixture
async def cm_client(harness: OmniTestHarness) -> httpx.AsyncClient:
    """Async httpx client pointed at the connector-manager."""
    async with httpx.AsyncClient(
        base_url=harness.connector_manager_url, timeout=30
    ) as client:
        yield client

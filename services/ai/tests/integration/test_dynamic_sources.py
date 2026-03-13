"""Integration tests for dynamic source types in search tool definition and system prompt.

Validates the full flow:
- Real ParadeDB (testcontainers) with sources inserted via SQL
- Real connector-manager container serving /sources from that DB
- The AI service builds correct tool definitions and system prompts

Uses existing db_pool/redis fixtures from conftest.py.
"""

import subprocess
import time
from pathlib import Path
from unittest.mock import AsyncMock

import httpx
import pytest
from fastapi import FastAPI
from starlette.requests import Request
from testcontainers.core.container import DockerContainer
from testcontainers.core.waiting_utils import wait_for_logs
from ulid import ULID

from db import UsersRepository
from db.models import Chat, Source
import db.connection
from prompts import build_chat_system_prompt
from routers.chat import _build_registry
from tools.search_handler import _build_search_tools

pytestmark = pytest.mark.integration

REPO_ROOT = (
    Path(__file__).resolve().parents[4]
)  # services/ai/tests/integration -> repo root
CM_IMAGE_TAG = "omni-connector-manager:test"


# ---------------------------------------------------------------------------
# Session-scoped fixtures: build image, start connector-manager container
# ---------------------------------------------------------------------------


@pytest.fixture(scope="session")
def connector_manager_image():
    """Build the connector-manager Docker image once per session."""
    result = subprocess.run(
        [
            "docker",
            "build",
            "-f",
            "services/connector-manager/Dockerfile",
            "-t",
            CM_IMAGE_TAG,
            ".",
        ],
        cwd=str(REPO_ROOT),
        capture_output=True,
        text=True,
        timeout=600,
    )
    if result.returncode != 0:
        pytest.skip(f"Failed to build connector-manager image: {result.stderr[-500:]}")
    return CM_IMAGE_TAG


@pytest.fixture(scope="session")
def connector_manager_container(
    connector_manager_image, initialized_db, redis_container
):
    """Start a real connector-manager container pointing at the test DB and Redis."""
    pg_container = (
        initialized_db  # initialized_db yields the postgres_container after migrations
    )

    # Get host-mapped ports for Postgres and Redis
    pg_host = pg_container.get_container_host_ip()
    pg_port = pg_container.get_exposed_port(5432)
    redis_host = redis_container.get_container_host_ip()
    redis_port = redis_container.get_exposed_port(6379)

    # From inside the connector-manager container, we reach the host via host.docker.internal
    # (added via extra_hosts). The host-mapped ports from testcontainers are on the host.
    container = (
        DockerContainer(connector_manager_image)
        .with_exposed_ports(8090)
        .with_env("DATABASE_HOST", "host.docker.internal")
        .with_env("DATABASE_PORT", str(pg_port))
        .with_env("DATABASE_USERNAME", "test")
        .with_env("DATABASE_PASSWORD", "test")
        .with_env("DATABASE_NAME", "test")
        .with_env("REDIS_URL", f"redis://host.docker.internal:{redis_port}")
        .with_env("PORT", "8090")
    )
    # Allow container to reach host-mapped ports via host.docker.internal
    container._kwargs = {"extra_hosts": {"host.docker.internal": "host-gateway"}}

    with container:
        # Wait for the connector-manager to be healthy
        wait_for_logs(container, "listening on", timeout=30)
        time.sleep(1)

        # Verify health endpoint
        cm_host = container.get_container_host_ip()
        cm_port = container.get_exposed_port(8090)
        cm_url = f"http://{cm_host}:{cm_port}"

        for attempt in range(10):
            try:
                resp = httpx.get(f"{cm_url}/health", timeout=3.0)
                if resp.status_code == 200:
                    break
            except Exception:
                pass
            time.sleep(1)
        else:
            logs = container.get_logs()
            pytest.fail(f"Connector-manager failed to become healthy. Logs: {logs}")

        yield container


@pytest.fixture(scope="session")
def connector_manager_url(connector_manager_container):
    """Return the base URL of the running connector-manager."""
    host = connector_manager_container.get_container_host_ip()
    port = connector_manager_container.get_exposed_port(8090)
    return f"http://{host}:{port}"


# ---------------------------------------------------------------------------
# Per-test fixtures
# ---------------------------------------------------------------------------


@pytest.fixture
async def test_user(db_pool) -> str:
    """Create a test user, return user ID."""
    users_repo = UsersRepository(pool=db_pool)
    user = await users_repo.create(
        email=f"{ULID()}@test.local",
        password_hash="not-a-real-hash",
        full_name="Test User",
    )
    return user.id


@pytest.fixture
def _patch_db_pool(db_pool, monkeypatch):
    """Point the global _db_pool at the test pool."""
    monkeypatch.setattr(db.connection, "_db_pool", db_pool)


async def _insert_source(
    conn,
    *,
    source_id: str,
    name: str,
    source_type: str,
    is_active: bool,
    created_by: str,
    is_deleted: bool = False,
):
    """Insert a source row into the real test database."""
    await conn.execute(
        """INSERT INTO sources (id, name, source_type, config, is_active, is_deleted, created_by)
           VALUES ($1, $2, $3, '{}', $4, $5, $6)""",
        source_id,
        name,
        source_type,
        is_active,
        is_deleted,
        created_by,
    )


async def _cleanup_sources(conn, created_by: str):
    """Remove sources created by a specific user (test isolation)."""
    await conn.execute("DELETE FROM sources WHERE created_by = $1", created_by)


def _make_request(app: FastAPI) -> Request:
    """Create a minimal Starlette Request bound to the given app."""
    scope = {"type": "http", "app": app}
    return Request(scope)


def _make_app() -> FastAPI:
    """Create a minimal FastAPI app with mocked state for _build_registry."""
    app = FastAPI()
    app.state = type(
        "State",
        (),
        {
            "searcher_tool": AsyncMock(),
            "models": {},
            "default_model_id": None,
            "content_storage": None,
            "redis_client": None,
        },
    )()
    return app


def _make_chat(user_id: str) -> Chat:
    return Chat(
        id=str(ULID()),
        user_id=user_id,
        model_id=None,
        title=None,
        created_at=None,
        updated_at=None,
    )


# ---------------------------------------------------------------------------
# Tests: _build_search_tools (unit-level, no DB needed)
# ---------------------------------------------------------------------------


def test_search_tool_has_query_with_operators():
    """Search tool query description includes universal operators."""
    tools = _build_search_tools()
    query_prop = tools[0]["input_schema"]["properties"]["query"]
    assert "in:<source>" in query_prop["description"]
    assert "type:<type>" in query_prop["description"]
    assert "before:<date>" in query_prop["description"]


def test_search_tool_includes_connector_operators():
    """Search tool query description includes connector-specific operators."""
    operators = [
        {
            "operator": "status",
            "attribute_key": "status",
            "value_type": "text",
            "source_type": "jira",
        },
        {
            "operator": "channel",
            "attribute_key": "channel_name",
            "value_type": "text",
            "source_type": "slack",
        },
    ]
    tools = _build_search_tools(operators)
    query_desc = tools[0]["input_schema"]["properties"]["query"]["description"]
    assert "Jira" in query_desc
    assert "status:<value>" in query_desc
    assert "Slack" in query_desc
    assert "channel:<value>" in query_desc


def test_search_tool_no_sources_or_attributes_params():
    """Search tool should not have sources, content_types, or attributes params."""
    tools = _build_search_tools()
    properties = tools[0]["input_schema"]["properties"]
    assert "sources" not in properties
    assert "content_types" not in properties
    assert "attributes" not in properties


# ---------------------------------------------------------------------------
# Tests: build_chat_system_prompt
# ---------------------------------------------------------------------------


def test_system_prompt_lists_only_active_sources():
    """System prompt Connected apps should list only active, non-deleted sources."""
    sources = [
        Source(
            id="1",
            name="Drive",
            source_type="google_drive",
            is_active=True,
            is_deleted=False,
        ),
        Source(
            id="2", name="Slack", source_type="slack", is_active=False, is_deleted=False
        ),
        Source(
            id="3", name="Jira", source_type="jira", is_active=True, is_deleted=False
        ),
    ]
    active_sources = [s for s in sources if s.is_active and not s.is_deleted]
    prompt = build_chat_system_prompt(active_sources)
    assert "Google Drive" in prompt
    assert "Jira" in prompt
    assert "Slack" not in prompt


def test_system_prompt_no_sources():
    """When no active sources, Connected apps should say None."""
    prompt = build_chat_system_prompt([])
    assert "Connected apps: None" in prompt


def test_system_prompt_deduplicates_source_types():
    """Multiple sources of the same type should appear once in Connected apps."""
    sources = [
        Source(
            id="1",
            name="Drive 1",
            source_type="google_drive",
            is_active=True,
            is_deleted=False,
        ),
        Source(
            id="2",
            name="Drive 2",
            source_type="google_drive",
            is_active=True,
            is_deleted=False,
        ),
    ]
    prompt = build_chat_system_prompt(sources)
    # Extract the Connected apps line and verify deduplication there
    connected_line = [
        l for l in prompt.splitlines() if l.startswith("Connected apps:")
    ][0]
    assert connected_line.count("Google Drive") == 1


# ---------------------------------------------------------------------------
# Tests: Full flow — real DB + real connector-manager container
# ---------------------------------------------------------------------------


@pytest.mark.asyncio
async def test_build_registry_search_tool_has_dynamic_sources(
    db_pool,
    test_user,
    _patch_db_pool,
    connector_manager_url,
    monkeypatch,
):
    """_build_registry fetches sources from real connector-manager and builds correct tool def."""
    monkeypatch.setattr("routers.chat.CONNECTOR_MANAGER_URL", connector_manager_url)
    monkeypatch.setattr("routers.chat.SANDBOX_URL", "")

    # Insert sources into real DB (connector-manager reads from same DB)
    source_ids = [str(ULID()) for _ in range(3)]
    async with db_pool.acquire() as conn:
        await _insert_source(
            conn,
            source_id=source_ids[0],
            name="My Drive",
            source_type="google_drive",
            is_active=True,
            created_by=test_user,
        )
        await _insert_source(
            conn,
            source_id=source_ids[1],
            name="Team Slack",
            source_type="slack",
            is_active=True,
            created_by=test_user,
        )
        await _insert_source(
            conn,
            source_id=source_ids[2],
            name="Old Confluence",
            source_type="confluence",
            is_active=False,
            created_by=test_user,
        )

    try:
        app = _make_app()
        request = _make_request(app)
        chat = _make_chat(test_user)

        result = await _build_registry(request, chat)

        # Verify search tool has query with operator syntax
        search_tools = result.registry.get_all_tools()
        search_tool = next(t for t in search_tools if t["name"] == "search_documents")
        query_desc = search_tool["input_schema"]["properties"]["query"]["description"]
        assert "in:<source>" in query_desc

        # Verify system prompt only includes active sources
        active_sources = [
            s for s in (result.sources or []) if s.is_active and not s.is_deleted
        ]
        prompt = build_chat_system_prompt(active_sources)
        assert "Google Drive" in prompt
        assert "Slack" in prompt
        assert "Confluence" not in prompt
    finally:
        async with db_pool.acquire() as conn:
            await _cleanup_sources(conn, test_user)


@pytest.mark.asyncio
async def test_build_registry_no_sources_generic_description(
    db_pool,
    test_user,
    _patch_db_pool,
    connector_manager_url,
    monkeypatch,
):
    """When no sources exist in DB, search tool has generic description."""
    monkeypatch.setattr("routers.chat.CONNECTOR_MANAGER_URL", connector_manager_url)
    monkeypatch.setattr("routers.chat.SANDBOX_URL", "")

    # Ensure no sources for this user
    async with db_pool.acquire() as conn:
        await _cleanup_sources(conn, test_user)

    app = _make_app()
    request = _make_request(app)
    chat = _make_chat(test_user)

    result = await _build_registry(request, chat)

    search_tools = result.registry.get_all_tools()
    search_tool = next(t for t in search_tools if t["name"] == "search_documents")
    query_desc = search_tool["input_schema"]["properties"]["query"]["description"]
    assert "in:<source>" in query_desc

    active_sources = [
        s for s in (result.sources or []) if s.is_active and not s.is_deleted
    ]
    prompt = build_chat_system_prompt(active_sources)
    assert "Connected apps: None" in prompt


@pytest.mark.asyncio
async def test_build_registry_deleted_sources_excluded(
    db_pool,
    test_user,
    _patch_db_pool,
    connector_manager_url,
    monkeypatch,
):
    """Deleted sources should not appear in tool definition or system prompt."""
    monkeypatch.setattr("routers.chat.CONNECTOR_MANAGER_URL", connector_manager_url)
    monkeypatch.setattr("routers.chat.SANDBOX_URL", "")

    source_ids = [str(ULID()) for _ in range(2)]
    async with db_pool.acquire() as conn:
        await _insert_source(
            conn,
            source_id=source_ids[0],
            name="Active Drive",
            source_type="google_drive",
            is_active=True,
            created_by=test_user,
        )
        await _insert_source(
            conn,
            source_id=source_ids[1],
            name="Deleted Jira",
            source_type="jira",
            is_active=True,
            is_deleted=True,
            created_by=test_user,
        )

    try:
        app = _make_app()
        request = _make_request(app)
        chat = _make_chat(test_user)

        result = await _build_registry(request, chat)

        search_tools = result.registry.get_all_tools()
        search_tool = next(t for t in search_tools if t["name"] == "search_documents")
        query_desc = search_tool["input_schema"]["properties"]["query"]["description"]
        assert "in:<source>" in query_desc

        # System prompt: only active non-deleted
        active_sources = [
            s for s in (result.sources or []) if s.is_active and not s.is_deleted
        ]
        prompt = build_chat_system_prompt(active_sources)
        assert "Google Drive" in prompt
        assert "Jira" not in prompt
    finally:
        async with db_pool.acquire() as conn:
            await _cleanup_sources(conn, test_user)

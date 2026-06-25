"""Integration test for the background agent end-to-end flow.

Tests the full lifecycle: agent creation → scheduler finds due agent →
executor runs the agent loop with mocked LLM → results written to agent_runs
table with execution log and summary.

Also tests permission scoping with a real searcher: personal agents only see
their user's documents, org agents see all documents.

Uses real Postgres (testcontainers), real searcher container, mocked LLM.
"""

import json
import subprocess
import time
from datetime import datetime, timezone
from pathlib import Path
from unittest.mock import AsyncMock

import httpx
import pytest
from ulid import ULID
from testcontainers.core.container import DockerContainer
from testcontainers.core.waiting_utils import wait_for_logs

import db.connection
from agents.executor import execute_agent
from agents.repository import AgentRepository, AgentRunRepository
from state import AppState
from tools.searcher_tool import SearcherTool
from tests.helpers import (
    create_test_user,
    create_test_source,
    create_test_document,
    create_mock_llm_multi,
)

pytestmark = pytest.mark.integration

REPO_ROOT = Path(__file__).resolve().parents[4]
SEARCHER_IMAGE_TAG = "omni-searcher:test"

SUMMARY_TEXT = "Searched for recent documents and found results."


# ---------------------------------------------------------------------------
# Searcher container fixtures (session-scoped — build once, reuse)
# ---------------------------------------------------------------------------


@pytest.fixture(scope="session")
def searcher_image():
    """Build the searcher Docker image once per session."""
    result = subprocess.run(
        [
            "docker",
            "build",
            "-f",
            "services/searcher/Dockerfile",
            "-t",
            SEARCHER_IMAGE_TAG,
            ".",
        ],
        cwd=str(REPO_ROOT),
        capture_output=True,
        text=True,
        timeout=900,
    )
    if result.returncode != 0:
        pytest.skip(f"Failed to build searcher image: {result.stderr[-500:]}")
    return SEARCHER_IMAGE_TAG


@pytest.fixture(scope="session")
def searcher_container(searcher_image, initialized_db, redis_container):
    """Start a real searcher container pointing at the test DB and Redis."""
    pg_container = initialized_db
    pg_host = pg_container.get_container_host_ip()
    pg_port = pg_container.get_exposed_port(5432)
    redis_host = redis_container.get_container_host_ip()
    redis_port = redis_container.get_exposed_port(6379)

    container = (
        DockerContainer(searcher_image)
        .with_exposed_ports(8002)
        .with_env("DATABASE_HOST", "host.docker.internal")
        .with_env("DATABASE_PORT", str(pg_port))
        .with_env("DATABASE_USERNAME", "test")
        .with_env("DATABASE_PASSWORD", "test")
        .with_env("DATABASE_NAME", "test")
        .with_env("REDIS_URL", f"redis://host.docker.internal:{redis_port}")
        .with_env("PORT", "8002")
        .with_env("AI_SERVICE_URL", "http://localhost:9999")
    )
    container._kwargs = {"extra_hosts": {"host.docker.internal": "host-gateway"}}

    with container:
        wait_for_logs(container, "listening on", timeout=30)
        time.sleep(1)

        host = container.get_container_host_ip()
        port = container.get_exposed_port(8002)
        url = f"http://{host}:{port}"

        for attempt in range(10):
            try:
                resp = httpx.get(f"{url}/health", timeout=3.0)
                if resp.status_code == 200:
                    break
            except Exception:
                pass
            time.sleep(1)
        else:
            logs = container.get_logs()
            pytest.fail(f"Searcher failed to become healthy. Logs: {logs}")

        yield container


@pytest.fixture(scope="session")
def searcher_url(searcher_container):
    host = searcher_container.get_container_host_ip()
    port = searcher_container.get_exposed_port(8002)
    return f"http://{host}:{port}"


# ---------------------------------------------------------------------------
# Agent-specific helpers
# ---------------------------------------------------------------------------


async def create_test_agent(db_pool, user_id: str, agent_type: str = "user") -> str:
    agent_id = str(ULID())
    async with db_pool.acquire() as conn:
        await conn.execute(
            """INSERT INTO agents (id, user_id, name, instructions, agent_type,
                                   schedule_type, schedule_value,
                                   allowed_sources, allowed_actions,
                                   is_enabled, is_deleted,
                                   created_at, updated_at)
               VALUES ($1, $2, $3, $4, $5, 'interval', '60',
                       '[]'::jsonb, '[]'::jsonb,
                       true, false,
                       NOW() - INTERVAL '2 minutes', NOW() - INTERVAL '2 minutes')""",
            agent_id,
            user_id,
            f"Test {agent_type} Agent",
            "Search for recent documents and summarize the findings.",
            agent_type,
        )
    return agent_id


# ---------------------------------------------------------------------------
# Fixtures
# ---------------------------------------------------------------------------


@pytest.fixture
def _patch_db_pool(db_pool, monkeypatch):
    """Point the global _db_pool at the test pool so repositories work."""
    monkeypatch.setattr(db.connection, "_db_pool", db_pool)


@pytest.fixture
def _patch_env(monkeypatch):
    """Disable connector manager and sandbox to simplify the test."""
    monkeypatch.setenv("CONNECTOR_MANAGER_URL", "")
    monkeypatch.setenv("SANDBOX_URL", "")


def _build_app_state(mock_llm, searcher_tool) -> AppState:
    app_state = AppState()
    app_state.models = {"mock-model": mock_llm}
    app_state.default_model_id = "mock-model"
    app_state.searcher_tool = searcher_tool
    app_state.content_storage = AsyncMock()
    app_state.redis_client = None
    return app_state


# ---------------------------------------------------------------------------
# Tests
# ---------------------------------------------------------------------------


@pytest.mark.asyncio
async def test_background_agent_end_to_end(db_pool, _patch_db_pool, _patch_env):
    """Full lifecycle: create agent -> find due -> execute -> verify DB results -> no longer due."""

    user_id, _ = await create_test_user(db_pool)
    await create_test_source(db_pool, user_id)
    agent_id = await create_test_agent(db_pool, user_id)

    agent_repo = AgentRepository(pool=db_pool)
    run_repo = AgentRunRepository(pool=db_pool)

    # Scheduler finds due agent
    now = datetime.now(timezone.utc)
    due_agents = await agent_repo.find_due_agents(now)
    assert agent_id in [a.id for a in due_agents]

    agent = await agent_repo.get_agent(agent_id)
    assert agent is not None

    mock_searcher = AsyncMock()
    mock_searcher.handle.return_value = AsyncMock(
        results=[], total_count=0, query_time_ms=1
    )

    mock_llm = create_mock_llm_multi(
        [
            ("tool_call", {"query": "recent documents"}),
            ("text", "Here are the results from my search."),
            ("text", SUMMARY_TEXT),
        ]
    )

    app_state = _build_app_state(mock_llm, mock_searcher)
    run = await execute_agent(agent, app_state)

    assert run.status == "completed"
    assert run.started_at is not None
    assert run.completed_at is not None
    assert run.error_message is None
    run_logs = await run_repo.list_run_logs(run.id)
    assert len(run_logs) >= 3
    assert run.summary is not None
    assert SUMMARY_TEXT in run.summary

    db_run = await run_repo.get_run(run.id)
    assert db_run is not None
    assert db_run.status == "completed"

    # Agent is no longer due
    due_agents_after = await agent_repo.find_due_agents(now)
    assert agent_id not in [a.id for a in due_agents_after]


@pytest.mark.asyncio
async def test_personal_agent_search_scoped_to_user(
    db_pool, _patch_db_pool, _patch_env, searcher_url, monkeypatch
):
    """Personal agent only sees documents the owning user has permission to access."""
    monkeypatch.setenv("SEARCHER_URL", searcher_url)

    user_a_id, user_a_email = await create_test_user(db_pool, "user-a")
    user_b_id, user_b_email = await create_test_user(db_pool, "user-b")
    source_id = await create_test_source(db_pool, user_a_id)

    await create_test_document(
        db_pool,
        source_id,
        "Quarterly Report for User A",
        "This quarterly report contains important financial data for user A.",
        {"users": [user_a_email]},
    )
    await create_test_document(
        db_pool,
        source_id,
        "Secret Plan for User B",
        "This secret plan is only visible to user B and should not leak.",
        {"users": [user_b_email]},
    )

    agent_id = await create_test_agent(db_pool, user_a_id, "user")
    agent = await AgentRepository(pool=db_pool).get_agent(agent_id)

    mock_llm = create_mock_llm_multi(
        [
            ("tool_call", {"query": "quarterly report secret plan"}),
            ("text", "Here are the results from my search."),
            ("text", SUMMARY_TEXT),
        ]
    )

    app_state = _build_app_state(mock_llm, SearcherTool())
    run = await execute_agent(agent, app_state)
    assert run.status == "completed"

    run_logs = await AgentRunRepository(pool=db_pool).list_run_logs(run.id)
    log_text = json.dumps([log.message for log in run_logs])
    assert (
        "Quarterly Report for User A" in log_text
    ), "Personal agent should see user A's document"
    assert (
        "Secret Plan for User B" not in log_text
    ), "Personal agent should NOT see user B's document"


@pytest.mark.asyncio
async def test_org_agent_search_sees_all_documents(
    db_pool, _patch_db_pool, _patch_env, searcher_url, monkeypatch
):
    """Org agent sees all documents regardless of per-user permissions."""
    monkeypatch.setenv("SEARCHER_URL", searcher_url)

    admin_id, admin_email = await create_test_user(db_pool, "admin")
    other_id, other_email = await create_test_user(db_pool, "other")
    source_id = await create_test_source(db_pool, admin_id)

    await create_test_document(
        db_pool,
        source_id,
        "Admin Visible Report",
        "This report is visible to the admin user only.",
        {"users": [admin_email]},
    )
    await create_test_document(
        db_pool,
        source_id,
        "Other User Private Doc",
        "This document belongs to the other user and is private to them.",
        {"users": [other_email]},
    )

    agent_id = await create_test_agent(db_pool, admin_id, "org")
    agent = await AgentRepository(pool=db_pool).get_agent(agent_id)

    mock_llm = create_mock_llm_multi(
        [
            ("tool_call", {"query": "admin visible report other user private doc"}),
            ("text", "Here are the results from my search."),
            ("text", SUMMARY_TEXT),
        ]
    )

    app_state = _build_app_state(mock_llm, SearcherTool())
    run = await execute_agent(agent, app_state)
    assert run.status == "completed"

    run_logs = await AgentRunRepository(pool=db_pool).list_run_logs(run.id)
    log_text = json.dumps([log.message for log in run_logs])
    assert "Admin Visible Report" in log_text, "Org agent should see admin's document"
    assert (
        "Other User Private Doc" in log_text
    ), "Org agent should see other user's document (no permission scoping)"

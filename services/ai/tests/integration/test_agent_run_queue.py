"""Integration tests for durable agent_runs queue semantics."""

from __future__ import annotations

import asyncio
from datetime import UTC, datetime, timedelta
from unittest.mock import AsyncMock

import pytest
from anthropic.types import MessageParam, ToolUseBlockParam
from ulid import ULID

import db.connection
from agents.models import AgentRunAlreadyActive, AgentRunRetryPolicy, AgentRunTriggerType
from agents.queue_worker import _execute_claimed_run
from agents.repository import AgentRepository, AgentRunRepository
from agents.scheduler import materialize_due_agent_runs
from state import AppState
from tests.helpers import create_mock_llm_multi, create_test_user

pytestmark = pytest.mark.integration


@pytest.fixture
async def _patch_db_pool(db_pool, monkeypatch):
    monkeypatch.setattr(db.connection, "_db_pool", db_pool)
    async with db_pool.acquire() as conn:
        await conn.execute("DELETE FROM agent_run_logs")
        await conn.execute("DELETE FROM agent_runs")
        await conn.execute("DELETE FROM agents")


async def _create_agent(db_pool, user_id: str) -> str:
    agent_id = str(ULID())
    async with db_pool.acquire() as conn:
        await conn.execute(
            """INSERT INTO agents (id, user_id, name, instructions, agent_type,
                                   schedule_type, schedule_value,
                                   allowed_sources, allowed_actions,
                                   is_enabled, is_deleted,
                                   created_at, updated_at)
               VALUES ($1, $2, 'Queue Test Agent', 'Do queue test work', 'user',
                       'interval', '60', '[]'::jsonb, '[]'::jsonb,
                       true, false, NOW() - INTERVAL '2 minutes', NOW() - INTERVAL '2 minutes')""",
            agent_id,
            user_id,
        )
    return agent_id


def _policy(max_attempts: int = 3) -> AgentRunRetryPolicy:
    return AgentRunRetryPolicy(
        max_attempts=max_attempts,
        backoff_delays=(timedelta(seconds=0), timedelta(seconds=0), timedelta(seconds=0)),
    )


def _app_state_with_llm(mock_llm) -> AppState:
    app_state = AppState()
    app_state.models = {"mock-model": mock_llm}
    app_state.default_model_id = "mock-model"
    app_state.searcher_tool = AsyncMock()
    app_state.content_storage = AsyncMock()
    app_state.redis_client = None
    return app_state


@pytest.mark.asyncio
async def test_create_run_if_idle_returns_conflict_for_active_run(db_pool, _patch_db_pool):
    user_id, _ = await create_test_user(db_pool)
    agent_id = await _create_agent(db_pool, user_id)
    repo = AgentRunRepository(pool=db_pool)

    first = await repo.create_run(agent_id, AgentRunTriggerType.MANUAL)
    assert not isinstance(first, AgentRunAlreadyActive)
    assert first.status == "pending"

    second = await repo.create_run(agent_id, AgentRunTriggerType.MANUAL)
    assert isinstance(second, AgentRunAlreadyActive)
    assert second.run.id == first.id


@pytest.mark.asyncio
async def test_concurrent_claimers_do_not_claim_same_run(db_pool, _patch_db_pool):
    user_id, _ = await create_test_user(db_pool)
    agent_id = await _create_agent(db_pool, user_id)
    repo = AgentRunRepository(pool=db_pool)
    await repo.create_run(agent_id, AgentRunTriggerType.MANUAL)

    results = await asyncio.gather(
        repo.claim_next_run(10, timedelta(minutes=5), _policy()),
        repo.claim_next_run(10, timedelta(minutes=5), _policy()),
    )
    claimed = [result for result in results if result is not None]
    assert len(claimed) == 1
    assert claimed[0].run.status == "running"
    assert claimed[0].run.claim_token == claimed[0].claim_token
    assert claimed[0].run.attempt_count == 1


@pytest.mark.asyncio
async def test_cluster_concurrency_cap_blocks_claims(db_pool, _patch_db_pool):
    user_id, _ = await create_test_user(db_pool)
    agent_a = await _create_agent(db_pool, user_id)
    agent_b = await _create_agent(db_pool, user_id)
    repo = AgentRunRepository(pool=db_pool)
    await repo.create_run(agent_a, AgentRunTriggerType.MANUAL)
    await repo.create_run(agent_b, AgentRunTriggerType.MANUAL)

    first = await repo.claim_next_run(1, timedelta(minutes=5), _policy())
    assert first is not None
    second = await repo.claim_next_run(1, timedelta(minutes=5), _policy())
    assert second is None


@pytest.mark.asyncio
async def test_heartbeat_and_completion_are_fenced_by_claim_token(db_pool, _patch_db_pool):
    user_id, _ = await create_test_user(db_pool)
    agent_id = await _create_agent(db_pool, user_id)
    repo = AgentRunRepository(pool=db_pool)
    await repo.create_run(agent_id, AgentRunTriggerType.MANUAL)
    claim = await repo.claim_next_run(10, timedelta(minutes=5), _policy())
    assert claim is not None

    assert not await repo.heartbeat_run(claim.run.id, "0" * 26, timedelta(minutes=5))
    assert await repo.heartbeat_run(claim.run.id, claim.claim_token, timedelta(minutes=5))

    assert await repo.complete_run(claim.run.id, "0" * 26, "bad") is None
    completed = await repo.complete_run(claim.run.id, claim.claim_token, "done")
    assert completed is not None
    assert completed.status == "completed"


@pytest.mark.asyncio
async def test_stale_lease_recovery_and_old_token_cannot_complete(db_pool, _patch_db_pool):
    user_id, _ = await create_test_user(db_pool)
    agent_id = await _create_agent(db_pool, user_id)
    repo = AgentRunRepository(pool=db_pool)
    await repo.create_run(agent_id, AgentRunTriggerType.MANUAL)
    first = await repo.claim_next_run(10, timedelta(minutes=5), _policy())
    assert first is not None

    async with db_pool.acquire() as conn:
        await conn.execute(
            "UPDATE agent_runs SET lease_expires_at = NOW() - INTERVAL '1 second' WHERE id = $1",
            first.run.id,
        )

    assert await repo.recover_stale_runs() == 1
    second = await repo.claim_next_run(10, timedelta(minutes=5), _policy())
    assert second is not None
    assert second.run.id == first.run.id
    assert second.claim_token != first.claim_token

    assert await repo.complete_run(first.run.id, first.claim_token, "old") is None
    completed = await repo.complete_run(second.run.id, second.claim_token, "new")
    assert completed is not None
    assert completed.status == "completed"


@pytest.mark.asyncio
async def test_run_logs_are_fenced_and_ordered(db_pool, _patch_db_pool):
    user_id, _ = await create_test_user(db_pool)
    agent_id = await _create_agent(db_pool, user_id)
    repo = AgentRunRepository(pool=db_pool)
    await repo.create_run(agent_id, AgentRunTriggerType.MANUAL)
    claim = await repo.claim_next_run(10, timedelta(minutes=5), _policy())
    assert claim is not None

    tool_use = MessageParam(
        role="assistant",
        content=[ToolUseBlockParam(type="tool_use", id="toolu_1", name="search", input={})],
    )
    assert await repo.append_run_log_messages(claim.run.id, "0" * 26, [tool_use]) == []
    rows = await repo.append_run_log_messages(claim.run.id, claim.claim_token, [tool_use])
    assert len(rows) == 1
    assert rows[0].message_seq_num == 0

    logs = await repo.list_run_logs(claim.run.id)
    assert [log.message_seq_num for log in logs] == [0]
    assert logs[0].message["role"] == "assistant"


@pytest.mark.asyncio
async def test_scheduler_materializes_due_run_without_duplicate(db_pool, _patch_db_pool):
    user_id, _ = await create_test_user(db_pool)
    agent_id = await _create_agent(db_pool, user_id)
    agent_repo = AgentRepository(pool=db_pool)
    run_repo = AgentRunRepository(pool=db_pool)

    created = await materialize_due_agent_runs(
        agent_repo,
        run_repo,
        datetime.now(UTC),
    )
    assert created == 1
    runs = await run_repo.list_runs(agent_id)
    assert len(runs) == 1
    assert runs[0].status == "pending"
    assert runs[0].trigger_type == "scheduled"

    created_again = await materialize_due_agent_runs(
        agent_repo,
        run_repo,
        datetime.now(UTC),
    )
    assert created_again == 0
    assert len(await run_repo.list_runs(agent_id)) == 1


@pytest.mark.asyncio
async def test_queue_worker_executes_claimed_run_through_executor(db_pool, _patch_db_pool):
    user_id, _ = await create_test_user(db_pool)
    agent_id = await _create_agent(db_pool, user_id)
    run_repo = AgentRunRepository(pool=db_pool)
    await run_repo.create_run(agent_id, AgentRunTriggerType.MANUAL)
    claim = await run_repo.claim_next_run(10, timedelta(minutes=5), _policy())
    assert claim is not None

    mock_llm = create_mock_llm_multi([
        ("text", "The queued agent run completed."),
        ("text", "Queued run summary."),
    ])
    await _execute_claimed_run(_app_state_with_llm(mock_llm), claim, _policy())

    completed = await run_repo.get_run(claim.run.id)
    assert completed is not None
    assert completed.status == "completed"
    assert completed.summary == "Queued run summary."
    logs = await run_repo.list_run_logs(claim.run.id)
    assert [log.message["role"] for log in logs] == ["user", "assistant", "user"]


@pytest.mark.asyncio
async def test_executor_recovers_interrupted_tool_call_from_run_logs(db_pool, _patch_db_pool):
    user_id, _ = await create_test_user(db_pool)
    agent_id = await _create_agent(db_pool, user_id)
    run_repo = AgentRunRepository(pool=db_pool)
    await run_repo.create_run(agent_id, AgentRunTriggerType.MANUAL)
    claim = await run_repo.claim_next_run(10, timedelta(minutes=5), _policy())
    assert claim is not None

    initial = MessageParam(role="user", content="Execute your scheduled task now.")
    interrupted_tool_use = MessageParam(
        role="assistant",
        content=[
            ToolUseBlockParam(
                type="tool_use",
                id="toolu_interrupted",
                name="search_documents",
                input={"query": "quarterly report"},
            )
        ],
    )
    await run_repo.append_run_log_messages(
        claim.run.id, claim.claim_token, [initial, interrupted_tool_use]
    )

    mock_llm = create_mock_llm_multi([
        ("text", "I saw the interrupted tool call and reconciled it."),
        ("text", "Recovered interrupted run."),
    ])
    await _execute_claimed_run(_app_state_with_llm(mock_llm), claim, _policy())

    logs = await run_repo.list_run_logs(claim.run.id)
    log_text = str([log.message for log in logs])
    assert "previous attempt was interrupted" in log_text
    assert (await run_repo.get_run(claim.run.id)).summary == "Recovered interrupted run."

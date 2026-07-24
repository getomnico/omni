"""End-to-end integration tests for durable conversation compaction."""

from __future__ import annotations

from collections.abc import AsyncIterator, Sequence
from datetime import timedelta
from types import SimpleNamespace
from typing import Any
from unittest.mock import AsyncMock

import pytest
from anthropic import MessageStreamEvent
from anthropic.types import MessageParam
from fastapi import FastAPI
from httpx import ASGITransport, AsyncClient
from ulid import ULID

import db.connection
import services.compaction as compaction_module
from agents.executor import execute_claimed_agent
from agents.models import Agent, AgentRun, AgentRunLog, AgentRunRetryPolicy, AgentRunTriggerType
from agents.repository import AgentRunRepository
from db.compactions import CompactionsRepository
from db.messages import MessagesRepository
from providers import ContextWindowInfo, LLMProvider, ProviderType, TokenUsage
from routers import chat_router
from services.compaction import ConversationCompactor
from state import AppState
from tests.helpers import create_test_user, text_response_events

pytestmark = pytest.mark.integration


class CannedSummaryProvider(LLMProvider):
    provider_type = ProviderType.OPENAI

    def __init__(
        self,
        summaries: Sequence[str],
        *,
        context_tokens: int = 128_000,
        fail_on_generate: bool = False,
        model_record_id: str | None = None,
    ):
        self._summaries = list(summaries)
        self.context_tokens = context_tokens
        self.fail_on_generate = fail_on_generate
        self.prompts: list[str] = []
        self.model_record_id = model_record_id
        self.model_name = "canned-summary-model"

    async def get_context_window_tokens(self) -> ContextWindowInfo:
        return ContextWindowInfo(tokens=self.context_tokens, source="safe_default")

    async def generate_response(
        self,
        prompt: str,
        max_tokens: int | None = None,
        temperature: float | None = None,
        top_p: float | None = None,
    ) -> tuple[str, TokenUsage]:
        if self.fail_on_generate:
            raise AssertionError("summary provider should not have been called")
        self.prompts.append(prompt)
        if not self._summaries:
            raise AssertionError("no canned summaries remaining")
        summary = self._summaries.pop(0)
        return summary, TokenUsage(input_tokens=111, output_tokens=22)

    async def stream_response(
        self,
        prompt: str,
        max_tokens: int | None = None,
        temperature: float | None = None,
        top_p: float | None = None,
        tools: list[dict[str, Any]] | None = None,
        messages: list[dict[str, Any]] | None = None,
        system_prompt: str | None = None,
    ) -> AsyncIterator[MessageStreamEvent]:
        raise NotImplementedError
        yield  # pragma: no cover

    async def health_check(self) -> bool:
        return True


class RecordingTargetProvider(LLMProvider):
    provider_type = ProviderType.OPENAI

    def __init__(
        self,
        *,
        context_tokens: int = 1_000,
        stream_texts: Sequence[str] | None = None,
        model_record_id: str | None = None,
    ):
        self.context_tokens = context_tokens
        self.stream_texts = list(stream_texts or ["done"])
        self.model_record_id = model_record_id
        self.model_name = "recording-target-model"
        self.recorded_messages: list[list[dict[str, Any]]] = []
        self.stream_calls: list[dict[str, Any]] = []

    async def get_context_window_tokens(self) -> ContextWindowInfo:
        return ContextWindowInfo(tokens=self.context_tokens, source="safe_default")

    async def generate_response(
        self,
        prompt: str,
        max_tokens: int | None = None,
        temperature: float | None = None,
        top_p: float | None = None,
    ) -> tuple[str, TokenUsage]:
        return "target response", TokenUsage(input_tokens=1, output_tokens=1)

    async def stream_response(
        self,
        prompt: str,
        max_tokens: int | None = None,
        temperature: float | None = None,
        top_p: float | None = None,
        tools: list[dict[str, Any]] | None = None,
        messages: list[dict[str, Any]] | None = None,
        system_prompt: str | None = None,
    ) -> AsyncIterator[MessageStreamEvent]:
        recorded = list(messages or [])
        self.recorded_messages.append(recorded)
        self.stream_calls.append(
            {
                "messages": recorded,
                "tools": tools or [],
                "system_prompt": system_prompt,
            }
        )
        idx = min(len(self.recorded_messages) - 1, len(self.stream_texts) - 1)
        for event in text_response_events(self.stream_texts[idx]):
            yield event

    async def health_check(self) -> bool:
        return True


@pytest.fixture(autouse=True)
async def _compaction_test_setup(db_pool, monkeypatch):
    monkeypatch.setattr(db.connection, "_db_pool", db_pool)
    monkeypatch.setattr(compaction_module, "COMPACTION_RECENT_MESSAGES_COUNT", 3)
    async with db_pool.acquire() as conn:
        await conn.execute("DELETE FROM compactions")
        await conn.execute("DELETE FROM model_usage")
        await conn.execute("DELETE FROM agent_run_logs")
        await conn.execute("DELETE FROM agent_runs")
        await conn.execute("DELETE FROM agents")
        await conn.execute("DELETE FROM chat_messages")
        await conn.execute("DELETE FROM chats")
        await conn.execute("DELETE FROM models")
        await conn.execute("DELETE FROM model_providers")


async def _create_model_records(db_pool) -> tuple[str, str]:
    provider_id = str(ULID())
    target_model_id = str(ULID())
    summary_model_id = str(ULID())
    async with db_pool.acquire() as conn:
        await conn.execute(
            "INSERT INTO model_providers (id, name, provider_type, config) VALUES ($1, $2, $3, $4::jsonb)",
            provider_id,
            "Compaction Test Provider",
            "openai",
            "{}",
        )
        await conn.execute(
            """INSERT INTO models (id, model_provider_id, model_id, display_name, is_default, is_secondary)
               VALUES ($1, $2, $3, $4, false, false), ($5, $2, $6, $7, false, true)""",
            target_model_id,
            provider_id,
            "recording-target-model",
            "Recording Target Model",
            summary_model_id,
            "canned-summary-model",
            "Canned Summary Model",
        )
    return target_model_id, summary_model_id


async def _create_chat(db_pool, user_id: str, model_id: str | None = None) -> str:
    chat_id = str(ULID())
    async with db_pool.acquire() as conn:
        await conn.execute(
            "INSERT INTO chats (id, user_id, title, model_id) VALUES ($1, $2, $3, $4)",
            chat_id,
            user_id,
            "Compaction test chat",
            model_id,
        )
    return chat_id


class _DummySearcherClient:
    async def upsert_capabilities(self, request):
        return None

    async def sync_capabilities(self, request):
        return None

    async def search_capabilities(self, request):
        return SimpleNamespace(results=[])

    async def get_attribute_values(self, attribute_keys):
        return {}


class _DummyMemoryProvider:
    pass


def _app_state_with_providers(
    target_model_id: str,
    target_provider: RecordingTargetProvider,
    summary_model_id: str,
    summary_provider: CannedSummaryProvider,
) -> AppState:
    state = AppState()
    state.models = {
        target_model_id: target_provider,
        summary_model_id: summary_provider,
    }
    state.default_model_id = target_model_id
    state.secondary_model_id = summary_model_id
    searcher_tool = AsyncMock()
    searcher_tool.client = _DummySearcherClient()
    state.searcher_tool = searcher_tool
    state.content_storage = None
    state.redis_client = None
    state.memory_provider = None
    return state


def _chat_app(state: AppState) -> FastAPI:
    app = FastAPI()
    app.state = state
    app.include_router(chat_router)
    return app


async def _create_agent(db_pool, user_id: str, model_id: str | None = None) -> str:
    agent_id = str(ULID())
    async with db_pool.acquire() as conn:
        await conn.execute(
            """INSERT INTO agents (id, user_id, name, instructions, agent_type,
                                      schedule_type, schedule_value, model_id,
                                      allowed_sources, allowed_actions,
                                      is_enabled, is_deleted)
               VALUES ($1, $2, 'Compaction Agent', 'Summarize long runs', 'user',
                       'interval', '60', $3, '[]'::jsonb, '[]'::jsonb, true, false)""",
            agent_id,
            user_id,
            model_id,
        )
    return agent_id


async def _get_agent(db_pool, agent_id: str) -> Agent:
    async with db_pool.acquire() as conn:
        row = await conn.fetchrow(
            """SELECT id, user_id, name, instructions, agent_type, schedule_type,
                      schedule_value, model_id, allowed_sources, allowed_actions,
                      is_enabled, is_deleted, created_at, updated_at
               FROM agents
               WHERE id = $1""",
            agent_id,
        )
    assert row is not None
    return Agent.from_row(dict(row))


async def _create_running_agent_run(
    db_pool, user_id: str, model_id: str | None = None
) -> tuple[str, str]:
    agent_id = await _create_agent(db_pool, user_id, model_id=model_id)
    repo = AgentRunRepository(pool=db_pool)
    created = await repo.create_run(agent_id, AgentRunTriggerType.MANUAL)
    assert isinstance(created, AgentRun)
    claim = await repo.claim_next_run(
        max_concurrent_runs=1,
        lease_duration=timedelta(minutes=5),
        retry_policy=AgentRunRetryPolicy(
            max_attempts=3,
            backoff_delays=(timedelta(seconds=0), timedelta(seconds=0), timedelta(seconds=0)),
        ),
    )
    assert claim is not None
    return claim.run.id, claim.claim_token


def _long_message(role: str, label: str) -> MessageParam:
    return MessageParam(role=role, content=f"{label} " + (f"{label}-payload " * 45))


def _rows_to_messages(rows: Sequence[Any]) -> list[MessageParam]:
    return [MessageParam(**row.message) for row in rows]


def _coalesce_log_rows(rows: list[AgentRunLog]) -> list[MessageParam]:
    return [MessageParam(**row.message) for row in rows]


async def _append_linear_chat_messages(
    repo: MessagesRepository,
    chat_id: str,
    labels: Sequence[str],
) -> list[Any]:
    parent_id: str | None = None
    rows = []
    for idx, label in enumerate(labels):
        role = "user" if idx % 2 == 0 else "assistant"
        row = await repo.create(chat_id, _long_message(role, label), parent_id=parent_id)
        rows.append(row)
        parent_id = row.id
    return rows


async def _prepare_chat_with_repo(
    *,
    chat_id: str,
    chat_rows: list[Any],
    compactions_repo: CompactionsRepository,
    summary_provider: CannedSummaryProvider,
    target_provider: RecordingTargetProvider,
    max_output_tokens: int = 100,
):
    compactor = ConversationCompactor(llm_provider=summary_provider)
    return await compactor.prepare_chat_conversation(
        chat_id=chat_id,
        chat_messages=chat_rows,
        messages=_rows_to_messages(chat_rows),
        compactions_repo=compactions_repo,
        target_provider=target_provider,
        tools=[],
        system_prompt="You are a helpful assistant.",
        max_output_tokens=max_output_tokens,
    )


async def _compaction_rows(db_pool, target_type: str):
    async with db_pool.acquire() as conn:
        return await conn.fetch(
            """SELECT id, target_type, chat_id, agent_run_id, anchor_message_id,
                      anchor_log_id, compacted_through_seq_num,
                      previous_compaction_id, summary, summary_message
               FROM compactions
               WHERE target_type = $1
               ORDER BY compacted_through_seq_num ASC""",
            target_type,
        )


def _summary_text(message: MessageParam) -> str:
    content = message["content"]
    assert isinstance(content, str)
    return content


@pytest.mark.asyncio
async def test_chat_compaction_only_applies_to_active_branch(db_pool):
    user_id, _ = await create_test_user(db_pool)
    chat_id = await _create_chat(db_pool, user_id)
    messages_repo = MessagesRepository(pool=db_pool)
    compactions_repo = CompactionsRepository(pool=db_pool)

    m1 = await messages_repo.create(chat_id, _long_message("user", "shared-01"))
    m2 = await messages_repo.create(chat_id, _long_message("assistant", "shared-02"), parent_id=m1.id)

    branch_a = [m1, m2]
    parent_id = m2.id
    for idx in range(3, 11):
        row = await messages_repo.create(
            chat_id,
            _long_message("user" if idx % 2 else "assistant", f"branch-a-{idx:02d}"),
            parent_id=parent_id,
        )
        branch_a.append(row)
        parent_id = row.id

    branch_b = [m1, m2]
    parent_id = m2.id
    for idx in range(3, 11):
        row = await messages_repo.create(
            chat_id,
            _long_message("user" if idx % 2 else "assistant", f"branch-b-{idx:02d}"),
            parent_id=parent_id,
        )
        branch_b.append(row)
        parent_id = row.id

    summary_provider = CannedSummaryProvider(["summary-A", "summary-B"])
    target_provider = RecordingTargetProvider(context_tokens=1_000)

    prepared_a = await _prepare_chat_with_repo(
        chat_id=chat_id,
        chat_rows=branch_a,
        compactions_repo=compactions_repo,
        summary_provider=summary_provider,
        target_provider=target_provider,
    )
    assert "summary-A" in _summary_text(prepared_a.messages[0])

    prepared_b = await _prepare_chat_with_repo(
        chat_id=chat_id,
        chat_rows=branch_b,
        compactions_repo=compactions_repo,
        summary_provider=summary_provider,
        target_provider=target_provider,
    )

    first_b_message = _summary_text(prepared_b.messages[0])
    assert "summary-B" in first_b_message
    assert "summary-A" not in first_b_message

    rows = await _compaction_rows(db_pool, "chat")
    assert len(rows) == 2
    assert rows[0]["anchor_message_id"].strip() in {row.id for row in branch_a}
    assert rows[1]["anchor_message_id"].strip() in {row.id for row in branch_b}
    assert rows[0]["anchor_message_id"].strip() not in {row.id for row in branch_b}


@pytest.mark.asyncio
async def test_chat_revisit_reuses_durable_compaction_without_resummarizing(db_pool):
    user_id, _ = await create_test_user(db_pool)
    chat_id = await _create_chat(db_pool, user_id)
    messages_repo = MessagesRepository(pool=db_pool)
    compactions_repo = CompactionsRepository(pool=db_pool)
    rows = await _append_linear_chat_messages(
        messages_repo, chat_id, [f"revisit-{idx:02d}" for idx in range(1, 9)]
    )

    first_summary_provider = CannedSummaryProvider(["durable-summary"])
    first = await _prepare_chat_with_repo(
        chat_id=chat_id,
        chat_rows=rows,
        compactions_repo=compactions_repo,
        summary_provider=first_summary_provider,
        target_provider=RecordingTargetProvider(context_tokens=1_000),
    )
    assert "durable-summary" in _summary_text(first.messages[0])

    no_summary_provider = CannedSummaryProvider([], fail_on_generate=True)
    second = await _prepare_chat_with_repo(
        chat_id=chat_id,
        chat_rows=rows,
        compactions_repo=compactions_repo,
        summary_provider=no_summary_provider,
        target_provider=RecordingTargetProvider(context_tokens=1_400),
    )

    assert "durable-summary" in _summary_text(second.messages[0])
    assert [dict(msg) for msg in second.messages[-3:]] == [dict(msg) for msg in _rows_to_messages(rows[-3:])]
    assert len(no_summary_provider.prompts) == 0

    async with db_pool.acquire() as conn:
        assert await conn.fetchval("SELECT COUNT(*) FROM chat_messages WHERE chat_id = $1", chat_id) == len(rows)
        assert await conn.fetchval("SELECT COUNT(*) FROM compactions WHERE target_type = 'chat'") == 1


@pytest.mark.asyncio
async def test_chat_rolling_recompaction_uses_previous_summary_plus_new_prefix(db_pool):
    user_id, _ = await create_test_user(db_pool)
    chat_id = await _create_chat(db_pool, user_id)
    messages_repo = MessagesRepository(pool=db_pool)
    compactions_repo = CompactionsRepository(pool=db_pool)

    initial_rows = await _append_linear_chat_messages(
        messages_repo, chat_id, [f"roll-{idx:02d}" for idx in range(1, 9)]
    )
    summary_provider = CannedSummaryProvider(["summary-c1", "summary-c2"])
    target_provider = RecordingTargetProvider(context_tokens=1_000)

    first = await _prepare_chat_with_repo(
        chat_id=chat_id,
        chat_rows=initial_rows,
        compactions_repo=compactions_repo,
        summary_provider=summary_provider,
        target_provider=target_provider,
    )
    assert "summary-c1" in _summary_text(first.messages[0])
    c1 = first.latest_compaction
    assert c1 is not None

    parent_id = initial_rows[-1].id
    for idx in range(9, 17):
        row = await messages_repo.create(
            chat_id,
            _long_message("user" if idx % 2 else "assistant", f"roll-{idx:02d}"),
            parent_id=parent_id,
        )
        parent_id = row.id

    all_rows = await messages_repo.get_by_chat(chat_id)
    second = await _prepare_chat_with_repo(
        chat_id=chat_id,
        chat_rows=all_rows,
        compactions_repo=compactions_repo,
        summary_provider=summary_provider,
        target_provider=target_provider,
    )

    c2 = second.latest_compaction
    assert c2 is not None
    assert c2.previous_compaction_id == c1.id
    assert "summary-c2" in _summary_text(second.messages[0])

    second_prompt = summary_provider.prompts[1]
    assert "summary-c1" in second_prompt
    assert "roll-06" in second_prompt
    assert "roll-10" in second_prompt
    assert "roll-13" not in second_prompt
    assert "roll-01" not in second_prompt
    assert "roll-02" not in second_prompt
    assert [dict(msg) for msg in second.messages[-3:]] == [dict(msg) for msg in _rows_to_messages(all_rows[-3:])]


@pytest.mark.asyncio
async def test_chat_compaction_preserves_recent_messages_as_raw(db_pool):
    user_id, _ = await create_test_user(db_pool)
    chat_id = await _create_chat(db_pool, user_id)
    messages_repo = MessagesRepository(pool=db_pool)
    rows = await _append_linear_chat_messages(
        messages_repo, chat_id, [f"recent-{idx:02d}" for idx in range(1, 10)]
    )

    prepared = await _prepare_chat_with_repo(
        chat_id=chat_id,
        chat_rows=rows,
        compactions_repo=CompactionsRepository(pool=db_pool),
        summary_provider=CannedSummaryProvider(["recent-summary"]),
        target_provider=RecordingTargetProvider(context_tokens=1_000),
    )

    assert "recent-summary" in _summary_text(prepared.messages[0])
    assert len(prepared.messages) >= 4
    raw_suffix_text = "\n".join(str(message["content"]) for message in prepared.messages[1:])
    assert "recent-05" in raw_suffix_text
    assert "recent-07" in raw_suffix_text
    assert "recent-09" in raw_suffix_text
    assert [dict(msg) for msg in prepared.messages[-3:]] == [dict(msg) for msg in _rows_to_messages(rows[-3:])]


@pytest.mark.asyncio
async def test_chat_compaction_does_not_split_tool_use_tool_result_pair(db_pool):
    user_id, _ = await create_test_user(db_pool)
    chat_id = await _create_chat(db_pool, user_id)
    messages_repo = MessagesRepository(pool=db_pool)

    parent_id = None
    rows = []
    for message in [
        _long_message("user", "tool-user-01"),
        _long_message("assistant", "tool-assistant-01"),
        _long_message("user", "tool-user-02"),
        _long_message("assistant", "tool-assistant-02"),
        _long_message("user", "tool-user-03"),
        MessageParam(
            role="assistant",
            content=[
                {"type": "text", "text": "I will call the tool."},
                {"type": "tool_use", "id": "tool-1", "name": "search", "input": {"query": "omni"}},
            ],
        ),
        MessageParam(
            role="user",
            content=[{"type": "tool_result", "tool_use_id": "tool-1", "content": "tool result payload"}],
        ),
        _long_message("assistant", "tool-followup"),
        _long_message("user", "tool-user-04"),
        _long_message("assistant", "tool-assistant-04"),
        _long_message("user", "tool-user-05"),
    ]:
        row = await messages_repo.create(chat_id, message, parent_id=parent_id)
        rows.append(row)
        parent_id = row.id

    prepared = await _prepare_chat_with_repo(
        chat_id=chat_id,
        chat_rows=rows,
        compactions_repo=CompactionsRepository(pool=db_pool),
        summary_provider=CannedSummaryProvider(["tool-summary"]),
        target_provider=RecordingTargetProvider(context_tokens=400),
        max_output_tokens=50,
    )

    assert "tool-summary" in _summary_text(prepared.messages[0])
    recent = prepared.messages[1:]
    has_tool_use = any(
        isinstance(msg.get("content"), list)
        and any(isinstance(block, dict) and block.get("type") == "tool_use" for block in msg["content"])
        for msg in recent
    )
    has_tool_result = any(
        isinstance(msg.get("content"), list)
        and any(isinstance(block, dict) and block.get("type") == "tool_result" for block in msg["content"])
        for msg in recent
    )
    assert has_tool_result
    assert has_tool_use


@pytest.mark.asyncio
async def test_agent_run_compaction_preserves_raw_logs(db_pool):
    user_id, _ = await create_test_user(db_pool)
    run_id, claim_token = await _create_running_agent_run(db_pool, user_id)
    run_repo = AgentRunRepository(pool=db_pool)
    compactions_repo = CompactionsRepository(pool=db_pool)

    messages = [_long_message("user" if idx % 2 else "assistant", f"agent-{idx:02d}") for idx in range(1, 10)]
    await run_repo.append_run_log_messages(run_id, claim_token, messages)
    log_rows = await run_repo.list_run_logs(run_id)

    compactor = ConversationCompactor(llm_provider=CannedSummaryProvider(["agent-summary"]))
    prepared = await compactor.prepare_agent_conversation(
        run_id=run_id,
        log_rows=log_rows,
        compactions_repo=compactions_repo,
        target_provider=RecordingTargetProvider(context_tokens=1_000),
        tools=[],
        system_prompt="Agent instructions",
        max_output_tokens=100,
        coalesce_messages=_coalesce_log_rows,
    )

    assert "agent-summary" in _summary_text(prepared.messages[0])
    assert [dict(msg) for msg in prepared.messages[-3:]] == [dict(msg) for msg in _coalesce_log_rows(log_rows[-3:])]

    stored_logs = await run_repo.list_run_logs(run_id)
    assert len(stored_logs) == len(log_rows)
    assert all("CONVERSATION SUMMARY" not in str(row.message) for row in stored_logs)

    compaction_rows = await _compaction_rows(db_pool, "agent_run")
    assert len(compaction_rows) == 1
    assert compaction_rows[0]["anchor_log_id"] is not None
    assert compaction_rows[0]["agent_run_id"].strip() == run_id


@pytest.mark.asyncio
async def test_agent_rolling_recompaction_uses_previous_summary_plus_new_logs(db_pool):
    user_id, _ = await create_test_user(db_pool)
    run_id, claim_token = await _create_running_agent_run(db_pool, user_id)
    run_repo = AgentRunRepository(pool=db_pool)
    compactions_repo = CompactionsRepository(pool=db_pool)
    summary_provider = CannedSummaryProvider(["agent-summary-c1", "agent-summary-c2"])
    target_provider = RecordingTargetProvider(context_tokens=1_000)
    compactor = ConversationCompactor(llm_provider=summary_provider)

    initial_messages = [_long_message("user" if idx % 2 else "assistant", f"agent-roll-{idx:02d}") for idx in range(1, 9)]
    await run_repo.append_run_log_messages(run_id, claim_token, initial_messages)
    initial_logs = await run_repo.list_run_logs(run_id)

    first = await compactor.prepare_agent_conversation(
        run_id=run_id,
        log_rows=initial_logs,
        compactions_repo=compactions_repo,
        target_provider=target_provider,
        tools=[],
        system_prompt="Agent instructions",
        max_output_tokens=100,
        coalesce_messages=_coalesce_log_rows,
    )
    c1 = first.latest_compaction
    assert c1 is not None

    more_messages = [_long_message("user" if idx % 2 else "assistant", f"agent-roll-{idx:02d}") for idx in range(9, 17)]
    await run_repo.append_run_log_messages(run_id, claim_token, more_messages)
    all_logs = await run_repo.list_run_logs(run_id)

    second = await compactor.prepare_agent_conversation(
        run_id=run_id,
        log_rows=all_logs,
        compactions_repo=compactions_repo,
        target_provider=target_provider,
        tools=[],
        system_prompt="Agent instructions",
        max_output_tokens=100,
        coalesce_messages=_coalesce_log_rows,
    )

    c2 = second.latest_compaction
    assert c2 is not None
    assert c2.previous_compaction_id == c1.id
    assert "agent-summary-c2" in _summary_text(second.messages[0])

    second_prompt = summary_provider.prompts[1]
    assert "agent-summary-c1" in second_prompt
    assert "agent-roll-06" in second_prompt
    assert "agent-roll-10" in second_prompt
    assert "agent-roll-13" not in second_prompt
    assert "agent-roll-01" not in second_prompt
    assert len(await run_repo.list_run_logs(run_id)) == len(all_logs)
    assert [dict(msg) for msg in second.messages[-3:]] == [dict(msg) for msg in _coalesce_log_rows(all_logs[-3:])]


@pytest.mark.asyncio
async def test_chat_stream_api_runs_compaction_before_provider_call(db_pool):
    target_model_id, summary_model_id = await _create_model_records(db_pool)
    user_id, _ = await create_test_user(db_pool)
    chat_id = await _create_chat(db_pool, user_id, model_id=target_model_id)
    messages_repo = MessagesRepository(pool=db_pool)
    rows = await _append_linear_chat_messages(
        messages_repo, chat_id, [f"api-chat-{idx:02d}" for idx in range(1, 10)]
    )

    target_provider = RecordingTargetProvider(
        context_tokens=1_000,
        stream_texts=["api chat response"],
        model_record_id=target_model_id,
    )
    summary_provider = CannedSummaryProvider(
        ["api-chat-summary"], model_record_id=summary_model_id
    )
    app = _chat_app(
        _app_state_with_providers(
            target_model_id, target_provider, summary_model_id, summary_provider
        )
    )

    async with AsyncClient(transport=ASGITransport(app=app), base_url="http://test") as client:
        response = await client.get(f"/chat/{chat_id}/stream", timeout=30)

    assert response.status_code == 200
    assert "api chat response" in response.text
    assert "event: compaction_start" in response.text
    assert "event: compaction_end" in response.text
    assert response.text.index("event: compaction_start") < response.text.index(
        "event: compaction_end"
    )
    assert response.text.index("event: compaction_end") < response.text.index(
        "event: message\n"
    )
    assert len(target_provider.recorded_messages) == 1
    provider_messages = target_provider.recorded_messages[0]
    assert "api-chat-summary" in _summary_text(provider_messages[0])
    assert [dict(msg) for msg in provider_messages[-3:]] == [dict(msg) for msg in _rows_to_messages(rows[-3:])]

    compaction_rows = await _compaction_rows(db_pool, "chat")
    assert len(compaction_rows) == 1
    assert compaction_rows[0]["anchor_message_id"].strip() in {row.id for row in rows}


@pytest.mark.asyncio
async def test_agent_execution_api_runs_compaction_before_provider_call(db_pool):
    target_model_id, summary_model_id = await _create_model_records(db_pool)
    user_id, _ = await create_test_user(db_pool)
    run_id, claim_token = await _create_running_agent_run(
        db_pool, user_id, model_id=target_model_id
    )
    run_repo = AgentRunRepository(pool=db_pool)

    messages = [
        _long_message("user" if idx % 2 else "assistant", f"api-agent-{idx:02d}")
        for idx in range(1, 41)
    ]
    await run_repo.append_run_log_messages(run_id, claim_token, messages)
    run = await run_repo.get_run(run_id)
    assert run is not None
    agent = await _get_agent(db_pool, run.agent_id)

    target_provider = RecordingTargetProvider(
        context_tokens=2_000,
        stream_texts=["agent first response", "agent run summary"],
        model_record_id=target_model_id,
    )
    summary_provider = CannedSummaryProvider(
        ["api-agent-compaction-summary"], model_record_id=summary_model_id
    )
    state = _app_state_with_providers(
        target_model_id, target_provider, summary_model_id, summary_provider
    )

    result = await execute_claimed_agent(agent, state, run, claim_token, run_repo)

    assert result.summary == "agent run summary"
    assert len(target_provider.recorded_messages) >= 2
    first_provider_messages = target_provider.recorded_messages[0]
    assert "api-agent-compaction-summary" in _summary_text(first_provider_messages[0])

    stored_logs = await run_repo.list_run_logs(run_id)
    assert len(stored_logs) > len(messages)
    assert all("CONVERSATION SUMMARY" not in str(row.message) for row in stored_logs)

    compaction_rows = await _compaction_rows(db_pool, "agent_run")
    assert len(compaction_rows) == 1
    assert compaction_rows[0]["agent_run_id"].strip() == run_id
    assert compaction_rows[0]["anchor_log_id"] is not None

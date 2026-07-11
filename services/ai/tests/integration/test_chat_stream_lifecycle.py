"""Integration tests for the chat stream lifecycle (producer/consumer, resume,
cancel, locks, heartbeat, persistence transform, interruption repair, approval).

Exercises the **Redis-backed decoupled streaming path** with real Postgres and
real Redis.

Design principle: **never let the consumer block on an empty stream.**  The
consumer (``_consume_run``) calls ``xread`` with ``block=_STREAM_HEARTBEAT_MS``
which defaults to 15 s.  If the LLM never writes events (e.g. it's gated at
``"pre"``), the consumer loops until ``_RUN_LOCK_TTL`` (300 s).  Instead:
* Gate at ``"post"`` (after events are written) or gate the **second** LLM call
  so the stream has data from the first call.
* Monkeypatch ``_RUN_LOCK_TTL`` and ``_STREAM_HEARTBEAT_MS`` to short values
  for timing-sensitive tests.
"""

from __future__ import annotations

import asyncio
import json
from unittest.mock import AsyncMock

import pytest
from fastapi import FastAPI
from httpx import ASGITransport, AsyncClient
from ulid import ULID

import db.connection
import routers.chat as chat_module
from db import ChatsRepository, MessagesRepository, UsersRepository
from db.tool_approvals import (
    ToolApprovalsRepository,
    ToolApprovalStatus,
    ToolApprovalType,
)
from routers import chat_router
from state import AppState
from tests.helpers import (
    GatedRecordingLLM,
    assert_sse_wire,
    collect_sse_events,
    stream_sse,
)
from tools import SearchResponse, SearchResult, ToolRegistry, ToolResult
from tools.omni_tool_result import OAuthRequiredPayload
from tools.searcher_client import Document
from tools.searcher_tool import SearcherTool

pytestmark = pytest.mark.integration


# =============================================================================
# Timing constants — patched short for fast tests
# =============================================================================

_FAST_LOCK_TTL = 30         # seconds (was 300)
_FAST_HEARTBEAT_MS = 1000   # ms (was 15000)
_FAST_TTL = 10              # stream TTL seconds (was 300)


@pytest.fixture
def _patch_timing(monkeypatch):
    """Shrink all timing constants so consumers/producers fail fast."""
    monkeypatch.setattr("streaming.run._RUN_LOCK_TTL", _FAST_LOCK_TTL)
    monkeypatch.setattr("routers.chat._RUN_LOCK_TTL", _FAST_LOCK_TTL)
    monkeypatch.setattr("streaming.run._STREAM_HEARTBEAT_MS", _FAST_HEARTBEAT_MS)
    monkeypatch.setattr("streaming.run._STREAM_TTL", _FAST_TTL)
    # _CANCEL_CHECK_INTERVAL_SECONDS and _RUN_LOCK_TTL are imported by name
    # in multiple modules; patch each local binding so monkeypatched values
    # take effect everywhere.
    monkeypatch.setattr("streaming.run._CANCEL_CHECK_INTERVAL_SECONDS", 0.5)
    monkeypatch.setattr("streaming.generate._CANCEL_CHECK_INTERVAL_SECONDS", 0.5)


# =============================================================================
# App builder
# =============================================================================


def _build_chat_app(
    llm_provider,
    redis_client,
    model_id: str,
) -> FastAPI:
    """Build a minimal test FastAPI app wired to the real Redis client."""
    app = FastAPI()
    app.state = AppState()
    app.state.models = {model_id: llm_provider}
    app.state.default_model_id = model_id
    app.state.secondary_model_id = model_id
    app.state.searcher_tool = SearcherTool()
    app.state.content_storage = None
    app.state.redis_client = redis_client
    app.state.memory_provider = None
    app.include_router(chat_router)
    return app


# =============================================================================
# Fixtures
# =============================================================================


@pytest.fixture
def _patch_db_pool(db_pool, monkeypatch):
    monkeypatch.setattr(db.connection, "_db_pool", db_pool)


@pytest.fixture
def _patch_chat_config(monkeypatch):
    monkeypatch.setattr("routers.chat.CONNECTOR_MANAGER_URL", "http://127.0.0.1:1")
    monkeypatch.setattr("routers.chat.SANDBOX_URL", "")
    monkeypatch.setenv("SEARCHER_URL", "http://127.0.0.1:1")


@pytest.fixture
async def seeded_chat(db_pool, _patch_db_pool, _patch_chat_config, _patch_timing) -> tuple[str, str, str]:
    """Create a user, model, chat, and first user message.

    Returns ``(chat_id, user_id, model_id)``.  Cleans up DB rows on teardown.
    """
    users_repo = UsersRepository()
    user = await users_repo.create(
        email=f"{ULID()}@test.local",
        password_hash="not-a-real-hash",
        full_name="Stream Test User",
    )

    async with db_pool.acquire() as conn:
        provider_id = str(ULID())
        await conn.execute(
            "INSERT INTO model_providers (id, name, provider_type, config) VALUES ($1, $2, $3, $4)",
            provider_id,
            "Stream Test Provider",
            "anthropic",
            "{}",
        )
        model_id = str(ULID())
        await conn.execute(
            "INSERT INTO models (id, model_provider_id, model_id, display_name, is_default) VALUES ($1, $2, $3, $4, $5)",
            model_id,
            provider_id,
            "stream-test-model",
            "Stream Test Model",
            False,
        )

    chat = await ChatsRepository().create(user_id=user.id, model_id=model_id)
    chat_id = chat.id

    await MessagesRepository().create(
        chat_id,
        {"role": "user", "content": "Hello"},
    )

    try:
        yield (chat_id, user.id, model_id)
    finally:
        async with db_pool.acquire() as conn:
            await conn.execute("DELETE FROM chat_messages WHERE chat_id = $1", chat_id)
            await conn.execute("DELETE FROM chats WHERE id = $1", chat_id)
        async with db_pool.acquire() as conn:
            await conn.execute("DELETE FROM models WHERE id = $1", model_id)
            await conn.execute("DELETE FROM model_providers WHERE id = $1", provider_id)
        async with db_pool.acquire() as conn:
            await conn.execute("DELETE FROM users WHERE id = $1", user.id)


@pytest.fixture
async def redis_keys(redis_client) -> None:
    """Teardown-only: clean up Redis stream/lock/cancel keys after each test."""
    yield
    for pattern in ("chat:stream:*", "chat:runlock:*", "chat:cancel:*"):
        cursor = 0
        while True:
            cursor, keys = await redis_client.scan(cursor, match=pattern, count=100)
            if keys:
                await redis_client.delete(*keys)
            if cursor == 0:
                break


def _client(app: FastAPI) -> AsyncClient:
    return AsyncClient(transport=ASGITransport(app=app), base_url="http://test")


class ScriptedActionHandler:
    def __init__(
        self,
        *,
        requires_approval: bool,
        results: list[ToolResult],
        tool_name: str = "gmail__send_email",
    ) -> None:
        self.tool_name = tool_name
        self._requires_approval = requires_approval
        self._results = results
        self.executions: list[dict] = []

    def get_tools(self):
        return [
            {
                "name": self.tool_name,
                "description": "Test connector action",
                "input_schema": {"type": "object", "properties": {}},
            }
        ]

    def can_handle(self, tool_name: str) -> bool:
        return tool_name == self.tool_name

    def requires_approval(self, tool_name: str) -> bool:
        return self._requires_approval and self.can_handle(tool_name)

    async def execute(self, tool_name: str, tool_input: dict, _context) -> ToolResult:
        self.executions.append(tool_input)
        result_index = min(len(self.executions) - 1, len(self._results) - 1)
        return self._results[result_index]


class MultiActionHandler:
    def __init__(self, tool_names: set[str], approval_tool_names: set[str]) -> None:
        self.tool_names = tool_names
        self.approval_tool_names = approval_tool_names
        self.executions: list[str] = []

    def get_tools(self):
        return [
            {
                "name": tool_name,
                "description": "Test connector action",
                "input_schema": {"type": "object", "properties": {}},
            }
            for tool_name in sorted(self.tool_names)
        ]

    def can_handle(self, tool_name: str) -> bool:
        return tool_name in self.tool_names

    def requires_approval(self, tool_name: str) -> bool:
        return tool_name in self.approval_tool_names

    async def execute(self, tool_name: str, _tool_input: dict, _context) -> ToolResult:
        self.executions.append(tool_name)
        return ToolResult(content=[{"type": "text", "text": f"{tool_name} completed"}])


def _install_scripted_registry(monkeypatch, handler) -> None:
    registry = ToolRegistry()
    registry.register(handler)

    async def build_registry(*_args, **_kwargs):
        return chat_module.RegistryResult(
            registry=registry,
            always_on_handlers=[handler],
            connector_handler=None,
            toolsets=[],
            sources=[],
            search_operators=[],
        )

    monkeypatch.setattr(chat_module, "_build_registry", build_registry)


async def _interventions(
    chat_id: str,
    approval_type: ToolApprovalType,
    statuses: set[ToolApprovalStatus],
):
    return await ToolApprovalsRepository().list_for_chat(
        chat_id=chat_id,
        approval_type=approval_type,
        statuses=statuses,
    )


# =============================================================================
# Buffered-run baseline (producer/consumer)
# =============================================================================


class TestBaseline:
    """Happy-path assertions on the decoupled producer/consumer."""

    @pytest.mark.asyncio
    async def test_happy_path_streams_text_and_persists_assistant(self, seeded_chat, redis_client, redis_keys):
        """E2E: stream a text response → DB has persisted assistant row;
        consumer events have strictly increasing ``id:`` lines; no duplicate
        writes."""
        chat_id, _user_id, model_id = seeded_chat
        llm = GatedRecordingLLM([("text", "This is a test response.")], model_id)

        app = _build_chat_app(llm, redis_client, model_id)
        async with _client(app) as client:
            events = await collect_sse_events(client, chat_id)

        terminal = [(e[0], e[1]) for e in events if e[0] in ("end_of_stream", "stream_error")]
        assert terminal, "Expected a terminal event"
        assert terminal[-1][0] == "end_of_stream", f"Unexpected terminal: {terminal[-1]}"

        msg_events = [e for e in events if e[0] == "message"]
        assert msg_events, "No message (SDK) events in stream"

        assert_sse_wire(events)

        db_msgs = await MessagesRepository().get_active_path(chat_id)
        assert len(db_msgs) >= 2, f"Expected ≥2 messages, got {len(db_msgs)}"
        assert db_msgs[-1].message["role"] == "assistant"
        assistant_text = db_msgs[-1].message["content"]
        if isinstance(assistant_text, list):
            text = " ".join(b.get("text", "") for b in assistant_text if b.get("type") == "text")
        else:
            text = str(assistant_text)
        assert "This is a test response." in text

        stream_key = f"chat:stream:{chat_id}"
        exists = await redis_client.exists(stream_key)
        assert exists == 1, "Stream key should exist after run completes (within TTL)"

    @pytest.mark.asyncio
    async def test_message_start_id_matches_db_row(
        self, seeded_chat, redis_client, redis_keys
    ):
        """``message_start``'s ``message.id`` equals the DB-assigned
        ``chat_messages.id`` of the assistant row."""
        chat_id, _user_id, model_id = seeded_chat
        llm = GatedRecordingLLM([("text", "Match the message id.")], model_id)

        app = _build_chat_app(llm, redis_client, model_id)
        async with _client(app) as client:
            events = await collect_sse_events(client, chat_id)

        msg_start_id: str | None = None
        for event_type, data, _sse_id in events:
            if event_type == "message":
                payload = json.loads(data)
                if payload.get("type") == "message_start":
                    msg_start_id = payload["message"]["id"]
                    break
        assert msg_start_id is not None, "No message_start event found"

        db_msgs = await MessagesRepository().get_active_path(chat_id)
        assistant_rows = [m for m in db_msgs if m.message["role"] == "assistant"]
        assert assistant_rows, "No assistant message in DB"
        assert assistant_rows[-1].id == msg_start_id, (
            f"message_start id {msg_start_id} != DB id {assistant_rows[-1].id}"
        )


# =============================================================================
# Resume / reconnect (Last-Event-ID)
# =============================================================================


class TestResume:
    """Tests for the ``Last-Event-ID`` resume/reconnect logic."""

    @pytest.mark.asyncio
    async def test_resume_from_last_event_id_yields_suffix_only(self, seeded_chat, redis_client, redis_keys):
        """Drop connection at ``id: k``, reconnect with ``Last-Event-ID: k`` →
        suffix events only, no duplicates."""
        chat_id, _user_id, model_id = seeded_chat
        llm = GatedRecordingLLM([("text", "First part. Second part. Final part.")], model_id)

        app = _build_chat_app(llm, redis_client, model_id)
        async with _client(app) as client:
            first_batch: list[tuple[str | None, str, str | None]] = []
            last_sse_id: str | None = None
            async for event_type, data, sse_id in stream_sse(client, chat_id):
                first_batch.append((event_type, data, sse_id))
                if sse_id is not None:
                    last_sse_id = sse_id
                if len(first_batch) >= 4:
                    break

            assert last_sse_id is not None, "No sse_id captured before disconnect"

            suffix_events = await collect_sse_events(
                client, chat_id, headers={"Last-Event-ID": last_sse_id}
            )

        suffix_ids = [sid for _et, _d, sid in suffix_events if sid is not None]
        assert all(sid > last_sse_id for sid in suffix_ids), (
            f"Suffix events include ids ≤ {last_sse_id}: {suffix_ids}"
        )

        suffix_terminals = [
            et for et, _d, _sid in suffix_events
            if et in ("end_of_stream", "stream_error", "not_resumable")
        ]
        assert suffix_terminals, "No terminal event in suffix"

    @pytest.mark.asyncio
    async def test_reconnect_after_run_replays_suffix(self, seeded_chat, redis_client, redis_keys):
        """Reconnect after run finished, within ``_STREAM_TTL``, with a
        ``Last-Event-ID`` → replays from that offset, then buffered
        ``end_of_stream``."""
        chat_id, _user_id, model_id = seeded_chat
        llm = GatedRecordingLLM([("text", "Complete stream.")], model_id)

        app = _build_chat_app(llm, redis_client, model_id)
        async with _client(app) as client:
            # Consume the full stream first
            first_events = await collect_sse_events(client, chat_id)
            first_sse_ids = [sid for _et, _d, sid in first_events if sid is not None]
            assert first_sse_ids, "No sse_ids in first stream"
            checkpoint_id = first_sse_ids[len(first_sse_ids) // 2]  # mid-point

            # Reconnect with Last-Event-ID at mid-point
            suffix = await collect_sse_events(
                client, chat_id, headers={"Last-Event-ID": checkpoint_id}
            )

        suffix_ids = [sid for _et, _d, sid in suffix if sid is not None]
        assert all(sid > checkpoint_id for sid in suffix_ids), (
            f"Suffix ids not all > {checkpoint_id}: {suffix_ids}"
        )
        assert any(
            et in ("end_of_stream", "stream_error") for et, _d, _ in suffix
        ), "No terminal event in suffix"

    @pytest.mark.asyncio
    async def test_reconnect_after_ttl_returns_not_resumable(self, seeded_chat, redis_client, redis_keys):
        """Reconnect after ``_STREAM_TTL`` expired with a ``Last-Event-ID`` →
        ``event: not_resumable`` and no fresh generation."""
        # _STREAM_TTL is already 10s from _patch_timing; the run completes
        # in <1s, so waiting 12s guarantees expiry.
        chat_id, _user_id, model_id = seeded_chat
        llm = GatedRecordingLLM([("text", "Transient response.")], model_id)

        app = _build_chat_app(llm, redis_client, model_id)
        async with _client(app) as client:
            first_events = await collect_sse_events(client, chat_id)
            assert first_events, "First stream produced no events"

        # Wait for TTL + margin
        await asyncio.sleep(_FAST_TTL + 3)

        async with _client(app) as client:
            events = await collect_sse_events(
                client, chat_id, headers={"Last-Event-ID": "0"}
            )

        event_types = [et for et, _d, _sid in events]
        assert "not_resumable" in event_types, (
            f"Expected not_resumable, got {event_types}"
        )
        msg_events = [et for et, _d, _sid in events if et == "message"]
        assert not msg_events, (
            f"Got message events after TTL expired: {msg_events}"
        )


# =============================================================================
# Cancellation
# =============================================================================


class TestCancel:
    """Graceful stop via the Redis cancel flag."""

    @pytest.mark.asyncio
    async def test_cancel_during_text_stream(self, seeded_chat, redis_client, redis_keys):
        """Cancel (cross‑worker style via the Redis flag) during a text
        response → consumer sees terminal event; ``stream/status`` later
        shows ``running=false``."""
        chat_id, _user_id, model_id = seeded_chat
        llm = GatedRecordingLLM([("text", "Response.")], model_id)

        app = _build_chat_app(llm, redis_client, model_id)
        async with _client(app) as client:
            stream_task = asyncio.create_task(
                collect_sse_events(client, chat_id)
            )

            # Let the run start and write at least the first event
            await asyncio.sleep(0.3)

            # Set the cancel flag directly (cross‑worker).  The producer's
            # event-loop cancel check (every 0.5s with our patch) will
            # detect it and finalize.
            await redis_client.set(f"chat:cancel:{chat_id}", "1", ex=_FAST_LOCK_TTL)

            # Wait for cancel check + margin
            await asyncio.sleep(1.0)

            events = await stream_task

        terminal = [(et, d) for et, d, _ in events if et in ("end_of_stream", "stream_error")]
        assert terminal, "No terminal event after cancel"

        async with _client(app) as status_client:
            resp = await status_client.get(f"/chat/{chat_id}/stream/status")
            assert resp.status_code == 200
            assert resp.json()["running"] is False

    @pytest.mark.asyncio
    async def test_pre_set_cancel_flag_cleaned_on_fresh_start(
        self, seeded_chat, redis_client, redis_keys
    ):
        """Cancel flag set with no producer running (cross‑worker) →
        subsequent stream starts cleanly (key cleaned up)."""
        chat_id, _user_id, model_id = seeded_chat

        await redis_client.set(f"chat:cancel:{chat_id}", "1", ex=300)

        llm = GatedRecordingLLM([("text", "I should be generated.")], model_id)

        app = _build_chat_app(llm, redis_client, model_id)
        async with _client(app) as client:
            events = await collect_sse_events(client, chat_id)

        terminal = [et for et, _d, _ in events if et in ("end_of_stream", "stream_error")]
        assert terminal, "No terminal event"

        cancel_key = f"chat:cancel:{chat_id}"
        exists = await redis_client.exists(cancel_key)
        assert exists == 0, "Cancel key was not cleaned up by producer"

    @pytest.mark.asyncio
    async def test_task_cancel_mid_stream_persists_partial(
        self, seeded_chat, redis_client, redis_keys
    ):
        """``task.cancel()`` (immediate Stop) mid-token-stream persists the
        partial assistant text in the database, so the row survives a page
        reload.

        Strategy: LLM with inter-event delay yields events slowly.  After the
        text_delta is processed (content_blocks has the partial text), cancel
        the producer task directly.  The fixed ``except CancelledError`` handler
        yields ``save_message``, which ``_persist_and_transform`` uses to update
        the early-persisted row instead of deleting it.
        """
        chat_id, _user_id, model_id = seeded_chat
        llm = GatedRecordingLLM(
            [("text", "Partial content after task cancel.")],
            model_id,
            inter_event_delay=0.3,
        )

        app = _build_chat_app(llm, redis_client, model_id)
        async with _client(app) as client:
            stream_task = asyncio.create_task(
                collect_sse_events(client, chat_id)
            )

            # Wait for the LLM to yield text_delta (content accumulated)
            # With inter_event_delay=0.3:
            #   t=0.0 message_start, t=0.3 content_block_start, t=0.6 text_delta
            await asyncio.sleep(0.8)

            # Cancel the producer task directly (immediate Stop)
            task = chat_module._run_tasks_by_chat.get(chat_id)
            assert task is not None, "Producer task not found"
            task.cancel()

            events = await stream_task

        # The stream must terminate with end_of_stream (clean stop, not error)
        assert any(
            et == "end_of_stream" for et, _, _ in events
        ), f"Expected end_of_stream after task.cancel(). Events: {[et for et, _, _ in events]}"

        # The partial assistant message must be persisted with its content
        db_msgs = await MessagesRepository().get_active_path(chat_id)
        assistant_msgs = [m for m in db_msgs if m.message["role"] == "assistant"]
        assert assistant_msgs, (
            "No assistant message persisted after task.cancel(). "
            "The partial content was lost!"
        )
        latest_asst = assistant_msgs[-1].message
        content = latest_asst["content"]
        text_blocks = [b for b in content if isinstance(b, dict) and b.get("type") == "text"]
        assert text_blocks, "No text block in persisted assistant message"
        text = " ".join(b["text"] for b in text_blocks)
        assert "Partial content" in text, (
            f"Partial assistant text missing after task.cancel(). "
            f"Got: {text!r}"
        )

    @pytest.mark.asyncio
    async def test_redis_flag_cancel_finalizes_partial_assistant(
        self, seeded_chat, redis_client, redis_keys
    ):
        """Cross-worker Stop via the Redis cancel flag finalizes the
        partial assistant content through the ``if cancelled: yield
        save_message`` checkpoint path (not the ``task.cancel()`` path).

        Strategy: LLM with inter-event delay yields text_delta events
        slowly so content_blocks accumulates content.  After text has
        started flowing, set the Redis cancel flag.  The event-loop
        cancel check (every ``_CANCEL_CHECK_INTERVAL_SECONDS``) detects
        it and finalizes via the checkpoint path.
        """
        chat_id, _user_id, model_id = seeded_chat
        llm = GatedRecordingLLM(
            [("text", "Partial content to persist via Redis flag.")],
            model_id,
            inter_event_delay=0.3,
        )

        app = _build_chat_app(llm, redis_client, model_id)
        async with _client(app) as client:
            stream_task = asyncio.create_task(
                collect_sse_events(client, chat_id)
            )

            # Wait for text_delta to be processed (content_blocks accumulates)
            await asyncio.sleep(0.8)

            # Set the Redis cancel flag (simulates cross-worker Stop)
            await redis_client.set(
                f"chat:cancel:{chat_id}", "1", ex=_FAST_LOCK_TTL
            )

            events = await stream_task

        # The stream must terminate cleanly (not with stream_error)
        assert any(
            et == "end_of_stream" for et, _, _ in events
        ), f"Expected end_of_stream after Redis-flag cancel. Events: {[et for et, _, _ in events]}"

        # The partial assistant message must be persisted
        db_msgs = await MessagesRepository().get_active_path(chat_id)
        assistant_msgs = [m for m in db_msgs if m.message["role"] == "assistant"]
        assert assistant_msgs, (
            "No assistant message persisted after Redis-flag cancel. "
            "The partial content was lost!"
        )
        latest_asst = assistant_msgs[-1].message
        content = latest_asst["content"]
        text_blocks = [
            b for b in content
            if isinstance(b, dict) and b.get("type") == "text"
        ]
        assert text_blocks, "No text block in persisted assistant message"
        text = " ".join(b["text"] for b in text_blocks)
        assert "Partial content" in text, (
            f"Partial assistant text missing after Redis-flag cancel. "
            f"Got: {text!r}"
        )


# =============================================================================
# /chat/{id}/stream/status
# =============================================================================


class TestStreamStatus:
    """Correctness of the ``stream/status`` endpoint at each lifecycle phase."""

    @pytest.mark.asyncio
    async def test_status_idle_all_flags_false(self, seeded_chat, redis_client, redis_keys):
        """Idle (no run active, no buffer) → all flags false."""
        chat_id, _user_id, _model_id = seeded_chat
        app = FastAPI()
        app.state = AppState()
        app.state.redis_client = redis_client
        app.include_router(chat_router)

        async with _client(app) as client:
            resp = await client.get(f"/chat/{chat_id}/stream/status")
            assert resp.status_code == 200
            status = resp.json()
            assert status["running"] is False
            assert status["resumable"] is False
            assert status["pending_approval"] is False
            assert status["pending_oauth"] is False

    @pytest.mark.asyncio
    async def test_status_running_true_during_run_then_false_after(self, seeded_chat, redis_client, redis_keys):
        """During a run → ``running=True``, then after the run completes →
        ``running=False, resumable=True``.

        The LLM is gated at ``"pre"`` so the producer stays alive (lock held,
        no events written) while we poll status.  This avoids a race where
        the stream finishes before the status poll catches ``running=True``.
        """
        import time

        chat_id, _user_id, model_id = seeded_chat
        llm = GatedRecordingLLM([("text", "Status check.")], model_id)
        llm.hold(0, at="pre")

        app = _build_chat_app(llm, redis_client, model_id)
        # Separate clients: one for the long-lived stream, one for status
        async with _client(app) as stream_cl, _client(app) as status_cl:
            stream_task = asyncio.create_task(
                collect_sse_events(stream_cl, chat_id)
            )

            # Poll for running=True (the route handler sets the lock before
            # returning; polling handles any startup delay).
            deadline = time.monotonic() + 5.0
            running = False
            while time.monotonic() < deadline:
                resp = await status_cl.get(f"/chat/{chat_id}/stream/status")
                assert resp.status_code == 200
                status = resp.json()
                if status["running"]:
                    running = True
                    break
                await asyncio.sleep(0.05)

            assert running is True, "Timed out waiting for running=True"

            # Release the LLM so the run completes and the lock is released.
            llm.release(0)
            await stream_task

        async with _client(app) as status_cl:
            resp = await status_cl.get(f"/chat/{chat_id}/stream/status")
            assert resp.status_code == 200
            status = resp.json()
            assert status["running"] is False
            assert status["resumable"] is True


# =============================================================================
# Lock & heartbeat resilience (slow / timing-sensitive)
# =============================================================================


@pytest.mark.slow
class TestLockAndHeartbeat:
    """Lock refresh, heartbeat emission, and vanished-producer detection."""

    @pytest.mark.asyncio
    async def test_consumer_emits_heartbeat_when_idle(self, seeded_chat, redis_client, redis_keys):
        """Consumer emits a ``heartbeat`` after ``_STREAM_HEARTBEAT_MS`` of
        idleness.  We make the LLM take a while to produce its first event
        (via a ``pre`` gate released after the heartbeat interval), so the
        consumer idles long enough."""
        chat_id, _user_id, model_id = seeded_chat
        # Gate at "pre" so the producer doesn't write anything for a while
        llm = GatedRecordingLLM([("text", "X.")], model_id)
        llm.hold(0, at="pre")

        app = _build_chat_app(llm, redis_client, model_id)
        async with _client(app) as client:
            stream_task = asyncio.create_task(
                collect_sse_events(client, chat_id)
            )

            # Wait > heartbeat interval while LLM is gated and stream is
            # empty.  During this time the consumer should fire a heartbeat.
            await asyncio.sleep(_FAST_HEARTBEAT_MS / 1000 + 0.5)

            # Now release the LLM so it writes events and the stream
            # completes.
            llm.release(0)

            events = await stream_task

        event_types = [et for et, _d, _sid in events]
        assert "heartbeat" in event_types, f"No heartbeat found in events: {event_types}"

    @pytest.mark.asyncio
    async def test_lock_refresh_keeps_lock_alive_past_ttl(
        self, seeded_chat, redis_client, redis_keys, monkeypatch
    ):
        """Lock refresh task keeps the producer lock alive while the LLM is
        silent longer than ``_RUN_LOCK_TTL``.

        Strategy: gate the LLM at ``"pre"``, wait past the TTL (which would
        expire without the background refresh), then verify the lock key still
        exists.  Finally release the LLM and consume the stream normally.
        """
        chat_id, _user_id, model_id = seeded_chat
        # Override the _patch_timing values for this test: fast refresh so
        # the lock is renewed well within its short TTL.
        monkeypatch.setattr("streaming.run._LOCK_REFRESH_INTERVAL", 2)
        monkeypatch.setattr("streaming.run._RUN_LOCK_TTL", 5)
        monkeypatch.setattr("routers.chat._RUN_LOCK_TTL", 5)

        llm = GatedRecordingLLM([("text", "OK.")], model_id)
        llm.hold(0, at="pre")

        app = _build_chat_app(llm, redis_client, model_id)
        lock_key = f"chat:runlock:{chat_id}"

        async with _client(app) as client:
            stream_task = asyncio.create_task(
                collect_sse_events(client, chat_id)
            )

            # Wait for the lock to be acquired
            await asyncio.sleep(0.3)
            assert await redis_client.exists(lock_key), "Lock was not acquired"

            # Sleep longer than _RUN_LOCK_TTL (5 s).  Without the refresh
            # background task the lock would expire.  With it the lock stays
            # alive.
            await asyncio.sleep(6)
            assert await redis_client.exists(
                lock_key
            ), "Lock expired even with refresh task running"

            llm.release(0)
            await stream_task

    @pytest.mark.asyncio
    async def test_producer_crash_delivers_stream_error(
        self, seeded_chat, redis_client, redis_keys
    ):
        """The producer encounters an unexpected error while
        streaming → ``stream_error`` event is delivered to the consumer and
        the run lock is cleaned up."""
        import routers.chat as chat_module

        chat_id, _user_id, model_id = seeded_chat
        llm = GatedRecordingLLM(
            [("text", "Crash.")],
            model_id,
            fail_on_call=0,
            fail_exc=RuntimeError("Unexpected producer crash"),
        )

        app = _build_chat_app(llm, redis_client, model_id)
        lock_key = f"chat:runlock:{chat_id}"

        async with _client(app) as client:
            events = await collect_sse_events(client, chat_id)

        # Wait for the producer task to finish its cleanup
        producer_task = chat_module._run_tasks_by_chat.get(chat_id)
        if producer_task is not None:
            try:
                await asyncio.wait_for(producer_task, timeout=5)
            except (asyncio.CancelledError, Exception):
                pass

        event_types = [et for et, _d, _sid in events]
        assert "stream_error" in event_types, (
            f"Expected stream_error, got events: {event_types}"
        )

        # Lock should be cleaned up after the crash
        exists = await redis_client.exists(lock_key)
        assert exists == 0, "Lock was not cleaned up after crash"

    @pytest.mark.asyncio
    async def test_provider_error_stream_error_includes_metadata(
        self, seeded_chat, redis_client, redis_keys
    ):
        """ProviderError stream_error payloads keep provider/model/status details."""
        from providers.types import ProviderError, ProviderType

        chat_id, _user_id, model_id = seeded_chat
        llm = GatedRecordingLLM(
            [("text", "unused")],
            model_id,
            fail_on_call=0,
            fail_exc=ProviderError(
                "Rate limited",
                provider_type=ProviderType.ANTHROPIC,
                model=model_id,
                status_code=429,
            ),
        )

        app = _build_chat_app(llm, redis_client, model_id)
        async with _client(app) as client:
            events = await collect_sse_events(client, chat_id)

        stream_errors = [
            json.loads(data)
            for event_type, data, _sid in events
            if event_type == "stream_error"
        ]
        assert stream_errors, f"Expected stream_error, got events: {events}"
        assert stream_errors[-1] == {
            "message": "Failed to generate response: Rate limited",
            "provider": "anthropic",
            "model": model_id,
            "statusCode": 429,
        }


# =============================================================================
# /chat/{id}/stream/status with pending approval
# =============================================================================


class TestStreamStatusPending:
    """stream/status pending_approval/pending_oauth flags."""

    @pytest.mark.asyncio
    async def test_status_pending_approval_true(
        self, seeded_chat, redis_client, redis_keys
    ):
        """A pending ``ToolApproval`` row with a ``tool_call_id`` that
        matches an active-path ``tool_use`` → ``pending_approval=true``."""
        chat_id, _user_id, model_id = seeded_chat

        # Seed: user msg → assistant with tool_use → user tool_result
        msgs_repo = MessagesRepository()
        active = await msgs_repo.get_active_path(chat_id)
        parent_id = active[-1].id

        tool_use_id = "toolu_for_approval"
        assistant_msg = await msgs_repo.create(
            chat_id,
            {
                "role": "assistant",
                "content": [
                    {
                        "type": "tool_use",
                        "id": tool_use_id,
                        "name": "search_documents",
                        "input": {"query": "test"},
                    }
                ],
            },
            parent_id=parent_id,
        )

        # Pending approval row
        approvals_repo = ToolApprovalsRepository()
        await approvals_repo.create_pending(
            chat_id=chat_id,
            user_id=_user_id,
            tool_name="search_documents",
            tool_input={"query": "test"},
            tool_call_id=tool_use_id,
            approval_type=ToolApprovalType.APPROVAL,
        )

        # Check status without starting a stream
        app = FastAPI()
        app.state = AppState()
        app.state.redis_client = redis_client
        app.include_router(chat_router)

        async with _client(app) as client:
            resp = await client.get(f"/chat/{chat_id}/stream/status")
            assert resp.status_code == 200
            status = resp.json()
            assert status["pending_approval"] is True, (
                f"Expected pending_approval=True, got {status}"
            )
            assert status["pending_oauth"] is False
            assert status["resumable"] is False


# =============================================================================
# Partial assistant message utility
# =============================================================================


class TestPartialAssistant:
    """Unit-level tests for ``_partial_assistant_message``."""

    def test_partial_assistant_strips_empty_text_blocks(self):
        """Empty text blocks are removed; non-empty text blocks are kept.
        Tool inputs with string JSON are parsed to dict."""
        from streaming.persist import partial_assistant_message as _partial_assistant_message
        from anthropic.types import TextBlockParam, ToolUseBlockParam

        blocks: list[TextBlockParam | ToolUseBlockParam] = [
            TextBlockParam(type="text", text="   "),  # stripped
            TextBlockParam(type="text", text="Hello"),  # kept
            ToolUseBlockParam(
                type="tool_use",
                id="toolu_test",
                name="test_tool",
                input='{"key": "value"}',  # JSON string → parsed
            ),
        ]

        result = _partial_assistant_message(blocks)
        assert result is not None
        content = result["content"]
        assert len(content) == 2, f"Expected 2 blocks, got {len(content)}"
        assert content[0]["text"] == "Hello"
        assert content[1]["type"] == "tool_use"
        assert content[1]["input"] == {"key": "value"}

    def test_partial_assistant_returns_none_when_all_blocks_empty(self):
        """All-empty blocks → returns None."""
        from streaming.persist import partial_assistant_message as _partial_assistant_message
        from anthropic.types import TextBlockParam

        result = _partial_assistant_message([
            TextBlockParam(type="text", text=""),
            TextBlockParam(type="text", text="   "),
        ])
        assert result is None

    def test_partial_assistant_parses_empty_tool_input(self):
        """Empty string tool input becomes empty dict."""
        from streaming.persist import partial_assistant_message as _partial_assistant_message
        from anthropic.types import ToolUseBlockParam

        result = _partial_assistant_message([
            ToolUseBlockParam(
                type="tool_use",
                id="toolu_empty",
                name="empty_tool",
                input="",
            ),
        ])
        assert result is not None
        assert result["content"][0]["input"] == {}


# =============================================================================
# Drop empty assistant messages from history
# =============================================================================


class TestDropEmpty:
    """``_drop_empty_assistant_messages`` filters out empty assistant rows."""

    @pytest.mark.asyncio
    async def test_drop_empty_assistant_from_history_before_llm_call(
        self, seeded_chat, redis_client, redis_keys
    ):
        """DB history contains an empty assistant row → it is removed before
        the provider call so the LLM never sees it."""
        chat_id, _user_id, model_id = seeded_chat
        msgs_repo = MessagesRepository()

        # Get current user message and append empty assistant + another user msg
        active = await msgs_repo.get_active_path(chat_id)
        parent_id = active[-1].id

        empty_asst = await msgs_repo.create(
            chat_id,
            {"role": "assistant", "content": []},
            parent_id=parent_id,
        )
        await msgs_repo.create(
            chat_id,
            {"role": "user", "content": "Continue"},
            parent_id=empty_asst.id,
        )

        llm = GatedRecordingLLM([("text", "Continuing.")], model_id)

        app = _build_chat_app(llm, redis_client, model_id)
        async with _client(app) as client:
            await collect_sse_events(client, chat_id)

        # The empty assistant should have been dropped from the LLM's history
        assert len(llm.calls) == 1, f"Expected 1 LLM call, got {len(llm.calls)}"
        msgs_for_llm = llm.calls[0]["messages"]
        roles = [m["role"] for m in msgs_for_llm]
        assert roles == ["user", "user"], (
            f"Expected [user, user], got {roles}. "
            "Empty assistant was not dropped."
        )


# =============================================================================
# Reconnect while run is active
# =============================================================================


class TestReconnectActive:
    """Reconnect while the producer is still running (lock alive)."""

    MOCK_SEARCH_RESPONSE = SearchResponse(
        results=[
            SearchResult(
                document=Document(
                    id="doc_1", title="Result", content_type="text",
                    url="", source_type="test",
                ),
                highlights=["test highlight"],
                source_type="test",
            ),
        ],
        total_count=1,
        query_time_ms=5,
    )

    @pytest.mark.asyncio
    async def test_reconnect_while_active_replays_from_zero(
        self, seeded_chat, redis_client, redis_keys
    ):
        """Reconnect while run active, no ``Last-Event-ID`` (page-reload) →
        attaches and replays whole buffer from ``\"0\"``.

        Strategy: LLM emits a tool call → router executes it → tool handler
        blocks on a gate → producer stays alive with events in stream →
        Phase 2 reconnects and reads from "0" → release gate → tool
        completes → LLM finishes → terminal event.
        """
        chat_id, _user_id, model_id = seeded_chat

        # Gated searcher: blocks until we release it.
        # Must provide a ``.client`` attribute (used by ``McpCapabilityHandler``
        # and ``SkillHandler`` in the registry build).
        searcher_gate = asyncio.Event()

        async def gated_handle(_request):
            await searcher_gate.wait()
            return self.MOCK_SEARCH_RESPONSE

        gated_searcher = AsyncMock(spec=SearcherTool)
        gated_searcher.handle = gated_handle
        gated_searcher.client = AsyncMock()

        # Two-turn LLM: tool call then text
        llm = GatedRecordingLLM(
            [
                (
                    "tool_call",
                    {
                        "name": "search_documents",
                        "input": {"query": "test"},
                        "id": "toolu_b2_active",
                    },
                ),
                ("text", "Search complete."),
            ],
            model_id,
        )

        app = _build_chat_app(llm, redis_client, model_id)
        app.state.searcher_tool = gated_searcher

        async with _client(app) as stream_cl, _client(app) as ctrl_cl:
            # Phase 1: start the stream in a background task
            stream_task = asyncio.create_task(
                collect_sse_events(stream_cl, chat_id)
            )

            # Wait for the producer to write the tool-call events and
            # block on the searcher gate
            await asyncio.sleep(0.5)

            # Phase 2: reconnect without Last-Event-ID (page reload)
            reload_events = await collect_sse_events(ctrl_cl, chat_id)

            # Release the searcher gate so the producer finishes
            searcher_gate.set()
            await stream_task

        # Phase 2 should have received the first-turn events (tool call)
        # plus the remaining events and terminal
        reload_msg = [et for et, _d, _sid in reload_events if et == "message"]
        assert reload_msg, "No message events in reload stream"
        assert any(
            et in ("end_of_stream", "stream_error")
            for et, _d, _ in reload_events
        ), "No terminal event in reload"


# =============================================================================
# Cancel before tool execution
# =============================================================================


class TestCancelEarly:
    """Cancel happens between the LLM's tool_use and tool execution."""

    @pytest.mark.asyncio
    async def test_cancel_before_tool_execution_persists_partial_assistant(
        self, seeded_chat, redis_client, redis_keys
    ):
        """Cancel flag set after the LLM emits a tool_use but before the
        router executes the tool → partial assistant (with the tool_use) is
        persisted; no tool_result row is created.

        Strategy: gate the LLM at ``"pre"`` (no events yet), set cancel,
        then release.  The router processes events and hits the cancel
        check before tool execution.
        """
        chat_id, _user_id, model_id = seeded_chat

        # Use an inter-event delay so the router's cancel check (inside
        # the event loop, every ``_CANCEL_CHECK_INTERVAL_SECONDS``) has
        # time to detect the flag before the tool execution check.
        llm = GatedRecordingLLM(
            [
                (
                    "tool_call",
                    {
                        "name": "search_documents",
                        "input": {"query": "cancel me"},
                        "id": "toolu_c2",
                    },
                ),
            ],
            model_id,
            inter_event_delay=0.15,  # 150ms between events → total ~900ms
        )

        app = _build_chat_app(llm, redis_client, model_id)
        async with _client(app) as client:
            stream_task = asyncio.create_task(
                collect_sse_events(client, chat_id)
            )

            # Wait for the LLM to start producing events (message_start
            # through part of the tool_use content blocks)
            await asyncio.sleep(0.5)

            # Set cancel flag — the next cancel check inside the event
            # loop will detect it and break before tool execution.
            await redis_client.set(
                f"chat:cancel:{chat_id}", "1", ex=_FAST_LOCK_TTL
            )

            events = await stream_task

        # Should have received the tool_use events and a terminal
        msg_events = [
            json.loads(d) for et, d, _ in events
            if et == "message" and d
        ]
        tool_use_events = [
            e for e in msg_events
            if e.get("type") == "content_block_start"
            and e.get("content_block", {}).get("type") == "tool_use"
        ]
        assert tool_use_events, "No tool_use content_block_start in events"

        terminal = [
            et for et, _d, _ in events
            if et in ("end_of_stream", "stream_error")
        ]
        assert terminal, "No terminal event after cancel"

        # Check that the assistant message with tool_use was persisted
        db_msgs = await MessagesRepository().get_active_path(chat_id)
        assistant_msgs = [m for m in db_msgs if m.message["role"] == "assistant"]
        assert assistant_msgs, "No assistant message persisted"
        latest_asst = assistant_msgs[-1].message
        blocks = latest_asst.get("content", [])
        assert any(
            b.get("type") == "tool_use" for b in blocks
        ), "Assistant message lacks tool_use block"

        # No tool_result should be present because the tool was never
        # executed (cancel happened before execution)
        user_msgs = [m for m in db_msgs if m.message["role"] == "user"]
        for um in user_msgs:
            blocks = um.message.get("content", [])
            if isinstance(blocks, list):
                for b in blocks:
                    assert b.get("type") != "tool_result", (
                        "tool_result found in user message despite cancel before execution"
                    )


# =============================================================================
# Cancel mid-tool-call (immediate Stop during tool execution)
# =============================================================================


class TestCancelMidToolCall:
    """Immediate Stop (``task.cancel()``) while a tool is executing.

    Pins #362's stated goal: stopping mid-tool-call must end generation
    promptly without running further tools or LLM calls, and the prior
    turn's assistant message (with the tool_use block) must be finalized.
    """

    MOCK_SEARCH_RESPONSE = SearchResponse(
        results=[
            SearchResult(
                document=Document(
                    id="doc_1", title="Result", content_type="text",
                    url="", source_type="test",
                ),
                highlights=["test highlight"],
                source_type="test",
            ),
        ],
        total_count=1,
        query_time_ms=5,
    )

    @pytest.mark.asyncio
    async def test_task_cancel_mid_tool_call_ends_promptly(
        self, seeded_chat, redis_client, redis_keys
    ):
        """``task.cancel()`` during tool execution → no further LLM calls
        (no second turn), the prior turn's assistant message (with tool_use)
        is persisted.

        Strategy: multi-turn LLM (tool_call then text), gated searcher that
        blocks until released.  Start the stream, wait for the tool to start
        executing (blocked on the gate), then cancel the producer task.
        """

        chat_id, _user_id, model_id = seeded_chat

        # Gated searcher: blocks until cancelled/never released
        searcher_gate = asyncio.Event()

        async def gated_handle(_request):
            await searcher_gate.wait()
            return self.MOCK_SEARCH_RESPONSE

        gated_searcher = AsyncMock(spec=SearcherTool)
        gated_searcher.handle = gated_handle
        gated_searcher.client = AsyncMock()

        # Multi-turn LLM: tool_call then text (second call should NOT happen)
        llm = GatedRecordingLLM(
            [
                (
                    "tool_call",
                    {
                        "name": "search_documents",
                        "input": {"query": "test"},
                        "id": "toolu_mt_cancel",
                    },
                ),
                ("text", "This should NOT be reached."),
            ],
            model_id,
        )

        app = _build_chat_app(llm, redis_client, model_id)
        app.state.searcher_tool = gated_searcher

        async with _client(app) as client:
            stream_task = asyncio.create_task(
                collect_sse_events(client, chat_id)
            )

            # Wait for the LLM to emit tool_use and the router to start
            # executing the tool (which blocks on searcher_gate)
            await asyncio.sleep(0.5)

            # Cancel the producer task (immediate Stop)
            task = chat_module._run_tasks_by_chat.get(chat_id)
            assert task is not None, "Producer task not found"
            task.cancel()

            events = await stream_task

        # No further LLM calls beyond the first (tool_call) turn
        assert len(llm.calls) == 1, (
            f"Expected exactly 1 LLM call (tool_call), "
            f"got {len(llm.calls)}. A second turn was not prevented!"
        )

        # The stream terminated (end_of_stream from the cancel path)
        assert any(
            et == "end_of_stream" for et, _, _ in events
        ), f"No end_of_stream after mid-tool-call cancel. Events: {[et for et, _, _ in events]}"

        # The prior turn's assistant message with tool_use is persisted
        db_msgs = await MessagesRepository().get_active_path(chat_id)
        assistant_msgs = [m for m in db_msgs if m.message["role"] == "assistant"]
        assert assistant_msgs, (
            "No assistant message persisted after mid-tool-call cancel."
        )
        latest_asst = assistant_msgs[-1].message
        blocks = latest_asst.get("content", [])
        tool_use_blocks = [
            b for b in blocks
            if isinstance(b, dict) and b.get("type") == "tool_use"
        ]
        assert tool_use_blocks, (
            "Prior turn's tool_use was not persisted after mid-tool-call cancel."
        )

        # Exactly one assistant message — no duplicate rows from a
        # late CancelledError handler re-saving the same content_blocks.
        assert len(assistant_msgs) == 1, (
            f"Expected exactly 1 assistant message after mid-tool-call cancel, "
            f"got {len(assistant_msgs)}. A duplicate save was not prevented!"
        )

        # No tool_result should be present because the tool was never
        # executed to completion
        user_msgs = [m for m in db_msgs if m.message["role"] == "user"]
        for um in user_msgs:
            blocks = um.message.get("content", [])
            if isinstance(blocks, list):
                for b in blocks:
                    assert b.get("type") != "tool_result", (
                        "tool_result found in user message despite "
                        "tool being cancelled mid-execution"
                    )


# =============================================================================
# Racing connect (concurrent streams)
# =============================================================================


class TestRacingConnect:
    """Two concurrent ``GET /chat/{id}/stream`` requests — one wins the
    ``SET NX`` lock, the other attaches as a consumer.  No duplicate DB writes."""

    @pytest.mark.asyncio
    async def test_racing_connect_no_duplicate_db_writes(
        self, seeded_chat, redis_client, redis_keys
    ):
        chat_id, _user_id, model_id = seeded_chat
        llm = GatedRecordingLLM([("text", "Racing.")], model_id)
        llm.hold(0, at="pre")

        app = _build_chat_app(llm, redis_client, model_id)
        lock_key = f"chat:runlock:{chat_id}"

        async with _client(app) as client1, _client(app) as client2:
            # Stream 1: will acquire the lock and become producer
            stream1_task = asyncio.create_task(
                collect_sse_events(client1, chat_id)
            )

            # Wait for lock acquisition
            import time

            deadline = time.monotonic() + 10
            while time.monotonic() < deadline:
                if await redis_client.exists(lock_key):
                    break
                await asyncio.sleep(0.05)
            assert await redis_client.exists(lock_key), "Lock not acquired"

            # Stream 2: should detect ``run_active`` and attach as consumer
            stream2_task = asyncio.create_task(
                collect_sse_events(client2, chat_id)
            )

            # Let stream 2 attach before releasing
            await asyncio.sleep(0.3)
            llm.release(0)

            events1 = await stream1_task
            events2 = await stream2_task

        assert any(
            et in ("end_of_stream", "stream_error") for et, _, _ in events1
        ), "Stream 1 has no terminal"
        assert any(
            et in ("end_of_stream", "stream_error") for et, _, _ in events2
        ), "Stream 2 has no terminal"

        # Exactly one assistant message in the DB — no duplicate writes
        db_msgs = await MessagesRepository().get_active_path(chat_id)
        assistant_msgs = [m for m in db_msgs if m.message["role"] == "assistant"]
        assert len(assistant_msgs) == 1, (
            f"Expected 1 assistant message, got {len(assistant_msgs)}. "
            "A duplicate run was created!"
        )


# =============================================================================
# Empty-row deletion on early cancel
# =============================================================================


class TestEmptyRowDelete:
    """When the run is cancelled before any content block, the early-persisted
    empty assistant row is deleted from the DB."""

    @pytest.mark.asyncio
    async def test_early_cancel_deletes_empty_assistant_row(
        self, seeded_chat, redis_client, redis_keys
    ):
        """LLM yields ``message_start`` (which triggers an early-persisted
        assistant row), then cancel fires before any content block.  The
        empty row should be deleted, leaving zero assistant rows in the DB
        for this chat."""
        import routers.chat as chat_module

        chat_id, _user_id, model_id = seeded_chat

        # inter_event_delay > _CANCEL_CHECK_INTERVAL_SECONDS (0.5s) so the
        # cancel check fires between message_start and content_block_start.
        # Since the check runs BEFORE processing the event, content_blocks
        # stays empty and _partial_assistant_message returns None.
        llm = GatedRecordingLLM(
            [("text", "X.")], model_id, inter_event_delay=0.55
        )

        app = _build_chat_app(llm, redis_client, model_id)
        async with _client(app) as client:
            stream_task = asyncio.create_task(
                collect_sse_events(client, chat_id)
            )

            # Wait for ``message_start`` to be processed
            await asyncio.sleep(0.2)

            # Set cancel flag — the cancel check will fire when
            # content_block_start arrives (0.55s after message_start),
            # detect the flag, and break before processing the event.
            await redis_client.set(
                f"chat:cancel:{chat_id}", "1", ex=_FAST_LOCK_TTL
            )

            events = await stream_task

        # Wait for the producer task to finish its cleanup
        producer_task = chat_module._run_tasks_by_chat.get(chat_id)
        if producer_task is not None:
            try:
                await asyncio.wait_for(producer_task, timeout=5)
            except (asyncio.CancelledError, Exception):
                pass

        event_types = [et for et, _d, _sid in events]
        # Should only have message_start + end_of_stream, no content
        msg_count = sum(1 for et, _, _ in events if et == "message")
        assert msg_count == 1, (
            f"Expected 1 message event (message_start), got {msg_count}. "
            f"Event types: {event_types}"
        )

        db_msgs = await MessagesRepository().get_active_path(chat_id)
        assistant_msgs = [m for m in db_msgs if m.message["role"] == "assistant"]
        assert len(assistant_msgs) == 0, (
            f"Expected 0 assistant messages (empty row should be deleted), "
            f"got {len(assistant_msgs)}. "
            f"Event types: {event_types}"
        )


# =============================================================================
# Interrupted tool call repair
# =============================================================================


class TestInterruptedToolCall:
    """``_repair_interrupted_tool_calls`` injects ``is_error`` tool_results
    into the LLM history when the DB ends on an unanswered ``tool_use``."""

    @pytest.mark.asyncio
    async def test_repair_interrupted_tool_calls_injects_error_result(
        self, seeded_chat, redis_client, redis_keys
    ):
        """Seed the DB with: user → assistant (tool_use, no tool_result).
        The repair path should inject an ``is_error`` tool_result message
        before the LLM call, and the LLM should receive the repaired history."""
        chat_id, _user_id, model_id = seeded_chat
        msgs_repo = MessagesRepository()

        # Get current user message and append an assistant with an unanswered
        # tool_use (no corresponding tool_result from the user).
        active = await msgs_repo.get_active_path(chat_id)
        parent_id = active[-1].id

        tool_use_id = "toolu_g1_unanswered"
        await msgs_repo.create(
            chat_id,
            {
                "role": "assistant",
                "content": [
                    {
                        "type": "tool_use",
                        "id": tool_use_id,
                        "name": "search_documents",
                        "input": {"query": "interrupted"},
                    }
                ],
            },
            parent_id=parent_id,
        )

        llm = GatedRecordingLLM([("text", "Repaired.")], model_id)

        app = _build_chat_app(llm, redis_client, model_id)
        async with _client(app) as client:
            await collect_sse_events(client, chat_id)

        # The LLM should have been called with the repaired history
        assert len(llm.calls) == 1, f"Expected 1 LLM call, got {len(llm.calls)}"
        msgs_for_llm = llm.calls[0]["messages"]

        # The last user message should contain the injected tool_result
        last_user = [m for m in msgs_for_llm if m["role"] == "user"][-1]
        content = last_user.get("content", [])
        if isinstance(content, str):
            content = [{"type": "text", "text": content}]
        tool_results = [b for b in content if b.get("type") == "tool_result"]
        assert tool_results, (
            "No tool_result injected by repair path. "
            f"Content: {content}"
        )
        assert tool_results[0]["tool_use_id"] == tool_use_id
        assert tool_results[0]["is_error"] is True, (
            "Repair tool_result should have is_error=True"
        )


# =============================================================================
# Multi-turn (multiple agent iterations)
# =============================================================================


class TestMultiTurn:
    """The agent loop runs through multiple LLM + tool-execution iterations."""

    @pytest.mark.asyncio
    async def test_multi_turn_executes_tool_then_responds(
        self, seeded_chat, redis_client, redis_keys
    ):
        """LLM produces tool_call → router executes tool → LLM second call
        produces text → stream completes with terminal.  Each iteration
        creates a separate assistant message.
        """
        chat_id, _user_id, model_id = seeded_chat

        llm = GatedRecordingLLM(
            [
                (
                    "tool_call",
                    {
                        "name": "search_documents",
                        "input": {"query": "multi-turn"},
                        "id": "toolu_mt",
                    },
                ),
                ("text", "Here are the search results."),
            ],
            model_id,
        )

        app = _build_chat_app(llm, redis_client, model_id)
        async with _client(app) as client:
            events = await collect_sse_events(client, chat_id)

        assert any(
            et in ("end_of_stream", "stream_error") for et, _, _ in events
        ), "No terminal event"

        # Each iteration creates its own assistant message:
        # iteration 1 → tool_use block, iteration 2 → text block
        db_msgs = await MessagesRepository().get_active_path(chat_id)
        assistant_msgs = [m for m in db_msgs if m.message["role"] == "assistant"]
        assert len(assistant_msgs) == 2, (
            f"Expected exactly 2 assistant msgs (one per iteration), "
            f"got {len(assistant_msgs)}"
        )

        # At least one has a tool_use block
        assert any(
            any(b.get("type") == "tool_use" for b in m.message.get("content", []))
            for m in assistant_msgs
        ), "No assistant with tool_use block"

        # At least one has a text block
        assert any(
            any(b.get("type") == "text" for b in m.message.get("content", []))
            for m in assistant_msgs
        ), "No assistant with text block"

        # The searcher mock returns empty results (ECONNREFUSED), but the
        # tool executes and produces a tool_result.  The user's next message
        # should contain the tool_result.
        user_msgs = [m for m in db_msgs if m.message["role"] == "user"]
        user_with_tool_result = None
        for um in user_msgs:
            content = um.message.get("content", [])
            if isinstance(content, list):
                if any(b.get("type") == "tool_result" for b in content):
                    user_with_tool_result = um
                    break
        assert user_with_tool_result is not None, (
            "No user message with tool_result found. "
            "Tool was not executed."
        )


# =============================================================================
# Approval and OAuth intervention resume
# =============================================================================


class TestInterventionResume:
    @pytest.mark.asyncio
    async def test_approved_action_executes_once_and_completes_intervention(
        self, seeded_chat, redis_client, redis_keys, monkeypatch
    ):
        chat_id, user_id, model_id = seeded_chat
        handler = ScriptedActionHandler(
            requires_approval=True,
            results=[
                ToolResult(
                    content=[{"type": "text", "text": "Email sent"}],
                    is_error=False,
                )
            ],
        )
        _install_scripted_registry(monkeypatch, handler)
        llm = GatedRecordingLLM(
            [
                (
                    "tool_call",
                    {
                        "name": handler.tool_name,
                        "input": {"to": "person@example.com"},
                        "id": "toolu_approval",
                    },
                ),
                ("text", "The email was sent."),
            ],
            model_id,
        )
        app = _build_chat_app(llm, redis_client, model_id)

        async with _client(app) as client:
            paused_events = await collect_sse_events(client, chat_id)
            assert any(event_type == "approval_required" for event_type, _, _ in paused_events)
            assert handler.executions == []

            approvals = await _interventions(
                chat_id,
                ToolApprovalType.APPROVAL,
                {ToolApprovalStatus.PENDING},
            )
            assert len(approvals) == 1
            await ToolApprovalsRepository().update_status(
                approvals[0].id, ToolApprovalStatus.APPROVED, user_id
            )

            resumed_events = await collect_sse_events(client, chat_id)

        assert handler.executions == [{"to": "person@example.com"}]
        assert any(event_type == "end_of_stream" for event_type, _, _ in resumed_events)
        completed = await _interventions(
            chat_id,
            ToolApprovalType.APPROVAL,
            {ToolApprovalStatus.COMPLETED},
        )
        assert [approval.id for approval in completed] == [approvals[0].id]

        active_path = await MessagesRepository().get_active_path(chat_id)
        tool_result_messages = [
            message
            for message in active_path
            if message.message["role"] == "user"
            and isinstance(message.message.get("content"), list)
            and any(
                block.get("type") == "tool_result"
                for block in message.message["content"]
            )
        ]
        assert len(tool_result_messages) == 1
        assert len(llm.calls) == 2
        assert llm.calls[1]["messages"][-1] == tool_result_messages[0].message

    @pytest.mark.asyncio
    async def test_denied_action_is_persisted_without_execution(
        self, seeded_chat, redis_client, redis_keys, monkeypatch
    ):
        chat_id, user_id, model_id = seeded_chat
        handler = ScriptedActionHandler(
            requires_approval=True,
            results=[ToolResult(content=[{"type": "text", "text": "unexpected"}])],
        )
        _install_scripted_registry(monkeypatch, handler)
        llm = GatedRecordingLLM(
            [
                (
                    "tool_call",
                    {
                        "name": handler.tool_name,
                        "input": {"to": "person@example.com"},
                        "id": "toolu_denied",
                    },
                ),
                ("text", "The email was not sent."),
            ],
            model_id,
        )
        app = _build_chat_app(llm, redis_client, model_id)

        async with _client(app) as client:
            await collect_sse_events(client, chat_id)
            approvals = await _interventions(
                chat_id,
                ToolApprovalType.APPROVAL,
                {ToolApprovalStatus.PENDING},
            )
            await ToolApprovalsRepository().update_status(
                approvals[0].id, ToolApprovalStatus.DENIED, user_id
            )
            await collect_sse_events(client, chat_id)

        assert handler.executions == []
        completed = await _interventions(
            chat_id,
            ToolApprovalType.APPROVAL,
            {ToolApprovalStatus.COMPLETED},
        )
        assert [approval.id for approval in completed] == [approvals[0].id]
        last_model_message = llm.calls[1]["messages"][-1]
        denial = last_model_message["content"][0]
        assert denial["type"] == "tool_result"
        assert denial["tool_use_id"] == "toolu_denied"
        assert denial["is_error"] is True
        assert "denied" in denial["content"][0]["text"].lower()

    @pytest.mark.asyncio
    async def test_approved_oauth_executes_once_then_completes_intervention(
        self, seeded_chat, redis_client, redis_keys, monkeypatch
    ):
        chat_id, user_id, model_id = seeded_chat
        oauth_payload = OAuthRequiredPayload(
            source_id="source-1",
            source_type="gmail",
            provider="google",
            oauth_start_url="/api/oauth/start?source_id=source-1",
        )
        handler = ScriptedActionHandler(
            requires_approval=False,
            results=[
                ToolResult(content=[], oauth_required=oauth_payload),
                ToolResult(content=[{"type": "text", "text": "Email sent"}]),
            ],
        )
        _install_scripted_registry(monkeypatch, handler)
        llm = GatedRecordingLLM(
            [
                (
                    "tool_call",
                    {
                        "name": handler.tool_name,
                        "input": {"to": "person@example.com"},
                        "id": "toolu_oauth",
                    },
                ),
                ("text", "The email was sent."),
            ],
            model_id,
        )
        app = _build_chat_app(llm, redis_client, model_id)

        async with _client(app) as client:
            paused_events = await collect_sse_events(client, chat_id)
            oauth_events = [
                json.loads(data)
                for event_type, data, _ in paused_events
                if event_type == "oauth_required"
            ]
            oauth_rows = await _interventions(
                chat_id,
                ToolApprovalType.OAUTH,
                {ToolApprovalStatus.PENDING},
            )
            assert len(oauth_rows) == 1
            assert oauth_events[0]["approval_id"] == oauth_rows[0].id
            await ToolApprovalsRepository().update_status(
                oauth_rows[0].id, ToolApprovalStatus.APPROVED, user_id
            )

            await collect_sse_events(client, chat_id)

        assert handler.executions == [
            {"to": "person@example.com"},
            {"to": "person@example.com"},
        ]
        completed = await _interventions(
            chat_id,
            ToolApprovalType.OAUTH,
            {ToolApprovalStatus.COMPLETED},
        )
        assert [approval.id for approval in completed] == [oauth_rows[0].id]
        assert len(llm.calls) == 2

    @pytest.mark.asyncio
    async def test_oauth_still_required_reuses_the_existing_row(
        self, seeded_chat, redis_client, redis_keys, monkeypatch
    ):
        chat_id, user_id, model_id = seeded_chat
        oauth_payload = OAuthRequiredPayload(
            source_id="source-1",
            source_type="gmail",
            provider="google",
            oauth_start_url="/api/oauth/start?source_id=source-1",
        )
        handler = ScriptedActionHandler(
            requires_approval=False,
            results=[ToolResult(content=[], oauth_required=oauth_payload)],
        )
        _install_scripted_registry(monkeypatch, handler)
        llm = GatedRecordingLLM(
            [
                (
                    "tool_call",
                    {
                        "name": handler.tool_name,
                        "input": {"to": "person@example.com"},
                        "id": "toolu_oauth_repeat",
                    },
                )
            ],
            model_id,
        )
        app = _build_chat_app(llm, redis_client, model_id)

        async with _client(app) as client:
            await collect_sse_events(client, chat_id)
            oauth_rows = await _interventions(
                chat_id,
                ToolApprovalType.OAUTH,
                {ToolApprovalStatus.PENDING},
            )
            await ToolApprovalsRepository().update_status(
                oauth_rows[0].id, ToolApprovalStatus.APPROVED, user_id
            )
            resumed_events = await collect_sse_events(client, chat_id)

        oauth_events = [
            json.loads(data)
            for event_type, data, _ in resumed_events
            if event_type == "oauth_required"
        ]
        assert oauth_events[0]["approval_id"] == oauth_rows[0].id
        all_oauth_rows = await _interventions(
            chat_id,
            ToolApprovalType.OAUTH,
            {ToolApprovalStatus.PENDING, ToolApprovalStatus.APPROVED},
        )
        assert [approval.id for approval in all_oauth_rows] == [oauth_rows[0].id]
        assert len(handler.executions) == 2
        assert len(llm.calls) == 1

    @pytest.mark.asyncio
    async def test_multi_tool_batch_persists_unblocked_results_then_resumes_only_blocker(
        self, seeded_chat, redis_client, redis_keys, monkeypatch
    ):
        chat_id, user_id, model_id = seeded_chat
        tool_names = {"test__a", "test__b", "test__c"}
        handler = MultiActionHandler(tool_names, {"test__b"})
        _install_scripted_registry(monkeypatch, handler)
        llm = GatedRecordingLLM(
            [
                (
                    "tool_calls",
                    [
                        {"name": "test__a", "input": {"value": "a"}, "id": "toolu_a"},
                        {"name": "test__b", "input": {"value": "b"}, "id": "toolu_b"},
                        {"name": "test__c", "input": {"value": "c"}, "id": "toolu_c"},
                    ],
                ),
                ("text", "All requested actions are complete."),
            ],
            model_id,
        )
        app = _build_chat_app(llm, redis_client, model_id)

        async with _client(app) as client:
            paused_events = await collect_sse_events(client, chat_id)
            assert any(event_type == "approval_required" for event_type, _, _ in paused_events)
            assert handler.executions == ["test__a", "test__c"]

            active_path = await MessagesRepository().get_active_path(chat_id)
            partial_results = [
                block
                for message in active_path
                if message.message["role"] == "user"
                and isinstance(message.message.get("content"), list)
                for block in message.message["content"]
                if block.get("type") == "tool_result"
            ]
            assert {result["tool_use_id"] for result in partial_results} == {
                "toolu_a",
                "toolu_c",
            }

            approvals = await _interventions(
                chat_id,
                ToolApprovalType.APPROVAL,
                {ToolApprovalStatus.PENDING},
            )
            assert [approval.tool_call_id for approval in approvals] == ["toolu_b"]
            await ToolApprovalsRepository().update_status(
                approvals[0].id, ToolApprovalStatus.APPROVED, user_id
            )
            await collect_sse_events(client, chat_id)

        assert handler.executions == ["test__a", "test__c", "test__b"]
        assert len(llm.calls) == 2
        logical_result_turn = llm.calls[1]["messages"][-1]
        assert logical_result_turn["role"] == "user"
        assert {
            block["tool_use_id"]
            for block in logical_result_turn["content"]
            if block.get("type") == "tool_result"
        } == {"toolu_a", "toolu_b", "toolu_c"}

    @pytest.mark.asyncio
    async def test_resume_repairs_unrelated_abandoned_call_instead_of_executing_it(
        self, seeded_chat, redis_client, redis_keys, monkeypatch
    ):
        chat_id, user_id, model_id = seeded_chat
        messages_repo = MessagesRepository()
        active_path = await messages_repo.get_active_path(chat_id)
        abandoned = await messages_repo.create(
            chat_id,
            {
                "role": "assistant",
                "content": [
                    {
                        "type": "tool_use",
                        "id": "toolu_abandoned",
                        "name": "test__abandoned",
                        "input": {},
                    }
                ],
            },
            parent_id=active_path[-1].id,
        )
        await messages_repo.create(
            chat_id,
            {
                "role": "assistant",
                "content": [
                    {
                        "type": "tool_use",
                        "id": "toolu_intervention",
                        "name": "test__intervention",
                        "input": {},
                    }
                ],
            },
            parent_id=abandoned.id,
        )
        approval = await ToolApprovalsRepository().create_pending(
            chat_id=chat_id,
            user_id=user_id,
            tool_name="test__intervention",
            tool_input={},
            tool_call_id="toolu_intervention",
        )
        await ToolApprovalsRepository().update_status(
            approval.id, ToolApprovalStatus.APPROVED, user_id
        )

        handler = MultiActionHandler(
            {"test__abandoned", "test__intervention"}, {"test__intervention"}
        )
        _install_scripted_registry(monkeypatch, handler)
        llm = GatedRecordingLLM([("text", "Intervention completed.")], model_id)
        app = _build_chat_app(llm, redis_client, model_id)

        async with _client(app) as client:
            await collect_sse_events(client, chat_id)

        assert handler.executions == ["test__intervention"]
        model_blocks = [
            block
            for message in llm.calls[0]["messages"]
            if message["role"] == "user" and isinstance(message.get("content"), list)
            for block in message["content"]
            if block.get("type") == "tool_result"
        ]
        abandoned_result = next(
            block for block in model_blocks if block["tool_use_id"] == "toolu_abandoned"
        )
        assert abandoned_result["is_error"] is True
        assert "interrupted" in abandoned_result["content"][0]["text"].lower()


# =============================================================================
# Agent chat path
# =============================================================================


class TestAgentChat:
    """The ``chat.agent_id != None`` branch: agent registry, system prompt,
    memory scoping, no approval flow."""

    @pytest.mark.asyncio
    async def test_agent_chat_uses_agent_system_prompt(
        self, db_pool, seeded_chat, redis_client, redis_keys
    ):
        """Agent chat creates a different registry and system prompt.  Basic
        streaming should still work end-to-end."""
        chat_id, user_id, model_id = seeded_chat

        # Create an agent
        agent_id = str(ULID())
        async with db_pool.acquire() as conn:
            await conn.execute(
                """INSERT INTO agents
                     (id, user_id, name, instructions, agent_type,
                      schedule_type, schedule_value, is_enabled, is_deleted)
                   VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9)""",
                agent_id,
                user_id,
                "Test Agent",
                "You are a test agent.",
                "user",
                "interval",
                "3600",
                True,
                False,
            )

        # Update the seeded chat to be an agent chat
        async with db_pool.acquire() as conn:
            await conn.execute(
                "UPDATE chats SET agent_id = $1 WHERE id = $2",
                agent_id,
                chat_id,
            )

        llm = GatedRecordingLLM([("text", "Agent response.")], model_id)

        app = _build_chat_app(llm, redis_client, model_id)
        async with _client(app) as client:
            events = await collect_sse_events(client, chat_id)

        assert any(
            et in ("end_of_stream", "stream_error") for et, _, _ in events
        ), "No terminal event in agent chat"

        msg_events = [et for et, _, _ in events if et == "message"]
        assert msg_events, "No message events in agent chat"

        db_msgs = await MessagesRepository().get_active_path(chat_id)
        assistant_msgs = [m for m in db_msgs if m.message["role"] == "assistant"]
        assert assistant_msgs, "No assistant message in agent chat DB"

        # The LLM's system prompt should contain agent instructions
        assert len(llm.calls) == 1, f"Expected 1 LLM call, got {len(llm.calls)}"
        system_prompt = llm.calls[0].get("system_prompt", "")
        assert "Test Agent" in system_prompt or "test agent" in system_prompt, (
            f"Agent instructions not found in system prompt: {system_prompt[:200]}"
        )


# =============================================================================
# Standalone endpoints (cancel API, context-overflow retry)
# =============================================================================


class TestStandaloneEndpoints:
    """Endpoint-level tests for non-streaming routes."""

    @pytest.mark.asyncio
    async def test_cancel_api_sets_redis_flag(
        self, seeded_chat, redis_client, redis_keys
    ):
        """``POST /chat/{id}/cancel`` sets the cancel flag."""
        chat_id, _user_id, model_id = seeded_chat

        app = FastAPI()
        app.state = AppState()
        app.state.redis_client = redis_client
        app.state.default_model_id = model_id
        app.state.searcher_tool = AsyncMock()
        app.include_router(chat_router)

        async with _client(app) as client:
            resp = await client.post(f"/chat/{chat_id}/cancel")
            assert resp.status_code == 200
            data = resp.json()
            assert data["status"] == "cancelling"

        # Verify the side effect: the Redis cancel flag was actually set,
        # not just an HTTP 200 returned.
        assert await redis_client.exists(f"chat:cancel:{chat_id}"), (
            "Cancel flag was not set in Redis"
        )

    @pytest.mark.asyncio
    async def test_context_overflow_triggers_compaction_retry(
        self, seeded_chat, redis_client, redis_keys
    ):
        """When the LLM raises ``ProviderError(is_context_overflow=True)``
        on the first call with no events emitted, the router retries once
        after forced compaction.  The second call succeeds."""
        from providers.types import ProviderError, ProviderType

        chat_id, _user_id, model_id = seeded_chat

        # The first LLM call raises context overflow; the second succeeds
        llm = GatedRecordingLLM(
            [("text", "Retried.")],
            model_id,
            fail_on_call=0,
            fail_exc=ProviderError(
                "Context window exceeded",
                provider_type=ProviderType.ANTHROPIC,
                is_context_overflow=True,
            ),
        )

        app = _build_chat_app(llm, redis_client, model_id)
        async with _client(app) as client:
            events = await collect_sse_events(client, chat_id)

        assert any(
            et in ("end_of_stream", "stream_error") for et, _, _ in events
        ), "No terminal event after context-overflow retry"

        # The LLM should have been called twice: first fails, second succeeds
        assert len(llm.calls) >= 2, (
            f"Expected ≥2 LLM calls for retry, got {len(llm.calls)}"
        )

        db_msgs = await MessagesRepository().get_active_path(chat_id)
        assistant_msgs = [m for m in db_msgs if m.message["role"] == "assistant"]
        assert assistant_msgs, "No assistant message persisted after retry"

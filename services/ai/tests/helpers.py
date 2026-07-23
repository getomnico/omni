"""Shared test helpers for integration tests.

Provides reusable DB data factories and mock LLM event generators
so that individual test files don't duplicate boilerplate.
"""

import asyncio
import json
from typing import Any
from unittest.mock import AsyncMock

from anthropic.types import (
    InputJSONDelta,
    Message,
    MessageDeltaUsage,
    RawContentBlockDeltaEvent,
    RawContentBlockStartEvent,
    RawContentBlockStopEvent,
    RawMessageDeltaEvent,
    RawMessageStartEvent,
    RawMessageStopEvent,
    TextBlock,
    TextDelta,
    ToolUseBlock,
    Usage,
)
from anthropic.types.raw_message_delta_event import Delta
from ulid import ULID

# =============================================================================
# DB data factories
# =============================================================================


async def create_test_user(db_pool, email_prefix: str = "test") -> tuple[str, str]:
    """Create a test user. Returns (user_id, email)."""
    user_id = str(ULID())
    email = f"{email_prefix}-{user_id}@example.com"
    async with db_pool.acquire() as conn:
        await conn.execute(
            """INSERT INTO users (id, email, password_hash)
               VALUES ($1, $2, $3)""",
            user_id,
            email,
            "hashed_password_placeholder",
        )
    return user_id, email


async def create_test_source(
    db_pool, user_id: str, source_type: str = "local_files"
) -> str:
    """Create a test source. Returns source_id."""
    source_id = str(ULID())
    async with db_pool.acquire() as conn:
        await conn.execute(
            """INSERT INTO sources (id, name, source_type, created_by)
               VALUES ($1, $2, $3, $4)""",
            source_id,
            "test-source",
            source_type,
            user_id,
        )
    return source_id


async def create_test_document_with_content(
    db_pool, source_id: str, content: str
) -> str:
    """Create a test document with a content blob (for embedding tests). Returns doc_id."""
    doc_id = str(ULID())
    content_id = str(ULID())
    content_bytes = content.encode("utf-8")
    async with db_pool.acquire() as conn:
        await conn.execute(
            """INSERT INTO content_blobs (id, content, size_bytes, storage_backend)
               VALUES ($1, $2, $3, 'postgres')""",
            content_id,
            content_bytes,
            len(content_bytes),
        )
        await conn.execute(
            """INSERT INTO documents (id, source_id, external_id, title, content_id, content)
               VALUES ($1, $2, $3, $4, $5, $6)""",
            doc_id,
            source_id,
            f"test-{doc_id}",
            "Test Document",
            content_id,
            content,
        )
    return doc_id


async def create_test_document(
    db_pool,
    source_id: str,
    title: str,
    content: str,
    permissions: dict | None = None,
    content_type: str | None = None,
    content_id: str | None = None,
    external_id: str | None = None,
) -> str:
    """Create a test document (for search tests). Returns doc_id.

    Optionally sets content_type and content_id; if content_id is provided,
    a matching content_blobs row is inserted so the document resolves to
    text content. external_id defaults to ext-{doc_id} when omitted.
    """
    doc_id = str(ULID())
    async with db_pool.acquire() as conn:
        if content_id is not None:
            content_bytes = content.encode("utf-8")
            await conn.execute(
                """INSERT INTO content_blobs (id, content, size_bytes, storage_backend)
                   VALUES ($1, $2, $3, 'postgres')""",
                content_id,
                content_bytes,
                len(content_bytes),
            )
        await conn.execute(
            """INSERT INTO documents
                 (id, source_id, external_id, title, content, permissions,
                  content_type, content_id)
               VALUES ($1, $2, $3, $4, $5, $6::jsonb, $7, $8)""",
            doc_id,
            source_id,
            external_id if external_id is not None else f"ext-{doc_id}",
            title,
            content,
            json.dumps(permissions or {}),
            content_type,
            content_id,
        )
    return doc_id


async def enqueue_document(db_pool, document_id: str) -> str:
    """Add document to embedding queue. Returns queue item ID."""
    item_id = str(ULID())
    async with db_pool.acquire() as conn:
        await conn.execute(
            """INSERT INTO embedding_queue (id, document_id, status)
               VALUES ($1, $2, 'pending')""",
            item_id,
            document_id,
        )
    return item_id


# =============================================================================
# Mock LLM event generators
# =============================================================================


def message_start_event():
    """A standard RawMessageStartEvent."""
    return RawMessageStartEvent(
        type="message_start",
        message=Message(
            id="msg_test",
            content=[],
            model="mock",
            role="assistant",
            stop_reason=None,
            stop_sequence=None,
            type="message",
            usage=Usage(input_tokens=10, output_tokens=0),
        ),
    )


def tool_call_events(
    tool_call_json: dict[str, Any],
    tool_name: str = "search_documents",
    tool_id: str = "toolu_test",
):
    """Yield Anthropic SDK events simulating a tool_use content block."""
    yield message_start_event()
    yield RawContentBlockStartEvent(
        type="content_block_start",
        index=0,
        content_block=ToolUseBlock(
            type="tool_use",
            id=tool_id,
            name=tool_name,
            input={},
        ),
    )
    yield RawContentBlockDeltaEvent(
        type="content_block_delta",
        index=0,
        delta=InputJSONDelta(
            type="input_json_delta",
            partial_json=json.dumps(tool_call_json),
        ),
    )
    yield RawContentBlockStopEvent(type="content_block_stop", index=0)
    yield RawMessageDeltaEvent(
        type="message_delta",
        delta=Delta(stop_reason="tool_use", stop_sequence=None),
        usage=MessageDeltaUsage(output_tokens=30),
    )
    yield RawMessageStopEvent(type="message_stop")


def multi_tool_call_events(tool_calls: list[dict[str, Any]]):
    """Yield one assistant turn containing multiple parallel tool calls."""
    yield message_start_event()
    for index, tool_call in enumerate(tool_calls):
        yield RawContentBlockStartEvent(
            type="content_block_start",
            index=index,
            content_block=ToolUseBlock(
                type="tool_use",
                id=tool_call["id"],
                name=tool_call["name"],
                input={},
            ),
        )
        yield RawContentBlockDeltaEvent(
            type="content_block_delta",
            index=index,
            delta=InputJSONDelta(
                type="input_json_delta",
                partial_json=json.dumps(tool_call["input"]),
            ),
        )
        yield RawContentBlockStopEvent(type="content_block_stop", index=index)
    yield RawMessageDeltaEvent(
        type="message_delta",
        delta=Delta(stop_reason="tool_use", stop_sequence=None),
        usage=MessageDeltaUsage(output_tokens=30),
    )
    yield RawMessageStopEvent(type="message_stop")


def text_response_events(text: str):
    """Yield Anthropic SDK events simulating a text response."""
    yield message_start_event()
    yield RawContentBlockStartEvent(
        type="content_block_start",
        index=0,
        content_block=TextBlock(type="text", text=""),
    )
    yield RawContentBlockDeltaEvent(
        type="content_block_delta",
        index=0,
        delta=TextDelta(type="text_delta", text=text),
    )
    yield RawContentBlockStopEvent(type="content_block_stop", index=0)
    yield RawMessageDeltaEvent(
        type="message_delta",
        delta=Delta(stop_reason="end_turn", stop_sequence=None),
        usage=MessageDeltaUsage(output_tokens=10),
    )
    yield RawMessageStopEvent(type="message_stop")


def create_mock_llm(
    tool_call_json: dict[str, Any],
    response_text: str = "Here are the results.",
    tool_name: str = "search_documents",
):
    """Return a mock LLMProvider: call 1 = tool call, call 2+ = text response."""
    call_count = 0

    async def stream_response(*_args, **_kwargs):
        nonlocal call_count
        call_count += 1
        if call_count == 1:
            for evt in tool_call_events(tool_call_json, tool_name=tool_name):
                yield evt
        else:
            for evt in text_response_events(response_text):
                yield evt

    provider = AsyncMock()
    provider.stream_response = stream_response
    provider.health_check.return_value = True
    return provider


def create_mock_llm_multi(
    responses: list[tuple[str, Any]],
):
    """Return a mock LLM with explicit per-call responses.

    Each entry is ("tool_call", {json}) or ("text", "response text").
    """
    call_count = 0

    async def stream_response(*_args, **_kwargs):
        nonlocal call_count
        idx = min(call_count, len(responses) - 1)
        call_count += 1
        kind, data = responses[idx]
        if kind == "empty":
            yield message_start_event()
            yield RawMessageStopEvent(type="message_stop")
        elif kind == "tool_call":
            for evt in tool_call_events(data):
                yield evt
        else:
            for evt in text_response_events(data):
                yield evt

    provider = AsyncMock()
    provider.stream_response = stream_response
    provider.health_check.return_value = True
    return provider


# =============================================================================
# SSE streaming helpers
# =============================================================================


async def stream_sse(
    client,
    chat_id: str,
    headers: dict[str, str] | None = None,
):
    """Async generator yielding (event_type, data, sse_id) triples from the
    chat SSE endpoint using httpx streaming, so the caller can react to events
    as they arrive (e.g. fire a POST /cancel mid-stream).

    Usage::

        async for event_type, data, sse_id in stream_sse(client, chat_id):
            if event_type == "message":
                payload = json.loads(data)
                if payload.get("type") == "message_stop":
                    break

    Args:
        client: An ``httpx.AsyncClient`` (with ``ASGITransport`` wired to the
            test app).
        chat_id: The chat ID.
        headers: Optional request headers (e.g. ``{"Last-Event-ID": "1234-0"}``).

    Yields:
        ``(event_type: str | None, data: str, sse_id: str | None)``. The
        ``sse_id`` is the ``id:`` prefix from the consumer's Redis-stream
        entry (``None`` for heartbeats and synthetic events).
    """
    url = f"/chat/{chat_id}/stream"
    async with client.stream("GET", url, headers=headers) as response:
        assert response.status_code == 200, f"Expected 200, got {response.status_code}"
        lines: list[str] = []
        sse_id: str | None = None
        async for raw_line in response.aiter_lines():
            line = raw_line.rstrip("\r")
            if line == "":
                if not lines:
                    continue
                event_type: str | None = None
                data_parts: list[str] = []
                for ln in lines:
                    if ln.startswith("id: "):
                        sse_id = ln[4:].strip()
                    elif ln.startswith("event: "):
                        event_type = ln[7:].strip()
                    elif ln.startswith("data: "):
                        data_parts.append(ln[6:])
                data = "\n".join(data_parts)
                yield (event_type, data, sse_id)
                lines = []
                continue
            lines.append(line)


async def collect_sse_events(
    client,
    chat_id: str,
    headers: dict[str, str] | None = None,
    timeout: float = 30,
) -> list[tuple[str | None, str, str | None]]:
    """Convenience helper: read all SSE events from the stream until a
    terminal event (``end_of_stream``, ``stream_error``, ``not_resumable``) is
    encountered, then return the complete list.

    Use this for simple assertion-based tests that don't need to interleave
    other actions (like ``POST /cancel``) while the stream is active.
    """
    events: list[tuple[str | None, str, str | None]] = []
    async for event_type, data, sse_id in stream_sse(client, chat_id, headers=headers):
        events.append((event_type, data, sse_id))
        if event_type in ("end_of_stream", "stream_error", "not_resumable"):
            break
    return events


def assert_sse_wire(events: list[tuple[str | None, str, str | None]]) -> None:
    """Validate the SSE wire format of a list of consumer events.

    Checks:
    * Every event from the Redis stream carries a non-None ``sse_id``.
      Heartbeats (spontaneous without an ``id:`` line) are exempt because
      ``_consume_run`` yields them directly, not from a Redis stream entry.
    * ``sse_id`` values are strictly increasing (when present).
    * Every event has at least an ``event_type`` and a ``data`` value.
    """
    seen_ids: list[str] = []
    for idx, (event_type, data, sse_id) in enumerate(events):
        assert event_type is not None, (
            f"Event {idx} is missing event: field. data={data!r}"
        )
        assert data is not None, (
            f"Event {idx} (type={event_type}) has no data: line"
        )
        if event_type == "heartbeat":
            # Heartbeats are synthetic and don't carry an id: line.
            continue
        assert (
            sse_id is not None
        ), f"Event {idx} (type={event_type}) missing id: line. data={data!r}"
        seen_ids.append(sse_id)
    # Verify monotonic ordering
    for i in range(1, len(seen_ids)):
        assert seen_ids[i] > seen_ids[i - 1], (
            f"sse_id not strictly increasing at position {i}: "
            f"{seen_ids[i - 1]} -> {seen_ids[i]}"
        )


# =============================================================================
# Gated mock LLM
# =============================================================================


class GatedRecordingLLM:
    """Mock LLMProvider that records every call's kwargs and can be gated with
    ``asyncio.Event`` per call.

    Each response entry follows the same convention as ``create_mock_llm_multi``:

    * ``("empty", None)``
    * ``("text", "response string")``
    * ``("tool_call", {"name": ..., "input": ..., "id": ...})``

    Gates are **open by default** (the call proceeds immediately).  Call
    ``hold(call_idx, at="pre")`` *before starting the stream* to close the
    gate for a specific call index; ``release(call_idx)`` opens it again.

    ``at="pre"`` blocks before yielding any SDK events; ``at="post"`` blocks
    after yielding all events.  ``at="pre"`` is what you want to make the LLM
    "hang" at the start of generation; ``at="post"`` is useful for idle-window
    scenarios where you need the agent loop to pause after a turn.
    """

    PERSISTED_BLOCK_EXTRAS: tuple[str, ...] = ()
    model_name = "gated-test"
    provider_type = "test"

    @property
    def supports_citations(self) -> bool:
        return False

    def __init__(
        self,
        responses: list[tuple[str, Any]],
        model_record_id: str,
        *,
        inter_event_delay: float = 0.0,
        fail_on_call: int | None = None,
        fail_exc: BaseException | None = None,
    ) -> None:
        self.responses = responses
        self.model_record_id = model_record_id
        self._inter_event_delay = inter_event_delay
        self._fail_on_call = fail_on_call
        self._fail_exc = fail_exc or Exception("GatedRecordingLLM simulated failure")
        # Recorded kwargs for every call to stream_response.
        self.calls: list[dict[str, Any]] = []
        # call_idx -> "pre" | "post"
        self._held: dict[int, str] = {}
        self._gates: dict[int, asyncio.Event] = {}

    def hold(self, call_idx: int, at: str = "pre") -> None:
        """Gate the call at ``call_idx`` so it blocks at ``at``.

        Must be called **before** the stream starts (or at least before the
        gate is awaited by the producer).
        """
        self._held[call_idx] = at

    def release(self, call_idx: int) -> None:
        """Open the gate for the call, letting it proceed."""
        gate = self._gates.get(call_idx)
        if gate is not None:
            gate.set()

    async def stream_response(self, **kwargs):
        call_idx = len(self.calls)
        # Record BEFORE the fail check so the retry counter is accurate
        captured = {}
        for k, v in kwargs.items():
            if isinstance(v, (list, dict)):
                captured[k] = json.loads(json.dumps(v))
            else:
                captured[k] = v
        self.calls.append(captured)
        if call_idx == self._fail_on_call:
            raise self._fail_exc

        if call_idx in self._held and self._held[call_idx] == "pre":
            gate = asyncio.Event()
            self._gates[call_idx] = gate
            await gate.wait()

        idx = min(call_idx, len(self.responses) - 1)
        kind, payload = self.responses[idx]
        if kind == "empty":
            yield message_start_event()
            yield RawMessageStopEvent(type="message_stop")
        elif kind == "tool_call":
            for event in tool_call_events(
                payload["input"],
                tool_name=payload.get("name", "search_documents"),
                tool_id=payload.get("id", f"toolu_{call_idx}"),
            ):
                yield event
                if self._inter_event_delay:
                    await asyncio.sleep(self._inter_event_delay)
        elif kind == "tool_calls":
            for event in multi_tool_call_events(payload):
                yield event
                if self._inter_event_delay:
                    await asyncio.sleep(self._inter_event_delay)
        else:
            for event in text_response_events(payload):
                yield event
                if self._inter_event_delay:
                    await asyncio.sleep(self._inter_event_delay)

        if call_idx in self._held and self._held[call_idx] == "post":
            gate = asyncio.Event()
            self._gates[call_idx] = gate
            await gate.wait()

    async def health_check(self) -> bool:
        return True

    async def get_context_window_tokens(self):
        from providers import SAFE_DEFAULT_CONTEXT_WINDOW_TOKENS, ContextWindowInfo

        return ContextWindowInfo(
            tokens=SAFE_DEFAULT_CONTEXT_WINDOW_TOKENS, source="safe_default"
        )

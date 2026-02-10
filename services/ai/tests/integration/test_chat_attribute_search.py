"""Integration tests for attribute_filters flowing through the chat SSE stream.

Validates that when the LLM emits a search_documents tool call with `attributes`,
the chat handler correctly maps them to `SearchRequest.attribute_filters` and
passes them to the searcher.

Uses real DB (testcontainers ParadeDB) for chat/message storage,
mock LLM that emits Anthropic SDK event objects, and a mock searcher
that captures the SearchRequest for assertion.
"""

import json
from typing import Any
from unittest.mock import AsyncMock

import pytest
from fastapi import FastAPI
from httpx import ASGITransport, AsyncClient

from anthropic.types import (
    RawMessageStartEvent,
    RawContentBlockStartEvent,
    RawContentBlockDeltaEvent,
    RawContentBlockStopEvent,
    RawMessageStopEvent,
    RawMessageDeltaEvent,
    Message,
    Usage,
    TextBlock,
    ToolUseBlock,
    InputJSONDelta,
    TextDelta,
    MessageDeltaUsage,
)
from anthropic.types.raw_message_delta_event import Delta
from ulid import ULID

from db import UsersRepository, ChatsRepository, MessagesRepository
import db.connection
from routers import chat_router
from state import AppState
from tools import SearchResponse, SearchResult
from tools.searcher_client import Document

pytestmark = pytest.mark.integration


# ---------------------------------------------------------------------------
# Mock LLM helpers
# ---------------------------------------------------------------------------


def _message_start_event():
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


def _tool_call_events(tool_call_json: dict[str, Any]):
    """Yield Anthropic SDK events simulating a tool_use content block."""
    yield _message_start_event()
    yield RawContentBlockStartEvent(
        type="content_block_start",
        index=0,
        content_block=ToolUseBlock(
            type="tool_use",
            id="toolu_attr_test",
            name="search_documents",
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


def _text_response_events(text: str):
    """Yield Anthropic SDK events simulating a final text response."""
    yield _message_start_event()
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
    tool_call_json: dict[str, Any], response_text: str = "Here are the results."
):
    """Return a mock LLMProvider whose stream_response yields tool call then text."""
    call_count = 0

    async def stream_response(*_args, **_kwargs):
        nonlocal call_count
        call_count += 1
        if call_count == 1:
            for evt in _tool_call_events(tool_call_json):
                yield evt
        else:
            for evt in _text_response_events(response_text):
                yield evt

    provider = AsyncMock()
    provider.stream_response = stream_response
    provider.health_check.return_value = True
    return provider


# ---------------------------------------------------------------------------
# Mock searcher
# ---------------------------------------------------------------------------

MOCK_SEARCH_RESPONSE = SearchResponse(
    results=[
        SearchResult(
            document=Document(
                id="doc_1",
                title="PROJ-101: Fix login bug",
                content_type="jira_issue",
                url="https://jira.example.com/browse/PROJ-101",
                source_type="jira",
            ),
            highlights=["Users cannot login when priority is High"],
        ),
        SearchResult(
            document=Document(
                id="doc_2",
                title="PROJ-202: Crash on startup",
                content_type="jira_issue",
                url="https://jira.example.com/browse/PROJ-202",
                source_type="jira",
            ),
            highlights=["Application crashes on startup for critical bugs"],
        ),
    ],
    total_count=2,
    query_time_ms=42,
)


def create_mock_searcher():
    """Return a mock SearcherTool that captures the SearchRequest."""
    searcher = AsyncMock()
    searcher.handle.return_value = MOCK_SEARCH_RESPONSE
    return searcher


# ---------------------------------------------------------------------------
# SSE parsing
# ---------------------------------------------------------------------------


def parse_sse_events(body: str) -> list[tuple[str, str]]:
    """Parse SSE text into list of (event_type, data) tuples."""
    events = []
    current_event = None
    current_data_lines: list[str] = []

    for line in body.split("\n"):
        if line.startswith("event: "):
            current_event = line[len("event: ") :]
        elif line.startswith("data: "):
            current_data_lines.append(line[len("data: ") :])
        elif line == "" and current_event is not None:
            events.append((current_event, "\n".join(current_data_lines)))
            current_event = None
            current_data_lines = []

    if current_event is not None:
        events.append((current_event, "\n".join(current_data_lines)))

    return events


# ---------------------------------------------------------------------------
# Fixtures
# ---------------------------------------------------------------------------


@pytest.fixture
async def chat_with_message(db_pool):
    """Create a user, chat, and user message in the real DB, return (chat_id, user_id)."""
    users_repo = UsersRepository(pool=db_pool)
    user = await users_repo.create(
        email=f"{ULID()}@test.local",
        password_hash="not-a-real-hash",
        full_name="Test User",
    )

    chats_repo = ChatsRepository(pool=db_pool)
    chat = await chats_repo.create(user_id=user.id)

    messages_repo = MessagesRepository(pool=db_pool)
    await messages_repo.create(
        chat_id=chat.id,
        message={"role": "user", "content": "Find all high-priority bugs"},
    )
    return chat.id, user.id


@pytest.fixture
def _patch_db_pool(db_pool, monkeypatch):
    """Point the global _db_pool at the test pool so ChatsRepository()/MessagesRepository() work."""
    monkeypatch.setattr(db.connection, "_db_pool", db_pool)


async def _stream_chat(app: FastAPI, chat_id: str) -> str:
    """Hit the SSE endpoint and return the full response body."""
    async with AsyncClient(
        transport=ASGITransport(app=app), base_url="http://test"
    ) as client:
        resp = await client.get(f"/chat/{chat_id}/stream", timeout=30)
        assert resp.status_code == 200
        return resp.text


def _build_app(llm_provider, searcher_tool) -> FastAPI:
    app = FastAPI()
    app.state = AppState()
    app.state.llm_provider = llm_provider
    app.state.searcher_tool = searcher_tool
    app.include_router(chat_router)
    return app


# ---------------------------------------------------------------------------
# Tests
# ---------------------------------------------------------------------------


@pytest.mark.asyncio
async def test_attribute_filters_flow_to_searcher(
    db_pool, chat_with_message, _patch_db_pool
):
    """attribute_filters from LLM tool call reach SearchRequest."""
    chat_id, _ = chat_with_message
    tool_call_json = {
        "query": "high priority bugs",
        "attributes": {"priority": "High", "issue_type": "Bug"},
    }
    searcher = create_mock_searcher()
    app = _build_app(create_mock_llm(tool_call_json), searcher)

    await _stream_chat(app, chat_id)

    searcher.handle.assert_called_once()
    captured_request = searcher.handle.call_args[0][0]
    assert captured_request.attribute_filters == {
        "priority": "High",
        "issue_type": "Bug",
    }


@pytest.mark.asyncio
async def test_no_attributes_sends_none(db_pool, chat_with_message, _patch_db_pool):
    """When the LLM omits attributes, attribute_filters should be None."""
    chat_id, _ = chat_with_message
    tool_call_json = {"query": "recent documents"}
    searcher = create_mock_searcher()
    app = _build_app(create_mock_llm(tool_call_json), searcher)

    await _stream_chat(app, chat_id)

    searcher.handle.assert_called_once()
    captured_request = searcher.handle.call_args[0][0]
    assert captured_request.attribute_filters is None


@pytest.mark.asyncio
async def test_array_attribute_filter(db_pool, chat_with_message, _patch_db_pool):
    """Array-valued attributes pass through for OR matching."""
    chat_id, _ = chat_with_message
    tool_call_json = {
        "query": "critical issues",
        "attributes": {"priority": ["High", "Critical"]},
    }
    searcher = create_mock_searcher()
    app = _build_app(create_mock_llm(tool_call_json), searcher)

    await _stream_chat(app, chat_id)

    searcher.handle.assert_called_once()
    captured_request = searcher.handle.call_args[0][0]
    assert captured_request.attribute_filters == {"priority": ["High", "Critical"]}


@pytest.mark.asyncio
async def test_stream_completes_with_tool_results(
    db_pool, chat_with_message, _patch_db_pool
):
    """Full SSE stream contains tool call events, save_message, tool_result, text, and end_of_stream."""
    chat_id, _ = chat_with_message
    tool_call_json = {
        "query": "high priority bugs",
        "attributes": {"priority": "High"},
    }
    response_text = "I found 2 high-priority bugs."
    searcher = create_mock_searcher()
    app = _build_app(create_mock_llm(tool_call_json, response_text), searcher)

    body = await _stream_chat(app, chat_id)
    events = parse_sse_events(body)
    event_types = [e[0] for e in events]

    # The stream should follow this pattern:
    # message (x6 for tool call) -> save_message -> message (tool_result) -> save_message
    # -> message (x6 for text response) -> save_message -> end_of_stream
    assert "save_message" in event_types
    assert "end_of_stream" in event_types

    # Verify tool_result event contains our canned document data
    tool_result_events = [
        (t, d) for t, d in events if t == "message" and "tool_result" in d
    ]
    assert len(tool_result_events) >= 1
    tool_result_data = json.loads(tool_result_events[0][1])
    assert tool_result_data["type"] == "tool_result"
    assert tool_result_data["is_error"] is False

    # Verify the final text appears in the stream
    text_deltas = [d for t, d in events if t == "message" and response_text in d]
    assert len(text_deltas) >= 1

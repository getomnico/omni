"""Integration tests for search_documents tool calls flowing through the chat SSE stream.

Validates that the chat handler correctly maps LLM tool calls to SearchRequest
and passes them to the searcher. Filters are now expressed via inline query
operators (e.g., "status:done in:jira") rather than separate tool parameters.

Uses real DB (testcontainers ParadeDB) for chat/message storage,
mock LLM that emits Anthropic SDK event objects, and a mock searcher
that captures the SearchRequest for assertion.
"""

import json
from unittest.mock import AsyncMock

import pytest
from fastapi import FastAPI
from httpx import ASGITransport, AsyncClient
from ulid import ULID

from .llm_helpers import create_mock_llm, parse_sse_events

from db import UsersRepository, ChatsRepository, MessagesRepository
import db.connection
from routers import chat_router
from state import AppState
from tools import SearchResponse, SearchResult
from tools.searcher_client import Document
from tests.helpers import create_mock_llm

pytestmark = pytest.mark.integration


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
# Fixtures
# ---------------------------------------------------------------------------


@pytest.fixture
async def test_model(db_pool) -> str:
    """Create a model provider and model in the DB, return the model row ID."""
    async with db_pool.acquire() as conn:
        provider_id = str(ULID())
        await conn.execute(
            "INSERT INTO model_providers (id, name, provider_type, config) VALUES ($1, $2, $3, $4)",
            provider_id,
            "Test Provider",
            "anthropic",
            "{}",
        )
        model_id = str(ULID())
        await conn.execute(
            "INSERT INTO models (id, model_provider_id, model_id, display_name, is_default) VALUES ($1, $2, $3, $4, $5)",
            model_id,
            provider_id,
            "test-model",
            "Test Model",
            False,
        )
    return model_id


@pytest.fixture
async def chat_with_message(db_pool, test_model):
    """Create a user, chat, and user message in the real DB, return (chat_id, user_id, model_id)."""
    users_repo = UsersRepository(pool=db_pool)
    user = await users_repo.create(
        email=f"{ULID()}@test.local",
        password_hash="not-a-real-hash",
        full_name="Test User",
    )

    chats_repo = ChatsRepository(pool=db_pool)
    chat = await chats_repo.create(user_id=user.id, model_id=test_model)

    messages_repo = MessagesRepository(pool=db_pool)
    await messages_repo.create(
        chat_id=chat.id,
        message={"role": "user", "content": "Find all high-priority bugs"},
    )
    return chat.id, user.id, test_model


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


def _build_app(llm_provider, searcher_tool, model_id: str) -> FastAPI:
    app = FastAPI()
    app.state = AppState()
    app.state.models = {model_id: llm_provider}
    app.state.default_model_id = model_id
    app.state.searcher_tool = searcher_tool
    app.include_router(chat_router)
    return app


# ---------------------------------------------------------------------------
# Tests
# ---------------------------------------------------------------------------


@pytest.mark.asyncio
async def test_inline_query_operators_flow_to_searcher(
    db_pool, chat_with_message, _patch_db_pool
):
    """Inline query operators are passed through to the searcher as the query string."""
    chat_id, _, model_id = chat_with_message
    tool_call_json = {
        "query": "status:done in:jira high priority bugs",
    }
    searcher = create_mock_searcher()
    app = _build_app(create_mock_llm(tool_call_json), searcher, model_id)

    await _stream_chat(app, chat_id)

    searcher.handle.assert_called_once()
    captured_request = searcher.handle.call_args[0][0]
    assert captured_request.query == "status:done in:jira high priority bugs"
    assert captured_request.attribute_filters is None
    assert captured_request.source_types is None


@pytest.mark.asyncio
async def test_simple_query_sends_no_filters(
    db_pool, chat_with_message, _patch_db_pool
):
    """A plain query without operators sends no filters."""
    chat_id, _, model_id = chat_with_message
    tool_call_json = {"query": "recent documents"}
    searcher = create_mock_searcher()
    app = _build_app(create_mock_llm(tool_call_json), searcher, model_id)

    await _stream_chat(app, chat_id)

    searcher.handle.assert_called_once()
    captured_request = searcher.handle.call_args[0][0]
    assert captured_request.attribute_filters is None
    assert captured_request.source_types is None


@pytest.mark.asyncio
async def test_stream_completes_with_tool_results(
    db_pool, chat_with_message, _patch_db_pool
):
    """Full SSE stream contains tool call events, save_message, tool_result, text, and end_of_stream."""
    chat_id, _, model_id = chat_with_message
    tool_call_json = {
        "query": "status:open in:jira high priority bugs",
    }
    response_text = "I found 2 high-priority bugs."
    searcher = create_mock_searcher()
    app = _build_app(create_mock_llm(tool_call_json, response_text), searcher, model_id)

    body = await _stream_chat(app, chat_id)
    events = parse_sse_events(body)
    event_types = [e[0] for e in events]

    assert "save_message" in event_types
    assert "end_of_stream" in event_types

    tool_result_events = [
        (t, d) for t, d in events if t == "message" and "tool_result" in d
    ]
    assert len(tool_result_events) >= 1
    tool_result_data = json.loads(tool_result_events[0][1])
    assert tool_result_data["type"] == "tool_result"
    assert tool_result_data["is_error"] is False

    text_deltas = [d for t, d in events if t == "message" and response_text in d]
    assert len(text_deltas) >= 1

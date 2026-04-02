"""Integration tests for @-mentioned document context injection.

Validates that when a user message has mentioned_document_ids, the chat handler
fetches each document's content and injects it into the user message before
sending to the LLM.

Uses real DB (testcontainers ParadeDB) for chat/message storage, a mock LLM
that captures the messages it receives, and a patched DocumentToolHandler.
"""

import json
from unittest.mock import AsyncMock, patch

import pytest
from fastapi import FastAPI
from httpx import ASGITransport, AsyncClient
from ulid import ULID

from db import UsersRepository, ChatsRepository
import db.connection
from routers import chat_router
from state import AppState
from models.chat import MentionedDocumentContext
from tools.document_handler import DocumentToolHandler
from .llm_helpers import create_capturing_llm, create_simple_llm, create_mock_searcher, parse_sse_events

pytestmark = pytest.mark.integration


# ---------------------------------------------------------------------------
# Fixtures
# ---------------------------------------------------------------------------


@pytest.fixture
async def test_model(db_pool) -> str:
    async with db_pool.acquire() as conn:
        provider_id = str(ULID())
        await conn.execute(
            "INSERT INTO model_providers (id, name, provider_type, config) VALUES ($1, $2, $3, $4)",
            provider_id, "Test Provider", "anthropic", "{}",
        )
        model_id = str(ULID())
        await conn.execute(
            "INSERT INTO models (id, model_provider_id, model_id, display_name, is_default) VALUES ($1, $2, $3, $4, $5)",
            model_id, provider_id, "test-model", "Test Model", False,
        )
    return model_id


@pytest.fixture
async def chat_with_mention(db_pool, test_model):
    """Create a user, chat, and user message with mentioned_document_ids."""
    users_repo = UsersRepository(pool=db_pool)
    user = await users_repo.create(
        email=f"{ULID()}@test.local",
        password_hash="not-a-real-hash",
        full_name="Test User",
    )

    chats_repo = ChatsRepository(pool=db_pool)
    chat = await chats_repo.create(user_id=user.id, model_id=test_model)

    async with db_pool.acquire() as conn:
        await conn.execute(
            """INSERT INTO chat_messages (id, chat_id, message_seq_num, message, mentioned_document_ids, created_at)
               VALUES ($1, $2, 1, $3, $4, NOW())""",
            str(ULID()),
            chat.id,
            json.dumps({"role": "user", "content": "Summarise @My Doc"}),
            json.dumps(["doc-123"]),
        )

    return chat.id, user.id, test_model


@pytest.fixture
async def chat_with_multiple_mentions(db_pool, test_model):
    """Create a chat with a message that mentions two documents."""
    users_repo = UsersRepository(pool=db_pool)
    user = await users_repo.create(
        email=f"{ULID()}@test.local",
        password_hash="not-a-real-hash",
        full_name="Test User",
    )

    chats_repo = ChatsRepository(pool=db_pool)
    chat = await chats_repo.create(user_id=user.id, model_id=test_model)

    async with db_pool.acquire() as conn:
        await conn.execute(
            """INSERT INTO chat_messages (id, chat_id, message_seq_num, message, mentioned_document_ids, created_at)
               VALUES ($1, $2, 1, $3, $4, NOW())""",
            str(ULID()),
            chat.id,
            json.dumps({"role": "user", "content": "Compare @Doc A and @Doc B"}),
            json.dumps(["doc-aaa", "doc-bbb"]),
        )

    return chat.id, user.id, test_model


@pytest.fixture
async def chat_without_mention(db_pool, test_model):
    """Create a chat with a normal message (no mentions)."""
    users_repo = UsersRepository(pool=db_pool)
    user = await users_repo.create(
        email=f"{ULID()}@test.local",
        password_hash="not-a-real-hash",
        full_name="Test User",
    )

    chats_repo = ChatsRepository(pool=db_pool)
    chat = await chats_repo.create(user_id=user.id, model_id=test_model)

    async with db_pool.acquire() as conn:
        await conn.execute(
            """INSERT INTO chat_messages (id, chat_id, message_seq_num, message, mentioned_document_ids, created_at)
               VALUES ($1, $2, 1, $3, $4, NOW())""",
            str(ULID()),
            chat.id,
            json.dumps({"role": "user", "content": "What is the weather?"}),
            json.dumps([]),
        )

    return chat.id, user.id, test_model


@pytest.fixture
def _patch_db_pool(db_pool, monkeypatch):
    monkeypatch.setattr(db.connection, "_db_pool", db_pool)


def _build_app(llm_provider, model_id: str) -> FastAPI:
    app = FastAPI()
    app.state = AppState()
    app.state.models = {model_id: llm_provider}
    app.state.default_model_id = model_id
    app.state.searcher_tool = create_mock_searcher()
    app.include_router(chat_router)
    return app


async def _stream_chat(app: FastAPI, chat_id: str) -> str:
    async with AsyncClient(
        transport=ASGITransport(app=app), base_url="http://test"
    ) as client:
        resp = await client.get(f"/chat/{chat_id}/stream", timeout=30)
        assert resp.status_code == 200
        return resp.text


# ---------------------------------------------------------------------------
# Tests
# ---------------------------------------------------------------------------


@pytest.mark.asyncio
async def test_mentioned_documents_injected(db_pool, chat_with_multiple_mentions, _patch_db_pool):
    """All mentioned documents are injected as separate blocks."""
    chat_id, _, model_id = chat_with_multiple_mentions

    captured_messages = []
    llm = create_capturing_llm(captured_messages)

    async def fetch_side_effect(doc_id, _):
        return {
            "doc-aaa": MentionedDocumentContext(doc_id="doc-aaa", title="Doc A", content="Content of A."),
            "doc-bbb": MentionedDocumentContext(doc_id="doc-bbb", title="Doc B", content="Content of B."),
        }.get(doc_id)

    app = _build_app(llm, model_id)

    with patch.object(DocumentToolHandler, "fetch_document_for_context", new_callable=AsyncMock) as mock_fetch:
        mock_fetch.side_effect = fetch_side_effect
        await _stream_chat(app, chat_id)

    last_user_msg = next(
        m for m in reversed(captured_messages) if m.get("role") == "user"
    )
    assert "Doc A" in last_user_msg["content"]
    assert "Content of A." in last_user_msg["content"]
    assert "Doc B" in last_user_msg["content"]
    assert "Content of B." in last_user_msg["content"]


@pytest.mark.asyncio
async def test_missing_document_skipped_stream_completes(db_pool, chat_with_mention, _patch_db_pool):
    """If fetch_document_for_context returns None, the stream completes without error."""
    chat_id, _, model_id = chat_with_mention

    app = _build_app(create_simple_llm(), model_id)

    with patch.object(DocumentToolHandler, "fetch_document_for_context", new_callable=AsyncMock) as mock_fetch:
        mock_fetch.return_value = None
        body = await _stream_chat(app, chat_id)

    events = parse_sse_events(body)
    event_types = [e[0] for e in events]
    assert "end_of_stream" in event_types
    assert "error" not in event_types


@pytest.mark.asyncio
async def test_no_mentions_unaffected(db_pool, chat_without_mention, _patch_db_pool):
    """Messages with no mentions pass through unchanged."""
    chat_id, _, model_id = chat_without_mention

    captured_messages = []
    llm = create_capturing_llm(captured_messages)

    app = _build_app(llm, model_id)

    with patch.object(DocumentToolHandler, "fetch_document_for_context", new_callable=AsyncMock) as mock_fetch:
        await _stream_chat(app, chat_id)

    # fetch_document_for_context should never have been called
    mock_fetch.assert_not_called()

    last_user_msg = next(
        m for m in reversed(captured_messages) if m.get("role") == "user"
    )
    assert last_user_msg["content"] == "What is the weather?"


@pytest.mark.asyncio
async def test_document_injected_in_xml_blocks(
    db_pool, chat_with_multiple_mentions, _patch_db_pool
):
    """Multiple injected documents produce correct full XML structure."""
    chat_id, _, model_id = chat_with_multiple_mentions

    captured_messages = []
    llm = create_capturing_llm(captured_messages)

    async def fetch_side_effect(doc_id, context):
        return {
            "doc-aaa": MentionedDocumentContext(doc_id="doc-aaa", title="Doc A", content="Content of A."),
            "doc-bbb": MentionedDocumentContext(doc_id="doc-bbb", title="Doc B", content="Content of B."),
        }.get(doc_id)

    app = _build_app(llm, model_id)

    with patch.object(DocumentToolHandler, "fetch_document_for_context", new_callable=AsyncMock) as mock_fetch:
        mock_fetch.side_effect = fetch_side_effect
        await _stream_chat(app, chat_id)

    last_user_msg = next(
        m for m in reversed(captured_messages) if m.get("role") == "user"
    )
    expected = (
        'Compare @Doc A and @Doc B\n\n'
        '<mentioned_documents>\n'
        '<document doc_id="doc-aaa" doc_title="Doc A">\nContent of A.\n</document>\n\n'
        '<document doc_id="doc-bbb" doc_title="Doc B">\nContent of B.\n</document>\n'
        '</mentioned_documents>'
    )
    assert last_user_msg["content"] == expected

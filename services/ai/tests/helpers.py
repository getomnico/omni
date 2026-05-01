"""Shared test helpers for integration tests.

Provides reusable DB data factories and mock LLM event generators
so that individual test files don't duplicate boilerplate.
"""

import json
from typing import Any
from unittest.mock import AsyncMock

from ulid import ULID

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
            """INSERT INTO documents (id, source_id, external_id, title, content_id, content, embedding_status)
               VALUES ($1, $2, $3, $4, $5, $6, 'pending')""",
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
        if kind == "tool_call":
            for evt in tool_call_events(data):
                yield evt
        else:
            for evt in text_response_events(data):
                yield evt

    provider = AsyncMock()
    provider.stream_response = stream_response
    provider.health_check.return_value = True
    return provider

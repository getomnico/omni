"""Shared LLM event helpers, mock factories, and SSE utilities for integration tests."""

import json
from typing import Any
from unittest.mock import AsyncMock

from tools.searcher_client import SearchResponse


def create_mock_searcher(results: list | None = None) -> AsyncMock:
    """Return a mock SearcherTool with an empty (or provided) search response."""
    searcher = AsyncMock()
    searcher.handle.return_value = SearchResponse(
        results=results or [],
        total_count=len(results) if results else 0,
        query_time_ms=1,
    )
    return searcher

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


# ---------------------------------------------------------------------------
# Anthropic event builders
# ---------------------------------------------------------------------------


def message_start_event() -> RawMessageStartEvent:
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


def tool_call_events(tool_call_json: dict[str, Any]):
    """Yield Anthropic SDK events simulating a tool_use content block."""
    yield message_start_event()
    yield RawContentBlockStartEvent(
        type="content_block_start",
        index=0,
        content_block=ToolUseBlock(
            type="tool_use",
            id="toolu_test",
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


def text_response_events(text: str):
    """Yield Anthropic SDK events simulating a final text response."""
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


# ---------------------------------------------------------------------------
# Mock LLM factories
# ---------------------------------------------------------------------------


def create_mock_llm(
    tool_call_json: dict[str, Any], response_text: str = "Here are the results."
):
    """LLM that emits a tool call on the first iteration then a text response."""
    call_count = 0

    async def stream_response(*_args, **_kwargs):
        nonlocal call_count
        call_count += 1
        if call_count == 1:
            for evt in tool_call_events(tool_call_json):
                yield evt
        else:
            for evt in text_response_events(response_text):
                yield evt

    provider = AsyncMock()
    provider.stream_response = stream_response
    provider.health_check.return_value = True
    return provider


def create_capturing_llm(captured_messages: list, response_text: str = "Here is a summary."):
    """LLM that records the messages it receives and returns a text response."""

    async def stream_response(*_args, messages, **_kwargs):
        captured_messages.extend(messages)
        for evt in text_response_events(response_text):
            yield evt

    provider = AsyncMock()
    provider.stream_response = stream_response
    provider.health_check.return_value = True
    return provider


def create_simple_llm(response_text: str = "Done."):
    """LLM that returns a text response without capturing anything."""

    async def stream_response(*_args, **_kwargs):
        for evt in text_response_events(response_text):
            yield evt

    provider = AsyncMock()
    provider.stream_response = stream_response
    provider.health_check.return_value = True
    return provider


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
            current_event = line[len("event: "):]
        elif line.startswith("data: "):
            current_data_lines.append(line[len("data: "):])
        elif line == "" and current_event is not None:
            events.append((current_event, "\n".join(current_data_lines)))
            current_event = None
            current_data_lines = []

    if current_event is not None:
        events.append((current_event, "\n".join(current_data_lines)))

    return events

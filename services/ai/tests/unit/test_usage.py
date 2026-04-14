"""Unit tests for UsageTracker and streaming usage attribution."""

import asyncio
from collections.abc import AsyncIterator
from unittest.mock import AsyncMock

import pytest
from anthropic.types import (
    Message,
    MessageDeltaUsage,
    RawMessageDeltaEvent,
    RawMessageStartEvent,
    RawMessageStopEvent,
    Usage,
)
from anthropic.types.message_stream_event import MessageStreamEvent
from anthropic.types.raw_message_delta_event import Delta

from providers import TokenUsage
from providers.azure_foundry import AzureFoundryProvider
from providers.openai import OpenAIProvider
from services.usage import UsageContext, UsagePurpose, UsageTracker


def _ctx() -> UsageContext:
    return UsageContext(
        user_id="u",
        model_id="m",
        model_name="m",
        provider_type="test",
        purpose=UsagePurpose.CHAT,
    )


async def _drain(stream: AsyncIterator[MessageStreamEvent]) -> None:
    async for _ in stream:
        pass


def _stream_factory(input_tokens: int, output_tokens: int):
    """Simulates a provider that only knows input_tokens at end-of-stream
    (like OpenAI reasoning models / Gemini / openai_compatible)."""

    async def stream() -> AsyncIterator[MessageStreamEvent]:
        yield RawMessageStartEvent(
            type="message_start",
            message=Message(
                id="msg",
                type="message",
                role="assistant",
                content=[],
                model="m",
                usage=Usage(input_tokens=0, output_tokens=0),
            ),
        )
        yield RawMessageDeltaEvent(
            type="message_delta",
            delta=Delta(stop_reason="end_turn"),
            usage=MessageDeltaUsage(
                input_tokens=input_tokens,
                output_tokens=output_tokens,
            ),
        )
        yield RawMessageStopEvent(type="message_stop")

    return stream


@pytest.mark.unit
@pytest.mark.asyncio
async def test_wrap_stream_captures_input_tokens_from_message_delta():
    """Providers that report input_tokens only on the final message_delta
    must still end up in the tracker's input_tokens field."""
    tracker = UsageTracker(repo=AsyncMock(), ctx=_ctx())

    await _drain(tracker.wrap_stream(_stream_factory(123, 45)()))

    assert tracker.input_tokens == 123
    assert tracker.output_tokens == 45


@pytest.mark.unit
@pytest.mark.asyncio
async def test_concurrent_streams_do_not_clobber_each_other():
    """Two concurrent streams must each end up with their own token counts.
    Regression guard for the old provider.last_usage shared-field race."""
    t1 = UsageTracker(repo=AsyncMock(), ctx=_ctx())
    t2 = UsageTracker(repo=AsyncMock(), ctx=_ctx())

    await asyncio.gather(
        _drain(t1.wrap_stream(_stream_factory(100, 10)())),
        _drain(t2.wrap_stream(_stream_factory(200, 20)())),
    )

    assert (t1.input_tokens, t1.output_tokens) == (100, 10)
    assert (t2.input_tokens, t2.output_tokens) == (200, 20)


@pytest.mark.unit
@pytest.mark.asyncio
async def test_azure_foundry_streams_propagate_input_tokens():
    """Regression guard for the Azure Foundry input_tokens=0 bug.

    Simulates an OpenAI delegate whose message_start reports input_tokens=0
    (as happens for reasoning models where response.created arrives before
    usage is populated), and only reports real input tokens on the final
    message_delta. UsageTracker must still end with the real count.
    """
    provider = AzureFoundryProvider.__new__(AzureFoundryProvider)
    provider._delegate = OpenAIProvider.__new__(OpenAIProvider)
    provider._delegate.stream_response = lambda **_: _stream_factory(500, 50)()

    tracker = UsageTracker(repo=AsyncMock(), ctx=_ctx())

    await _drain(tracker.wrap_stream(provider.stream_response(prompt="hi")))

    assert tracker.input_tokens == 500
    assert tracker.output_tokens == 50

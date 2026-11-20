from __future__ import annotations

import asyncio
import threading
from types import SimpleNamespace
from unittest.mock import AsyncMock

import pytest

from providers.bedrock import BedrockProvider
from providers.gemini import GeminiProvider
from providers.openai import OpenAIProvider
from providers.openai_compatible import OpenAICompatibleProvider, REASONING_CONTENT_KEY
from providers.anthropic import AnthropicProvider
from providers.types import ProviderError

pytestmark = pytest.mark.unit


class _AsyncEvents:
    def __init__(self, events):
        self.events = events

    def __aiter__(self):
        async def iterate():
            for event in self.events:
                yield event

        return iterate()


def _openai_provider(events):
    provider = OpenAIProvider.__new__(OpenAIProvider)
    provider.model = "test-model"
    provider.model_name = "test-model"
    provider.client = SimpleNamespace(
        responses=SimpleNamespace(create=AsyncMock(return_value=_AsyncEvents(events)))
    )
    return provider


@pytest.mark.asyncio
@pytest.mark.parametrize(
    ("reason", "expected"),
    [("max_output_tokens", "max_tokens"), ("content_filter", "end_turn")],
)
async def test_openai_incomplete_emits_reason_without_usage(reason, expected):
    response = SimpleNamespace(
        id="response-1",
        model="test-model",
        usage=None,
        incomplete_details=SimpleNamespace(reason=reason),
    )
    events = [
        SimpleNamespace(type="response.created", response=response),
        SimpleNamespace(type="response.output_text.delta", delta="partial answer"),
        SimpleNamespace(type="response.incomplete", response=response),
    ]

    output = [event async for event in _openai_provider(events).stream_response("hi")]

    assert output[-2].type == "message_delta"
    assert output[-2].delta.stop_reason == expected
    assert output[-2].usage.output_tokens == 0
    assert output[-1].type == "message_stop"
    assert sum(event.type == "content_block_stop" for event in output) == 1


@pytest.mark.asyncio
async def test_openai_compatible_length_emits_max_tokens_and_preserves_usage_and_reasoning():
    tool_delta = SimpleNamespace(
        index=0,
        id="call-partial",
        function=SimpleNamespace(name="search", arguments='{"query":'),
    )
    reasoning_delta = SimpleNamespace(
        content="partial",
        tool_calls=[tool_delta],
        model_extra={REASONING_CONTENT_KEY: "internal reasoning"},
    )
    chunks = [
        SimpleNamespace(
            choices=[SimpleNamespace(delta=reasoning_delta, finish_reason="length")],
            usage=None,
        ),
        SimpleNamespace(
            choices=[],
            usage=SimpleNamespace(prompt_tokens=12, completion_tokens=8),
        ),
    ]
    provider = OpenAICompatibleProvider.__new__(OpenAICompatibleProvider)
    provider.model = "test-model"
    provider.model_name = "test-model"
    provider.client = SimpleNamespace(
        chat=SimpleNamespace(
            completions=SimpleNamespace(create=AsyncMock(return_value=_AsyncEvents(chunks)))
        )
    )

    output = [event async for event in provider.stream_response("hi")]

    delta = next(event for event in output if event.type == "message_delta")
    assert delta.delta.stop_reason == "max_tokens"
    assert delta.usage.input_tokens == 12
    assert delta.usage.output_tokens == 8
    assert output[-1].type == "message_stop"
    reasoning_blocks = [
        event.content_block
        for event in output
        if event.type == "content_block_start"
        and getattr(event.content_block, REASONING_CONTENT_KEY, None)
    ]
    assert len(reasoning_blocks) == 1
    assert getattr(reasoning_blocks[0], REASONING_CONTENT_KEY) == "internal reasoning"
    tool_block = next(
        event.content_block
        for event in output
        if event.type == "content_block_start" and event.content_block.type == "tool_use"
    )
    assert tool_block.name == "search"
    assert sum(event.type == "content_block_stop" for event in output) == 3


@pytest.mark.asyncio
async def test_openai_compatible_terminal_without_usage_has_bounded_drain_and_closes_stream():
    class HangingStream:
        def __init__(self):
            self.closed = False

        def __aiter__(self):
            async def iterate():
                yield SimpleNamespace(
                    choices=[
                        SimpleNamespace(
                            delta=SimpleNamespace(content=None, tool_calls=None),
                            finish_reason="length",
                        )
                    ],
                    usage=None,
                )
                await asyncio.Future()

            return iterate()

        async def close(self):
            self.closed = True

    stream = HangingStream()
    provider = OpenAICompatibleProvider.__new__(OpenAICompatibleProvider)
    provider.model = "test-model"
    provider.model_name = "test-model"
    provider.client = SimpleNamespace(
        chat=SimpleNamespace(
            completions=SimpleNamespace(create=AsyncMock(return_value=stream))
        )
    )

    output = await asyncio.wait_for(
        _collect(provider.stream_response("hi")), timeout=1.0
    )

    delta = next(event for event in output if event.type == "message_delta")
    assert delta.delta.stop_reason == "max_tokens"
    assert stream.closed
    assert output[-1].type == "message_stop"


async def _collect(stream):
    return [event async for event in stream]


@pytest.mark.asyncio
async def test_gemini_max_tokens_stops_stream_and_emits_reason_without_usage():
    text_part = SimpleNamespace(text="partial", function_call=None, thought_signature=None)
    candidate = SimpleNamespace(
        content=SimpleNamespace(parts=[text_part]), finish_reason="MAX_TOKENS"
    )
    chunks = _AsyncEvents([SimpleNamespace(candidates=[candidate], usage_metadata=None)])
    models = SimpleNamespace(generate_content_stream=AsyncMock(return_value=chunks))
    provider = GeminiProvider.__new__(GeminiProvider)
    provider.model = "test-model"
    provider.model_name = "test-model"
    provider.client = SimpleNamespace(aio=SimpleNamespace(models=models))

    output = [event async for event in provider.stream_response("hi")]

    delta = next(event for event in output if event.type == "message_delta")
    assert delta.delta.stop_reason == "max_tokens"
    assert delta.usage.output_tokens == 0
    assert output[-1].type == "message_stop"
    assert models.generate_content_stream.await_count == 1


@pytest.mark.asyncio
async def test_gemini_uses_final_usage_after_an_earlier_usage_chunk():
    early_usage = SimpleNamespace(
        prompt_token_count=3,
        cached_content_token_count=0,
        candidates_token_count=2,
    )
    final_usage = SimpleNamespace(
        prompt_token_count=7,
        cached_content_token_count=1,
        candidates_token_count=9,
    )
    chunks = _AsyncEvents(
        [
            SimpleNamespace(
                candidates=[SimpleNamespace(content=None, finish_reason=None)],
                usage_metadata=early_usage,
            ),
            SimpleNamespace(
                candidates=[SimpleNamespace(content=None, finish_reason="MAX_TOKENS")],
                usage_metadata=None,
            ),
            SimpleNamespace(candidates=[], usage_metadata=final_usage),
        ]
    )
    models = SimpleNamespace(generate_content_stream=AsyncMock(return_value=chunks))
    provider = GeminiProvider.__new__(GeminiProvider)
    provider.model = "test-model"
    provider.model_name = "test-model"
    provider.client = SimpleNamespace(aio=SimpleNamespace(models=models))

    output = [event async for event in provider.stream_response("hi")]

    delta = next(event for event in output if event.type == "message_delta")
    assert delta.delta.stop_reason == "max_tokens"
    assert delta.usage.input_tokens == 6
    assert delta.usage.output_tokens == 9
    assert delta.usage.cache_read_input_tokens == 1


@pytest.mark.asyncio
async def test_gemini_terminal_without_usage_has_bounded_drain_and_closes_stream():
    class HangingStream:
        def __init__(self):
            self.closed = False

        def __aiter__(self):
            async def iterate():
                yield SimpleNamespace(
                    candidates=[
                        SimpleNamespace(
                            content=None,
                            finish_reason="MAX_TOKENS",
                        )
                    ],
                    usage_metadata=None,
                )
                await asyncio.Future()

            return iterate()

        async def aclose(self):
            self.closed = True

    stream = HangingStream()
    models = SimpleNamespace(generate_content_stream=AsyncMock(return_value=stream))
    provider = GeminiProvider.__new__(GeminiProvider)
    provider.model = "test-model"
    provider.model_name = "test-model"
    provider.client = SimpleNamespace(aio=SimpleNamespace(models=models))

    output = await asyncio.wait_for(
        _collect(provider.stream_response("hi")), timeout=1.0
    )

    delta = next(event for event in output if event.type == "message_delta")
    assert delta.delta.stop_reason == "max_tokens"
    assert stream.closed
    assert output[-1].type == "message_stop"


@pytest.mark.asyncio
async def test_bedrock_amazon_message_stop_normalizes_token_limit_and_usage():
    provider = BedrockProvider.__new__(BedrockProvider)
    provider.model_id = "amazon.nova-lite-v1:0"
    provider.model_family = "amazon"

    class Client:
        def converse_stream(self, **kwargs):
            return {
                "stream": iter(
                    [
                        {"messageStart": {"role": "assistant"}},
                        {"messageStop": {"stopReason": "max_tokens"}},
                        {"metadata": {"usage": {"inputTokens": 5, "outputTokens": 9}}},
                    ]
                )
            }

    provider.client = Client()
    output = [event async for event in provider.stream_response("hi")]

    delta = next(event for event in output if event.type == "message_delta")
    assert delta.delta.stop_reason == "max_tokens"
    assert delta.usage.input_tokens == 5
    assert delta.usage.output_tokens == 9
    assert output[-1].type == "message_stop"
    assert len(output) == 3


@pytest.mark.asyncio
async def test_bedrock_amazon_message_stop_without_metadata_has_bounded_drain():
    provider = BedrockProvider.__new__(BedrockProvider)
    provider.model_id = "amazon.nova-lite-v1:0"
    provider.model_family = "amazon"

    class BlockingEventStream:
        def __init__(self):
            self.closed = threading.Event()

        def __iter__(self):
            def generate():
                yield {"messageStart": {"role": "assistant"}}
                yield {"messageStop": {"stopReason": "max_tokens"}}
                self.closed.wait()

            return generate()

        def close(self):
            self.closed.set()

    event_stream = BlockingEventStream()

    class Client:
        def converse_stream(self, **kwargs):
            return {"stream": event_stream}

    provider.client = Client()
    output = await asyncio.wait_for(_collect(provider.stream_response("hi")), timeout=1.0)

    delta = next(event for event in output if event.type == "message_delta")
    assert delta.delta.stop_reason == "max_tokens"
    assert delta.usage.output_tokens == 0
    assert output[-1].type == "message_stop"
    assert event_stream.closed.is_set()


@pytest.mark.asyncio
async def test_anthropic_stream_closes_sdk_stream_when_adapter_is_closed():
    class ClosableStream:
        def __init__(self):
            self.closed = False

        def __aiter__(self):
            async def iterate():
                yield SimpleNamespace(type="message_start")
                await asyncio.Future()

            return iterate()

        async def close(self):
            self.closed = True

    stream = ClosableStream()
    provider = AnthropicProvider.__new__(AnthropicProvider)
    provider.model = "test-model"
    provider.model_name = "test-model"
    provider.client = SimpleNamespace(
        messages=SimpleNamespace(create=AsyncMock(return_value=stream))
    )

    adapter = provider.stream_response("hi")
    assert (await anext(adapter)).type == "message_start"
    await adapter.aclose()

    assert stream.closed


@pytest.mark.asyncio
async def test_openai_compatible_read_timeout_before_finish_is_not_suppressed():
    class TimedOutStream:
        def __aiter__(self):
            async def iterate():
                raise TimeoutError("provider read timeout")
                yield

            return iterate()

    provider = OpenAICompatibleProvider.__new__(OpenAICompatibleProvider)
    provider.model = "test-model"
    provider.model_name = "test-model"
    provider.client = SimpleNamespace(
        chat=SimpleNamespace(
            completions=SimpleNamespace(
                create=AsyncMock(return_value=TimedOutStream())
            )
        )
    )

    with pytest.raises(ProviderError, match="provider read timeout"):
        await _collect(provider.stream_response("hi"))

from __future__ import annotations

import json
from collections.abc import AsyncIterator, Callable, Sequence
from dataclasses import dataclass
from types import SimpleNamespace
from typing import Protocol
from unittest.mock import AsyncMock, MagicMock

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
from anthropic.types.message_stream_event import MessageStreamEvent
from anthropic.types.raw_message_delta_event import Delta

from providers import LLMProvider, ProviderError, TokenUsage
from providers.anthropic import AnthropicProvider
from providers.azure_foundry import AzureFoundryProvider
from providers.bedrock import BedrockProvider
from providers.gemini import GeminiProvider
from providers.openai import OpenAIProvider
from providers.openai_compatible import OpenAICompatibleProvider
from providers.types import ProviderType
from providers.vertex_ai import VertexAIProvider


class ContractProvider(Protocol):
    def stream_response(self, **kwargs: object) -> AsyncIterator[MessageStreamEvent]: ...

    async def generate_response(
        self, prompt: str, **kwargs: object
    ) -> tuple[str, TokenUsage]: ...


class AsyncEventStream:
    def __init__(self, events: Sequence[object]) -> None:
        self.events = events

    def __aiter__(self) -> AsyncIterator[object]:
        return self._iterate()

    async def _iterate(self) -> AsyncIterator[object]:
        for event in self.events:
            yield event


@dataclass
class StreamCase:
    provider: LLMProvider
    request: AsyncMock | MagicMock
    expected_input_tokens: int
    expected_cache_read_tokens: int = 0
    request_model_key: str = "model"


@dataclass
class GenerateCase:
    provider: LLMProvider
    request: AsyncMock | MagicMock
    expected_usage: TokenUsage
    request_model_key: str = "model"


_WEATHER_TOOL = {
    "name": "get_weather",
    "description": "Get the weather for a city.",
    "input_schema": {
        "type": "object",
        "properties": {"city": {"type": "string"}},
        "required": ["city"],
    },
}


def _anthropic_events(model: str = "configured-model") -> list[MessageStreamEvent]:
    return [
        RawMessageStartEvent(
            type="message_start",
            message=Message(
                id="msg_test",
                type="message",
                role="assistant",
                content=[],
                model=model,
                usage=Usage(input_tokens=0, output_tokens=0),
            ),
        ),
        RawContentBlockStartEvent(
            type="content_block_start",
            index=0,
            content_block=TextBlock(type="text", text=""),
        ),
        RawContentBlockDeltaEvent(
            type="content_block_delta",
            index=0,
            delta=TextDelta(type="text_delta", text="Weather: "),
        ),
        RawContentBlockStopEvent(type="content_block_stop", index=0),
        RawContentBlockStartEvent(
            type="content_block_start",
            index=1,
            content_block=ToolUseBlock(
                type="tool_use", id="toolu_test", name="get_weather", input={}
            ),
        ),
        RawContentBlockDeltaEvent(
            type="content_block_delta",
            index=1,
            delta=InputJSONDelta(
                type="input_json_delta", partial_json='{"city":"Bengaluru"}'
            ),
        ),
        RawContentBlockStopEvent(type="content_block_stop", index=1),
        RawMessageDeltaEvent(
            type="message_delta",
            delta=Delta(stop_reason="tool_use"),
            usage=MessageDeltaUsage(input_tokens=11, output_tokens=7),
        ),
        RawMessageStopEvent(type="message_stop"),
    ]


def _openai_response_events() -> list[object]:
    usage = SimpleNamespace(
        input_tokens=11,
        output_tokens=7,
        input_tokens_details=SimpleNamespace(cached_tokens=0),
    )
    response = SimpleNamespace(
        id="response_test",
        model="configured-model",
        usage=usage,
        error=None,
    )
    function_call = SimpleNamespace(
        type="function_call", id="item_test", call_id="toolu_test", name="get_weather"
    )
    return [
        SimpleNamespace(type="response.created", response=response),
        SimpleNamespace(type="response.output_text.delta", delta="Weather: "),
        SimpleNamespace(type="response.output_text.done"),
        SimpleNamespace(type="response.output_item.added", item=function_call),
        SimpleNamespace(
            type="response.function_call_arguments.delta",
            item_id="item_test",
            delta='{"city":"Bengaluru"}',
        ),
        SimpleNamespace(type="response.output_item.done", item=function_call),
        SimpleNamespace(type="response.completed", response=response),
    ]


def _openai_compatible_chunks() -> list[object]:
    def chunk(content=None, tool_calls=None, finish_reason=None, usage=None):
        delta = SimpleNamespace(content=content, tool_calls=tool_calls)
        return SimpleNamespace(
            choices=(
                []
                if usage is not None
                else [SimpleNamespace(delta=delta, finish_reason=finish_reason)]
            ),
            usage=usage,
        )

    first_tool_delta = SimpleNamespace(
        index=0,
        id="toolu_test",
        function=SimpleNamespace(name="get_weather", arguments='{"city":"'),
    )
    second_tool_delta = SimpleNamespace(
        index=0,
        id=None,
        function=SimpleNamespace(name=None, arguments='Bengaluru"}'),
    )
    return [
        chunk(content="Weather: "),
        chunk(tool_calls=[first_tool_delta]),
        chunk(tool_calls=[second_tool_delta], finish_reason="tool_calls"),
        chunk(usage=SimpleNamespace(prompt_tokens=11, completion_tokens=7)),
    ]


def _gemini_chunks() -> list[object]:
    usage = SimpleNamespace(
        prompt_token_count=11,
        cached_content_token_count=2,
        candidates_token_count=7,
    )
    text_part = SimpleNamespace(text="Weather: ", function_call=None, thought_signature=None)
    tool_part = SimpleNamespace(
        text=None,
        function_call=SimpleNamespace(
            name="get_weather", args={"city": "Bengaluru"}
        ),
        thought_signature=None,
    )
    return [
        SimpleNamespace(
            candidates=[
                SimpleNamespace(
                    content=SimpleNamespace(parts=[text_part]),
                )
            ],
            usage_metadata=None,
        ),
        SimpleNamespace(
            candidates=[
                SimpleNamespace(
                    content=SimpleNamespace(parts=[tool_part]),
                )
            ],
            usage_metadata=usage,
        ),
    ]


def _bedrock_amazon_events() -> list[dict[str, object]]:
    return [
        {"messageStart": {"role": "assistant"}},
        {
            "contentBlockStart": {
                "contentBlockIndex": 0,
                "start": {},
            }
        },
        {
            "contentBlockDelta": {
                "contentBlockIndex": 0,
                "delta": {"text": "Weather: "},
            }
        },
        {"contentBlockStop": {"contentBlockIndex": 0}},
        {
            "contentBlockStart": {
                "contentBlockIndex": 1,
                "start": {
                    "toolUse": {"toolUseId": "toolu_test", "name": "get_weather"}
                },
            }
        },
        {
            "contentBlockDelta": {
                "contentBlockIndex": 1,
                "delta": {"toolUse": {"input": '{"city":"Bengaluru"}'}},
            }
        },
        {"contentBlockStop": {"contentBlockIndex": 1}},
        {"metadata": {"usage": {"inputTokens": 11, "outputTokens": 7}}},
        {"messageStop": {"stopReason": "tool_use"}},
    ]


def _anthropic_stream_case() -> StreamCase:
    request = AsyncMock(return_value=AsyncEventStream(_anthropic_events()))
    provider = AnthropicProvider.__new__(AnthropicProvider)
    provider.model = "configured-model"
    provider.model_name = "configured-model"
    object.__setattr__(
        provider, "client", SimpleNamespace(messages=SimpleNamespace(create=request))
    )
    return StreamCase(provider, request, expected_input_tokens=11)


def _openai_stream_case() -> StreamCase:
    request = AsyncMock(return_value=AsyncEventStream(_openai_response_events()))
    provider = OpenAIProvider.__new__(OpenAIProvider)
    provider.model = "configured-model"
    provider.model_name = "configured-model"
    object.__setattr__(
        provider, "client", SimpleNamespace(responses=SimpleNamespace(create=request))
    )
    return StreamCase(provider, request, expected_input_tokens=11)


def _openai_compatible_stream_case() -> StreamCase:
    request = AsyncMock(return_value=AsyncEventStream(_openai_compatible_chunks()))
    provider = OpenAICompatibleProvider.__new__(OpenAICompatibleProvider)
    provider.model = "configured-model"
    provider.model_name = "configured-model"
    object.__setattr__(
        provider,
        "client",
        SimpleNamespace(chat=SimpleNamespace(completions=SimpleNamespace(create=request))),
    )
    return StreamCase(provider, request, expected_input_tokens=11)


def _gemini_stream_case() -> StreamCase:
    request = AsyncMock(return_value=AsyncEventStream(_gemini_chunks()))
    provider = GeminiProvider.__new__(GeminiProvider)
    provider.model = "configured-model"
    provider.model_name = "configured-model"
    object.__setattr__(
        provider,
        "client",
        SimpleNamespace(
            aio=SimpleNamespace(models=SimpleNamespace(generate_content_stream=request))
        ),
    )
    return StreamCase(
        provider,
        request,
        expected_input_tokens=9,
        expected_cache_read_tokens=2,
    )


def _bedrock_anthropic_stream_case() -> StreamCase:
    request = AsyncMock(return_value=AsyncEventStream(_anthropic_events()))
    provider = BedrockProvider.__new__(BedrockProvider)
    provider.model_id = "configured-model"
    provider.model_name = "configured-model"
    provider.model_family = "anthropic"
    object.__setattr__(
        provider, "client", SimpleNamespace(messages=SimpleNamespace(create=request))
    )
    return StreamCase(provider, request, expected_input_tokens=11)


def _bedrock_amazon_stream_case() -> StreamCase:
    request = MagicMock(return_value={"stream": iter(_bedrock_amazon_events())})
    provider = BedrockProvider.__new__(BedrockProvider)
    provider.model_id = "configured-model"
    provider.model_name = "configured-model"
    provider.model_family = "amazon"
    object.__setattr__(provider, "client", SimpleNamespace(converse_stream=request))
    return StreamCase(
        provider,
        request,
        expected_input_tokens=11,
        request_model_key="modelId",
    )


def stream_cases() -> list[tuple[str, Callable[[], StreamCase]]]:
    return [
        ("anthropic", _anthropic_stream_case),
        ("openai", _openai_stream_case),
        ("openai-compatible", _openai_compatible_stream_case),
        ("gemini", _gemini_stream_case),
        ("bedrock-anthropic", _bedrock_anthropic_stream_case),
        ("bedrock-amazon", _bedrock_amazon_stream_case),
    ]


def _anthropic_generate_case() -> GenerateCase:
    request = AsyncMock(
        return_value=SimpleNamespace(
            content=[SimpleNamespace(text="Weather: sunny")],
            usage=SimpleNamespace(
                input_tokens=11,
                output_tokens=7,
                cache_read_input_tokens=0,
                cache_creation_input_tokens=0,
            ),
        )
    )
    provider = AnthropicProvider.__new__(AnthropicProvider)
    provider.model = "configured-model"
    provider.model_name = "configured-model"
    object.__setattr__(
        provider, "client", SimpleNamespace(messages=SimpleNamespace(create=request))
    )
    return GenerateCase(provider, request, TokenUsage(11, 7))


def _openai_generate_case() -> GenerateCase:
    request = AsyncMock(
        return_value=SimpleNamespace(
            output_text="Weather: sunny",
            status="completed",
            usage=SimpleNamespace(
                input_tokens=11,
                output_tokens=7,
                input_tokens_details=SimpleNamespace(cached_tokens=0),
            ),
        )
    )
    provider = OpenAIProvider.__new__(OpenAIProvider)
    provider.model = "configured-model"
    provider.model_name = "configured-model"
    object.__setattr__(
        provider, "client", SimpleNamespace(responses=SimpleNamespace(create=request))
    )
    return GenerateCase(provider, request, TokenUsage(11, 7))


def _openai_compatible_generate_case() -> GenerateCase:
    request = AsyncMock(
        return_value=SimpleNamespace(
            choices=[SimpleNamespace(message=SimpleNamespace(content="Weather: sunny"))],
            usage=SimpleNamespace(prompt_tokens=11, completion_tokens=7),
        )
    )
    provider = OpenAICompatibleProvider.__new__(OpenAICompatibleProvider)
    provider.model = "configured-model"
    provider.model_name = "configured-model"
    object.__setattr__(
        provider,
        "client",
        SimpleNamespace(chat=SimpleNamespace(completions=SimpleNamespace(create=request))),
    )
    return GenerateCase(provider, request, TokenUsage(11, 7))


def _gemini_generate_case() -> GenerateCase:
    request = AsyncMock(
        return_value=SimpleNamespace(
            text="Weather: sunny",
            usage_metadata=SimpleNamespace(
                prompt_token_count=11,
                cached_content_token_count=2,
                candidates_token_count=7,
            ),
        )
    )
    provider = GeminiProvider.__new__(GeminiProvider)
    provider.model = "configured-model"
    provider.model_name = "configured-model"
    object.__setattr__(
        provider,
        "client",
        SimpleNamespace(
            aio=SimpleNamespace(models=SimpleNamespace(generate_content=request))
        ),
    )
    return GenerateCase(provider, request, TokenUsage(9, 7, cache_read_tokens=2))


def _bedrock_anthropic_generate_case() -> GenerateCase:
    request = AsyncMock(
        return_value=SimpleNamespace(
            content=[SimpleNamespace(text="Weather: sunny")],
            usage=SimpleNamespace(
                input_tokens=11,
                output_tokens=7,
                cache_read_input_tokens=0,
                cache_creation_input_tokens=0,
            ),
        )
    )
    provider = BedrockProvider.__new__(BedrockProvider)
    provider.model_id = "configured-model"
    provider.model_name = "configured-model"
    provider.model_family = "anthropic"
    object.__setattr__(
        provider, "client", SimpleNamespace(messages=SimpleNamespace(create=request))
    )
    return GenerateCase(provider, request, TokenUsage(11, 7))


def _bedrock_amazon_generate_case() -> GenerateCase:
    request = MagicMock(
        return_value={
            "usage": {"inputTokens": 11, "outputTokens": 7},
            "output": {"message": {"content": [{"text": "Weather: sunny"}]}},
        }
    )
    provider = BedrockProvider.__new__(BedrockProvider)
    provider.model_id = "configured-model"
    provider.model_name = "configured-model"
    provider.model_family = "amazon"
    object.__setattr__(provider, "client", SimpleNamespace(converse=request))
    return GenerateCase(provider, request, TokenUsage(11, 7), request_model_key="modelId")


def generate_cases() -> list[tuple[str, Callable[[], GenerateCase]]]:
    return [
        ("anthropic", _anthropic_generate_case),
        ("openai", _openai_generate_case),
        ("openai-compatible", _openai_compatible_generate_case),
        ("gemini", _gemini_generate_case),
        ("bedrock-anthropic", _bedrock_anthropic_generate_case),
        ("bedrock-amazon", _bedrock_amazon_generate_case),
    ]


class RecordingDelegate:
    def __init__(self) -> None:
        self.stream_kwargs: dict[str, object] | None = None
        self.generate_kwargs: dict[str, object] | None = None
        self.stream_events = _anthropic_events()
        self.error: ProviderError | None = None

    async def stream_response(self, **kwargs: object) -> AsyncIterator[MessageStreamEvent]:
        self.stream_kwargs = kwargs
        for event in self.stream_events:
            yield event

    async def generate_response(self, **kwargs: object) -> tuple[str, TokenUsage]:
        self.generate_kwargs = kwargs
        if self.error is not None:
            raise self.error
        return "ok", TokenUsage(input_tokens=1, output_tokens=2)


def wrapper_cases() -> list[tuple[type[AzureFoundryProvider] | type[VertexAIProvider], ProviderType]]:
    return [
        (AzureFoundryProvider, ProviderType.AZURE_FOUNDRY),
        (VertexAIProvider, ProviderType.VERTEX_AI),
    ]


# Keep this imported here so provider contract tests can use one JSON assertion helper.
def parse_tool_input(partial_json: str) -> dict[str, object]:
    value = json.loads(partial_json)
    if not isinstance(value, dict):
        raise AssertionError("tool input was not a JSON object")
    return value

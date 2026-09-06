from __future__ import annotations

import copy
from collections.abc import Callable
from typing import cast
from unittest.mock import AsyncMock, MagicMock

import pytest

from providers import ProviderError, ProviderType, TokenUsage
from providers.azure_foundry import AzureFoundryProvider
from providers.vertex_ai import VertexAIProvider
from tests.provider_contract import (
    ContractProvider,
    GenerateCase,
    RecordingDelegate,
    StreamCase,
    generate_cases,
    parse_tool_input,
    stream_cases,
    wrapper_cases,
)

pytestmark = pytest.mark.unit


@pytest.mark.asyncio
@pytest.mark.parametrize(
    ("provider_name", "case_factory"),
    stream_cases(),
    ids=[name for name, _ in stream_cases()],
)
async def test_stream_response_normalizes_provider_streams(
    provider_name: str, case_factory: Callable[[], StreamCase]
) -> None:
    case = case_factory()
    requested_model = "request-time-model"
    stream_kwargs: dict[str, object] = {
        "prompt": "Call get_weather for Bengaluru.",
        "tools": [copy.deepcopy(_weather_tool())],
        "system_prompt": "Be concise.",
        "model": requested_model,
    }

    provider = cast(ContractProvider, case.provider)
    events = [event async for event in provider.stream_response(**stream_kwargs)]
    event_types = [event.type for event in events]

    assert event_types[0] == "message_start"
    assert event_types[-1] == "message_stop"
    assert event_types.count("message_stop") == 1
    assert all(
        event_type
        in {
            "message_start",
            "content_block_start",
            "content_block_delta",
            "content_block_stop",
            "message_delta",
            "message_stop",
        }
        for event_type in event_types
    ), f"{provider_name} emitted an unknown event"

    started: set[int] = set()
    stopped: set[int] = set()
    text_parts: list[str] = []
    tool_json_parts: list[str] = []
    tool_block = None
    message_delta = None

    for event in events:
        if event.type == "content_block_start":
            assert event.index not in started
            started.add(event.index)
            if event.content_block.type == "tool_use":
                tool_block = event.content_block
        elif event.type == "content_block_delta":
            assert event.index in started
            assert event.index not in stopped
            if event.delta.type == "text_delta":
                text_parts.append(event.delta.text)
            elif event.delta.type == "input_json_delta":
                tool_json_parts.append(event.delta.partial_json)
        elif event.type == "content_block_stop":
            assert event.index in started
            assert event.index not in stopped
            stopped.add(event.index)
        elif event.type == "message_delta":
            message_delta = event

    assert started == stopped, f"{provider_name} left a content block open"
    assert "".join(text_parts) == "Weather: "
    assert tool_block is not None
    assert tool_block.id
    assert tool_block.name == "get_weather"
    assert parse_tool_input("".join(tool_json_parts)) == {"city": "Bengaluru"}
    assert message_delta is not None
    assert message_delta.usage.input_tokens == case.expected_input_tokens
    assert message_delta.usage.output_tokens == 7
    assert (getattr(message_delta.usage, "cache_read_input_tokens", 0) or 0) == (
        case.expected_cache_read_tokens
    )

    request_kwargs = _request_kwargs(case.request)
    assert request_kwargs[case.request_model_key] == requested_model


@pytest.mark.asyncio
@pytest.mark.parametrize(
    ("provider_name", "case_factory"),
    generate_cases(),
    ids=[name for name, _ in generate_cases()],
)
async def test_generate_response_returns_text_and_usage(
    provider_name: str, case_factory: Callable[[], GenerateCase]
) -> None:
    case = case_factory()
    requested_model = "request-time-model"
    kwargs: dict[str, object] = {
        "max_tokens": 32,
        "model": requested_model,
    }

    provider = cast(ContractProvider, case.provider)
    text, usage = await provider.generate_response("Summarize the weather.", **kwargs)

    assert text == "Weather: sunny", provider_name
    assert usage == case.expected_usage
    request_kwargs = _request_kwargs(case.request)
    assert request_kwargs[case.request_model_key] == requested_model


@pytest.mark.asyncio
@pytest.mark.parametrize(
    ("provider_class", "provider_type"),
    wrapper_cases(),
    ids=[name for name in ("azure-foundry", "vertex-ai")],
)
async def test_cloud_wrappers_forward_contract_and_rewrap_errors(
    provider_class: type[AzureFoundryProvider] | type[VertexAIProvider],
    provider_type: ProviderType,
) -> None:
    delegate = RecordingDelegate()
    provider = provider_class.__new__(provider_class)
    provider.model_name = "configured-model"
    object.__setattr__(provider, "_delegate", delegate)
    contract_provider = cast(ContractProvider, provider)

    events = [
        event
        async for event in contract_provider.stream_response(
            prompt="hello",
            max_tokens=32,
            tools=[copy.deepcopy(_weather_tool())],
            messages=[{"role": "user", "content": "hello"}],
            system_prompt="Be concise.",
            model="request-time-model",
        )
    ]
    assert events == delegate.stream_events
    assert delegate.stream_kwargs == {
        "prompt": "hello",
        "max_tokens": 32,
        "tools": [
            {
                "name": "get_weather",
                "description": "Get the weather for a city.",
                "input_schema": {
                    "type": "object",
                    "properties": {"city": {"type": "string"}},
                    "required": ["city"],
                },
            }
        ],
        "messages": [{"role": "user", "content": "hello"}],
        "system_prompt": "Be concise.",
        "model": "request-time-model",
    }

    assert await contract_provider.generate_response("hello", model="request-time-model") == (
        "ok",
        TokenUsage(input_tokens=1, output_tokens=2),
    )
    assert delegate.generate_kwargs == {
        "prompt": "hello",
        "max_tokens": None,
        "model": "request-time-model",
    }

    delegate.error = ProviderError(
        "delegate failed",
        provider_type=ProviderType.OPENAI,
        model="request-time-model",
        status_code=503,
        is_context_overflow=True,
    )
    with pytest.raises(ProviderError) as raised:
        await contract_provider.generate_response("hello", model="request-time-model")

    assert raised.value.provider_type is provider_type
    assert raised.value.message == "delegate failed"
    assert raised.value.model == "request-time-model"
    assert raised.value.status_code == 503
    assert raised.value.is_context_overflow


def _weather_tool() -> dict[str, object]:
    return {
        "name": "get_weather",
        "description": "Get the weather for a city.",
        "input_schema": {
            "type": "object",
            "properties": {"city": {"type": "string"}},
            "required": ["city"],
        },
    }


def _request_kwargs(request: AsyncMock | MagicMock) -> dict[str, object]:
    call = request.await_args if isinstance(request, AsyncMock) else request.call_args
    assert call is not None
    return dict(call.kwargs)


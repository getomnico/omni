from __future__ import annotations

import json
from types import SimpleNamespace

import pytest

from providers.openai import OpenAIProvider

pytestmark = pytest.mark.unit


def test_convert_messages_does_not_forward_search_result_extras():
    provider = OpenAIProvider.__new__(OpenAIProvider)
    messages = [
        {
            "role": "user",
            "content": [
                {
                    "type": "tool_result",
                    "tool_use_id": "call-1",
                    "content": [
                        {
                            "type": "search_result",
                            "title": "Issue",
                            "source": "https://example.invalid/issue",
                            "source_type": "jira",
                            "internal_extra": "must-not-be-sent",
                            "content": [{"type": "text", "text": "body"}],
                        }
                    ],
                }
            ],
        }
    ]

    converted = provider._convert_messages(messages)

    encoded = json.dumps(converted)
    assert "source_type" not in encoded
    assert "must-not-be-sent" not in encoded
    assert converted == [
        {
            "type": "function_call_output",
            "call_id": "call-1",
            "output": "[Issue](https://example.invalid/issue)\nbody",
        }
    ]

    internal_search_result = messages[0]["content"][0]["content"][0]
    assert internal_search_result["source_type"] == "jira"
    assert internal_search_result["internal_extra"] == "must-not-be-sent"


class _FakeStream:
    def __aiter__(self):
        return self._events()

    async def _events(self):
        yield SimpleNamespace(
            type="response.completed", response=SimpleNamespace(usage=None)
        )


class _FakeResponses:
    def __init__(self, response=None):
        self.params = None
        self.response = response or SimpleNamespace(
            status="completed", usage=None, output_text="ok"
        )

    async def create(self, **params):
        self.params = params
        return self.response


def _provider_with_fake_client(model: str, response=None):
    responses = _FakeResponses(response=response)
    provider = OpenAIProvider.__new__(OpenAIProvider)
    provider.model = model
    provider.client = SimpleNamespace(responses=responses)
    return provider, responses


@pytest.mark.parametrize(
    "model",
    ["gpt-5", "gpt-5.2", "gpt-5.4", "gpt-5.4-mini", "gpt-5.5"],
)
@pytest.mark.asyncio
async def test_generate_response_omits_sampling_params_for_gpt5(model):
    provider, responses = _provider_with_fake_client(model)

    await provider.generate_response("title", max_tokens=20, temperature=0.7, top_p=0.9)

    assert responses.params["model"] == model
    assert responses.params["max_output_tokens"] == 1024
    assert "temperature" not in responses.params
    assert "top_p" not in responses.params


@pytest.mark.parametrize("model", ["gpt-4o", "gpt-4.1", "gpt-4.1-mini"])
@pytest.mark.asyncio
async def test_generate_response_forwards_sampling_params_for_supported_models(model):
    provider, responses = _provider_with_fake_client(model)

    await provider.generate_response("title", max_tokens=20, temperature=0.7, top_p=0.9)

    assert responses.params["max_output_tokens"] == 20
    assert responses.params["temperature"] == 0.7
    assert responses.params["top_p"] == 0.9


@pytest.mark.asyncio
async def test_stream_response_omits_sampling_params_for_gpt5():
    provider, responses = _provider_with_fake_client("gpt-5.2", response=_FakeStream())

    events = [
        event
        async for event in provider.stream_response(
            "hello", max_tokens=20, temperature=0.7, top_p=0.9
        )
    ]

    assert responses.params["max_output_tokens"] == 1024
    assert "temperature" not in responses.params
    assert "top_p" not in responses.params
    assert events[-1].type == "message_stop"


@pytest.mark.asyncio
async def test_health_check_uses_reasoning_model_token_floor():
    provider, responses = _provider_with_fake_client("gpt-5.2")

    assert await provider.health_check() is True
    assert responses.params["max_output_tokens"] == 1024

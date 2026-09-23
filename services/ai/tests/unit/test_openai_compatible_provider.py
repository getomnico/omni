from __future__ import annotations

import json
from types import SimpleNamespace

import httpx
import pytest
from openai import APIStatusError

from providers.openai_compatible import (
    REASONING_CONTENT_KEY,
    _convert_messages_to_openai,
    _get_passthrough_delta_value,
    validate_openai_tool_message_sequence,
)


def test_convert_messages_preserves_deepseek_reasoning_content_on_assistant_message():
    messages = [
        {
            "role": "assistant",
            "content": [
                {
                    "type": "text",
                    "text": "Final answer",
                },
                {
                    "type": "text",
                    "text": "",
                    REASONING_CONTENT_KEY: "private chain of thought token",
                },
            ],
        }
    ]

    converted = _convert_messages_to_openai(messages)

    assert converted == [
        {
            "role": "assistant",
            "content": "Final answer",
            REASONING_CONTENT_KEY: "private chain of thought token",
        }
    ]


def test_convert_messages_preserves_deepseek_reasoning_content_with_tool_calls():
    messages = [
        {
            "role": "assistant",
            "content": [
                {
                    "type": "tool_use",
                    "id": "call_1",
                    "name": "search",
                    "input": {"query": "omni"},
                },
                {
                    "type": "text",
                    "text": "",
                    REASONING_CONTENT_KEY: "tool reasoning token",
                },
            ],
        }
    ]

    converted = _convert_messages_to_openai(messages)

    assert len(converted) == 1
    assistant_message = converted[0]
    assert assistant_message["role"] == "assistant"
    assert assistant_message[REASONING_CONTENT_KEY] == "tool reasoning token"
    assert "content" not in assistant_message
    assert assistant_message["tool_calls"][0]["id"] == "call_1"


def test_convert_messages_does_not_forward_search_result_extras():
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

    converted = _convert_messages_to_openai(messages)

    encoded = json.dumps(converted)
    assert "source_type" not in encoded
    assert "must-not-be-sent" not in encoded
    assert converted == [
        {
            "role": "tool",
            "tool_call_id": "call-1",
            "content": "[Issue](https://example.invalid/issue)\nbody",
        }
    ]

    internal_search_result = messages[0]["content"][0]["content"][0]
    assert internal_search_result["source_type"] == "jira"
    assert internal_search_result["internal_extra"] == "must-not-be-sent"


def test_get_passthrough_delta_value_reads_pydantic_extra():
    class Delta:
        model_extra = {REASONING_CONTENT_KEY: "reasoning delta"}

    assert (
        _get_passthrough_delta_value(Delta(), REASONING_CONTENT_KEY)
        == "reasoning delta"
    )


def _assistant_with_tool_calls(tool_call_ids: list[str]) -> dict:
    return {
        "role": "assistant",
        "content": [
            {
                "type": "tool_use",
                "id": tool_id,
                "name": "search",
                "input": {},
            }
            for tool_id in tool_call_ids
        ],
    }


def _tool_message(tool_call_id: str) -> dict:
    return {
        "role": "tool",
        "tool_call_id": tool_call_id,
        "content": "result",
    }


def _openai_assistant_with_tool_calls(tool_call_ids: list[str]) -> dict:
    """An OpenAI-format assistant message carrying tool_calls (as the validator
    sees them post-conversion)."""
    return {
        "role": "assistant",
        "content": None,
        "tool_calls": [
            {
                "id": tool_id,
                "type": "function",
                "function": {"name": "search", "arguments": "{}"},
            }
            for tool_id in tool_call_ids
        ],
    }


def test_validate_accepts_parallel_tool_messages():
    converted = _convert_messages_to_openai(
        [
            {"role": "user", "content": "hi"},
            _assistant_with_tool_calls(["call_A", "call_B"]),
            {
                "role": "user",
                "content": [
                    {"type": "tool_result", "tool_use_id": "call_A", "content": "r1"},
                    {"type": "tool_result", "tool_use_id": "call_B", "content": "r2"},
                ],
            },
            _assistant_with_tool_calls(["call_C"]),
            {
                "role": "user",
                "content": [
                    {"type": "tool_result", "tool_use_id": "call_C", "content": "r3"}
                ],
            },
        ]
    )

    validate_openai_tool_message_sequence(converted)


def test_validate_rejects_duplicate_tool_call_id():
    converted = [
        _openai_assistant_with_tool_calls(["call_A"]),
        _tool_message("call_A"),
        _tool_message("call_A"),
    ]

    with pytest.raises(ValueError, match="no matching tool_calls"):
        validate_openai_tool_message_sequence(converted)


def test_validate_rejects_missing_tool_result_before_user_message():
    """An assistant batch must be fully satisfied before any non-tool message."""
    converted = [
        _openai_assistant_with_tool_calls(["call_A", "call_B"]),
        _tool_message("call_A"),
        {"role": "user", "content": "new question"},
    ]

    with pytest.raises(ValueError, match="never answered"):
        validate_openai_tool_message_sequence(converted)


def test_validate_rejects_tool_call_answered_after_user_message():
    """A tool message cannot be paired with an assistant batch across a user
    message boundary."""
    converted = [
        _openai_assistant_with_tool_calls(["call_A"]),
        {"role": "user", "content": "new question"},
        _tool_message("call_A"),
    ]

    with pytest.raises(ValueError, match="never answered"):
        validate_openai_tool_message_sequence(converted)


def test_validate_rejects_tool_message_after_user_message():
    converted = [
        _openai_assistant_with_tool_calls(["call_A"]),
        _tool_message("call_A"),
        {"role": "user", "content": "new question"},
        _tool_message("call_A"),
    ]

    with pytest.raises(ValueError, match="no matching tool_calls"):
        validate_openai_tool_message_sequence(converted)


def test_validate_rejects_consecutive_assistant_tool_call_messages():
    converted = [
        _openai_assistant_with_tool_calls(["call_A"]),
        _openai_assistant_with_tool_calls(["call_B"]),
    ]

    with pytest.raises(ValueError, match="never answered"):
        validate_openai_tool_message_sequence(converted)


def test_validate_rejects_trailing_unanswered_tool_calls():
    converted = [_openai_assistant_with_tool_calls(["call_A"])]

    with pytest.raises(ValueError, match="never answered"):
        validate_openai_tool_message_sequence(converted)


def test_validate_rejects_tool_message_without_preceding_assistant():
    converted = [_tool_message("call_A")]

    with pytest.raises(ValueError, match="no matching tool_calls"):
        validate_openai_tool_message_sequence(converted)


def test_convert_messages_includes_user_text_document():
    document = {
        "type": "document",
        "title": "Report.pdf",
        "source": {"type": "text", "data": "Q3 revenue grew 14%."},
    }

    converted = _convert_messages_to_openai(
        [{"role": "user", "content": [{"type": "text", "text": "Review:"}, document]}]
    )

    assert converted == [
        {
            "role": "user",
            "content": (
                'Review:\nDocument title: "Report.pdf"\n'
                "Document content:\nQ3 revenue grew 14%."
            ),
        }
    ]


def test_convert_messages_omits_assistant_and_unsupported_documents():
    text_document = {
        "type": "document",
        "source": {"type": "text", "data": "untrusted"},
    }
    binary_document = {
        "type": "document",
        "source": {"type": "base64", "data": "ignored"},
    }

    converted = _convert_messages_to_openai(
        [
            {"role": "assistant", "content": [text_document]},
            {
                "role": "user",
                "content": [{"type": "text", "text": "safe"}, binary_document],
            },
        ]
    )

    assert converted == [{"role": "assistant"}, {"role": "user", "content": "safe"}]


def test_convert_messages_inlines_base64_image_for_user():
    converted = _convert_messages_to_openai(
        [
            {
                "role": "user",
                "content": [
                    {
                        "type": "image",
                        "source": {
                            "type": "base64",
                            "media_type": "image/png",
                            "data": "aGVsbG8=",
                        },
                    },
                    {"type": "text", "text": "what is this?"},
                ],
            }
        ]
    )

    assert len(converted) == 1
    content = converted[0]["content"]
    assert isinstance(content, list)
    assert content[0] == {
        "type": "image_url",
        "image_url": {"url": "data:image/png;base64,aGVsbG8="},
    }
    assert content[1] == {"type": "text", "text": "what is this?"}


def test_convert_messages_pure_text_user_stays_single_string():
    converted = _convert_messages_to_openai(
        [
            {
                "role": "user",
                "content": [
                    {"type": "text", "text": "hello"},
                    {"type": "text", "text": "world"},
                ],
            }
        ]
    )

    assert converted == [{"role": "user", "content": "hello\nworld"}]


def test_convert_messages_ignores_non_base64_image_source():
    converted = _convert_messages_to_openai(
        [
            {
                "role": "user",
                "content": [
                    {"type": "image", "source": {"type": "url", "url": "https://x.invalid/a.png"}},
                    {"type": "text", "text": "hi"},
                ],
            }
        ]
    )

    assert converted == [{"role": "user", "content": "hi"}]


# --- Endpoint vision-capability metadata ---------------------------------

_CATALOG = {
    "data": [
        {
            "id": "vendor/vision-model",
            "architecture": {"input_modalities": ["text", "image"]},
        },
        {
            "id": "vendor/text-model",
            "architecture": {"input_modalities": ["text"]},
        },
        {"id": "vendor/no-architecture"},
    ]
}


class _FakeResponse:
    def __init__(self, payload, status_code=200):
        self._payload = payload
        self.status_code = status_code

    def raise_for_status(self):
        if self.status_code >= 400:
            raise RuntimeError(f"HTTP {self.status_code}")

    def json(self):
        return self._payload


class _FakeAsyncClient:
    routes: dict[tuple[str, str], _FakeResponse] = {}
    constructions = 0

    def __init__(self, **kwargs):
        type(self).constructions += 1

    async def __aenter__(self):
        return self

    async def __aexit__(self, *exc_info):
        return False

    async def get(self, url, **kwargs):
        return type(self).routes[("GET", url)]

    async def post(self, url, **kwargs):
        return type(self).routes[("POST", url)]


class _FakeStatusError(APIStatusError):
    def __init__(self, status_code, message, body=None):
        super().__init__(
            message,
            response=httpx.Response(
                status_code, request=httpx.Request("POST", "http://probe.test")
            ),
            body=body,
        )


@pytest.fixture(autouse=True)
def _reset_probe_caches(monkeypatch):
    import providers.openai_compatible as oc

    monkeypatch.setattr(oc, "_openrouter_catalog_cache", {})
    monkeypatch.setattr(oc, "_ollama_vision_cache", {})
    monkeypatch.setattr(oc, "_live_vision_cache", {})


def _provider_with_client(monkeypatch, outcome="ok", exc=None):
    """Provider whose SDK client answers the live probe with `outcome`/`exc`."""
    import providers.openai_compatible as oc

    provider = oc.OpenAICompatibleProvider(
        base_url="http://gpu-box:8000/v1", model="some-model"
    )
    calls = {"count": 0}

    async def create(**kwargs):
        calls["count"] += 1
        if exc is not None:
            raise exc
        assert kwargs["messages"][0]["content"][1]["type"] == "image_url"
        return object()

    provider.client = SimpleNamespace(
        chat=SimpleNamespace(completions=SimpleNamespace(create=create))
    )
    return provider, calls


@pytest.mark.asyncio
async def test_generic_endpoint_live_probe_accepts_image_and_caches(monkeypatch):
    provider, calls = _provider_with_client(monkeypatch, outcome="ok")

    assert await provider.supports_vision("some-model") is True
    assert await provider.supports_vision("some-model") is True
    assert calls["count"] == 1


@pytest.mark.asyncio
async def test_live_probe_image_rejection_means_no_vision(monkeypatch):
    from providers.openai_compatible import _probe_error_mentions_images

    exc = _FakeStatusError(
        400,
        "Model does not support image input",
        body={"error": {"message": "Model does not support image input"}},
    )
    assert _probe_error_mentions_images(exc)
    provider, _ = _provider_with_client(monkeypatch, exc=exc)

    assert await provider.supports_vision("some-model") is False


@pytest.mark.asyncio
async def test_live_probe_auth_failure_stays_unknown(monkeypatch):
    exc = _FakeStatusError(401, "Incorrect API key provided")
    provider, _ = _provider_with_client(monkeypatch, exc=exc)

    assert await provider.supports_vision("some-model") is None


@pytest.mark.asyncio
async def test_live_probe_server_error_stays_unknown(monkeypatch):
    exc = _FakeStatusError(500, "image pipeline exploded")
    provider, _ = _provider_with_client(monkeypatch, exc=exc)

    assert await provider.supports_vision("some-model") is None


def test_vision_and_all_ids_from_catalog_parses_modalities():
    from providers.openai_compatible import _vision_and_all_ids_from_catalog

    vision, all_ids = _vision_and_all_ids_from_catalog(_CATALOG)
    assert vision == frozenset({"vendor/vision-model"})
    assert all_ids == frozenset(
        {"vendor/vision-model", "vendor/text-model", "vendor/no-architecture"}
    )
    assert _vision_and_all_ids_from_catalog({"data": "junk"}) == (frozenset(), frozenset())
    assert _vision_and_all_ids_from_catalog(None) == (frozenset(), frozenset())


def test_catalog_match_is_tri_state():
    from providers.openai_compatible import _catalog_supports_images

    vision_ids = frozenset({"vendor/vision-model"})
    all_ids = frozenset({"vendor/vision-model", "vendor/text-model"})

    assert _catalog_supports_images(vision_ids, all_ids, "vendor/vision-model") is True
    assert (
        _catalog_supports_images(vision_ids, all_ids, "vendor/vision-model:free")
        is True
    )
    assert _catalog_supports_images(vision_ids, all_ids, "vendor/text-model") is False
    assert _catalog_supports_images(vision_ids, all_ids, "vendor/unknown") is None


def test_ollama_payload_vision_capability():
    from providers.openai_compatible import _ollama_supports_vision

    assert _ollama_supports_vision({"capabilities": ["completion", "vision"]})
    assert not _ollama_supports_vision({"capabilities": ["completion"]})
    assert not _ollama_supports_vision(None)


@pytest.mark.asyncio
async def test_openrouter_probe_reads_public_catalog(monkeypatch):
    import providers.openai_compatible as oc

    monkeypatch.setattr(oc.httpx, "AsyncClient", _FakeAsyncClient)
    _FakeAsyncClient.constructions = 0
    _FakeAsyncClient.routes = {
        ("GET", "https://openrouter.ai/api/v1/models"): _FakeResponse(_CATALOG)
    }

    provider = oc.OpenAICompatibleProvider(
        base_url="https://openrouter.ai/api/v1", model="vendor/vision-model"
    )
    assert await provider.supports_vision("vendor/vision-model") is True
    assert await provider.supports_vision("vendor/text-model") is False
    assert await provider.supports_vision("vendor/vision-model:free") is True

    # The catalog is fetched once and reused across models.
    assert _FakeAsyncClient.constructions == 1


@pytest.mark.asyncio
async def test_openrouter_unknown_model_falls_back_to_live_probe(monkeypatch):
    import providers.openai_compatible as oc

    monkeypatch.setattr(oc.httpx, "AsyncClient", _FakeAsyncClient)
    _FakeAsyncClient.routes = {
        ("GET", "https://openrouter.ai/api/v1/models"): _FakeResponse(_CATALOG)
    }
    provider, calls = _provider_with_client(monkeypatch, outcome="ok")

    assert await provider.supports_vision("vendor/not-in-catalog") is True
    assert calls["count"] == 1


@pytest.mark.asyncio
async def test_openrouter_probe_failure_falls_back_to_live_probe(monkeypatch):
    import providers.openai_compatible as oc

    monkeypatch.setattr(oc.httpx, "AsyncClient", _FakeAsyncClient)
    _FakeAsyncClient.routes = {
        ("GET", "https://openrouter.ai/api/v1/models"): _FakeResponse({}, status_code=500)
    }
    provider, calls = _provider_with_client(monkeypatch, outcome="ok")

    assert await provider.supports_vision("vendor/vision-model") is True
    assert calls["count"] == 1


@pytest.mark.asyncio
async def test_ollama_probe_asks_api_show(monkeypatch):
    import providers.openai_compatible as oc

    monkeypatch.setattr(oc.httpx, "AsyncClient", _FakeAsyncClient)
    _FakeAsyncClient.routes = {
        ("POST", "http://localhost:11434/api/show"): _FakeResponse(
            {"capabilities": ["completion", "vision"]}
        )
    }

    provider = oc.OpenAICompatibleProvider(
        base_url="http://localhost:11434/v1", model="llava:13b"
    )
    assert await provider.supports_vision("llava:13b") is True


@pytest.mark.asyncio
async def test_ollama_unknown_model_falls_back_to_live_probe(monkeypatch):
    import providers.openai_compatible as oc

    monkeypatch.setattr(oc.httpx, "AsyncClient", _FakeAsyncClient)
    _FakeAsyncClient.routes = {
        ("POST", "http://localhost:11434/api/show"): _FakeResponse({}, status_code=404)
    }
    provider, calls = _provider_with_client(monkeypatch, outcome="ok")

    assert await provider.supports_vision("ghost") is True
    assert calls["count"] == 1

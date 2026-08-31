"""Integration test for the Gemini provider — live tool-call round trip."""

import json
import os
import re
from pathlib import Path
from unittest.mock import AsyncMock

import pytest
from anthropic.types import MessageParam
from dotenv import load_dotenv

from providers.gemini import GeminiProvider
from services.usage import UsageContext, UsagePurpose, UsageTracker
from tests.integration.test_providers_api.conftest import (
    report_usage,
    require_env,
    stream_usage,
)

pytestmark = pytest.mark.real_llm

# Local verification reads the key from a repo-root .env.test
# (GEMINI_API_KEY); CI provides TEST_GEMINI_API_KEY.
_env_test = Path(__file__).resolve().parents[5] / ".env.test"
if _env_test.exists():
    load_dotenv(_env_test)

API_KEY = os.environ.get("GEMINI_API_KEY") or require_env("TEST_GEMINI_API_KEY")
MODEL = os.environ.get("TEST_GEMINI_MODEL", "gemini-3.5-flash-lite")

_WEATHER_TOOL = {
    "name": "get_weather",
    "description": "Get the current weather for a city.",
    "input_schema": {
        "type": "object",
        "properties": {"city": {"type": "string"}},
        "required": ["city"],
    },
}

# Mirrors a connector action schema (e.g. darwinbox) that declares
# `additionalProperties: false`. The genai SDK serializes that key as the
# snake_case `additional_properties`, which the Gemini API rejects with a 400
# INVALID_ARGUMENT — the provider must strip it before sending. Keeping it in
# the live round trip guards that path end-to-end: if sanitization regresses,
# this request 400s before a single event streams.
_DARWINBOX_STYLE_TOOL = {
    "name": "darwinbox__get_attendance",
    "description": "Get attendance for the calling employee.",
    "input_schema": {
        "type": "object",
        "properties": {
            "from_date": {"type": "string"},
            "to_date": {"type": "string"},
            "month": {"type": "string"},
        },
        "additionalProperties": False,
    },
}

_TOOLS = [_WEATHER_TOOL, _DARWINBOX_STYLE_TOOL]


class TestGemini:
    async def test_stream(self) -> None:
        """Stream a forced function call and continue with the tool result.

        Gemini 3 attaches an opaque ``thought_signature`` to function-call
        parts (thinking-with-tools); the provider encodes it as the
        ``_gemini_thought_signature`` sidecar on the ``tool_use`` block,
        which must survive the rebuild and round trip.  The follow-up
        request must be accepted by the API.

        The tool list also includes a darwinbox-style schema with
        ``additionalProperties: false`` (see ``_DARWINBOX_STYLE_TOOL``) to
        guard the provider's schema sanitization against the real API.
        """
        provider = GeminiProvider(api_key=API_KEY, model=MODEL)
        messages: list[MessageParam] = [
            {
                "role": "user",
                "content": "What is the weather in Bangalore? Use the get_weather tool.",
            }
        ]

        events = [
            e
            async for e in provider.stream_response(
                prompt="",
                messages=messages,
                tools=_TOOLS,
                max_tokens=512,
            )
        ]
        assert events[-1].type == "message_stop"

        # Rebuild the assistant content blocks exactly as streamed, the way
        # the chat pipeline persists them (text/tool_use, in order).  The
        # ``_gemini_thought_signature`` sidecar travels on the blocks.
        assistant_blocks: list[dict] = []
        tool_use_id: str | None = None
        for event in events:
            if event.type == "content_block_start":
                block = event.content_block
                if block.type == "tool_use":
                    tool_use_id = block.id
                    rebuilt = {
                        "type": "tool_use",
                        "id": block.id,
                        "name": block.name,
                        "input": "",
                    }
                    sig = getattr(block, "_gemini_thought_signature", None)
                    if sig is not None:
                        rebuilt["_gemini_thought_signature"] = sig
                    assistant_blocks.append(rebuilt)
                elif block.type == "text":
                    assistant_blocks.append({"type": "text", "text": block.text})
            elif event.type == "content_block_delta":
                if event.delta.type == "input_json_delta":
                    tool_block = next(
                        b for b in assistant_blocks if b["type"] == "tool_use"
                    )
                    tool_block["input"] += event.delta.partial_json
                elif event.delta.type == "text_delta":
                    text_block = next(
                        b for b in assistant_blocks if b["type"] == "text"
                    )
                    text_block["text"] += event.delta.text

        assert tool_use_id is not None, "Model did not call get_weather"
        assert re.fullmatch(
            r"[a-zA-Z0-9_-]+", tool_use_id
        ), f"Invalid tool_use id: {tool_use_id!r}"

        tool_block = next(b for b in assistant_blocks if b["type"] == "tool_use")
        tool_block["input"] = json.loads(tool_block["input"])

        # Continuation with the tool_result — must be accepted by the API.
        continuation: list[MessageParam] = [
            messages[0],
            {"role": "assistant", "content": assistant_blocks},
            {
                "role": "user",
                "content": [
                    {
                        "type": "tool_result",
                        "tool_use_id": tool_use_id,
                        "content": "The weather in Bangalore is sunny and 28C.",
                    }
                ],
            },
        ]
        events2 = [
            e
            async for e in provider.stream_response(
                prompt="",
                messages=continuation,
                tools=_TOOLS,
                max_tokens=512,
            )
        ]
        assert events2[-1].type == "message_stop"

        # Usage must actually flow through the streamed events — Gemini
        # reports real token counts on the final ``message_delta``, and the
        # chat pipeline's UsageTracker reads exactly that.  Regression guard:
        # if the API stops reporting usage_metadata (or the provider stops
        # forwarding it), tracking silently records nothing.
        u = stream_usage(events) or stream_usage(events2)
        assert u is not None, (
            "Gemini stream carried no usage on message_delta; "
            "token usage tracking would silently record nothing"
        )
        report_usage("Gemini.stream", u[0], u[1])


class TestGeminiCachedUsage:
    """Cache-hit usage accounting against the real API.

    Gemini's ``usage_metadata.prompt_token_count`` is the TOTAL prompt size
    and includes the cached portion; the provider must report
    ``input_tokens`` as ``prompt - cached`` so cached tokens are not double
    counted against ``cache_read_input_tokens``.  Uses an explicitly created
    context cache so the hit is guaranteed.
    """

    @staticmethod
    def _delta_usage(events: list) -> tuple[int, int, int] | None:
        """Extract (input, output, cache_read) from the message_delta usage."""
        for event in events:
            if event.type == "message_delta" and event.usage is not None:
                u = event.usage
                return (
                    getattr(u, "input_tokens", 0) or 0,
                    getattr(u, "output_tokens", 0) or 0,
                    getattr(u, "cache_read_input_tokens", 0) or 0,
                )
        return None

    async def test_cached_input_tokens_exclude_cache_hits(self) -> None:
        """Cached input tokens must be excluded from ``input_tokens``.

        Gemini's ``prompt_token_count`` is the TOTAL prompt size and
        includes the cached portion (verified against the live API:
        prompt = content + cached).  The provider must report
        ``input_tokens`` as ``total - cached`` (with ``cache_read`` carrying
        the cached portion), otherwise cached tokens are double counted.
        ``count_tokens`` gives the independent uncached total to check
        against.  Fails on the old mapping (input = full prompt total).
        """
        from google.genai import types

        provider = GeminiProvider(api_key=API_KEY, model=MODEL)
        long_text = "The quick brown fox jumps over the lazy dog. " * 600
        messages: list[MessageParam] = [{"role": "user", "content": long_text}]

        # Explicit context cache over the same content → guaranteed cache hit.
        cache = await provider.client.aio.caches.create(
            model=MODEL,
            config=types.CreateCachedContentConfig(
                display_name="omni-usage-test",
                contents=[
                    types.Content(role="user", parts=[types.Part(text=long_text)])
                ],
                ttl="300s",
            ),
        )
        try:
            real_stream = provider.client.aio.models.generate_content_stream

            def _stream_with_cache(**kwargs):
                # Inject the cached content into the request the provider
                # builds, keeping the real provider stream path intact.
                cfg = kwargs.get("config")
                if cfg is not None:
                    cfg_dump = cfg.model_dump()
                    cfg_dump["cached_content"] = cache.name
                    kwargs["config"] = types.GenerateContentConfig(**cfg_dump)
                else:
                    kwargs["config"] = types.GenerateContentConfig(
                        cached_content=cache.name, max_output_tokens=32
                    )
                return real_stream(**kwargs)

            provider.client.aio.models.generate_content_stream = _stream_with_cache

            # Through the real pipeline UsageTracker.
            tracker = UsageTracker(
                repo=AsyncMock(),
                ctx=UsageContext(
                    user_id="u",
                    model_id="m",
                    model_name=MODEL,
                    provider_type="gemini",
                    purpose=UsagePurpose.CHAT,
                ),
            )
            events = [
                e
                async for e in tracker.wrap_stream(
                    provider.stream_response(
                        prompt="", messages=messages, max_tokens=32
                    )
                )
            ]
        finally:
            await provider.client.aio.caches.delete(name=cache.name)

        assert events[-1].type == "message_stop"
        u = self._delta_usage(events)
        assert u is not None, "No usage on message_delta"
        input_uncached, out, cached = u

        # Independent uncached total: the request content's own token count.
        count = await provider.client.aio.models.count_tokens(
            model=MODEL, contents=long_text
        )
        expected_uncached = count.total_tokens
        assert cached > 0, (
            "Expected a cache hit from the explicit context cache "
            f"(model={MODEL}); cached tokens = {cached}"
        )
        assert input_uncached == expected_uncached, (
            f"uncached input ({input_uncached}) != count_tokens "
            f"({expected_uncached}); cached tokens are being double counted"
        )
        assert out > 0

        # The pipeline's UsageTracker must capture the same split.
        assert tracker.input_tokens == input_uncached
        assert tracker.cache_read_tokens == cached
        assert tracker.output_tokens == out

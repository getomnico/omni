"""Integration test for the Gemini provider — live tool-call round trip."""

import json
import os
import re

import pytest
from anthropic.types import MessageParam

from tests.integration.test_providers_api.conftest import (
    require_env,
    stream_usage,
    report_usage,
)
from providers.gemini import GeminiProvider

pytestmark = pytest.mark.real_llm

API_KEY = require_env("TEST_GEMINI_API_KEY")
MODEL = os.environ.get("TEST_GEMINI_MODEL", "gemini-2.5-flash")

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

        u = stream_usage(events) or stream_usage(events2)
        if u:
            report_usage("Gemini.stream", u[0], u[1])

"""Integration test for the Anthropic provider — live tool-call round trip."""

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
from providers.anthropic import AnthropicProvider

pytestmark = pytest.mark.real_llm

API_KEY = require_env("TEST_ANTHROPIC_API_KEY")
MODEL = os.environ.get("TEST_ANTHROPIC_MODEL", "claude-haiku-4-5")

_WEATHER_TOOL = {
    "name": "get_weather",
    "description": "Get the current weather for a city.",
    "input_schema": {
        "type": "object",
        "properties": {"city": {"type": "string"}},
        "required": ["city"],
    },
}

# Force extended thinking with the smallest legal budget so the test
# deterministically exercises the thinking-block code path (Anthropic
# requires budget_tokens >= 1024 and < max_tokens).
_THINKING = {"type": "enabled", "budget_tokens": 1024}
_MAX_TOKENS = 2048


def _rebuild_assistant_blocks(
    events: list,
) -> tuple[list[dict], str | None, str | None]:
    """Rebuild the assistant content blocks exactly as streamed, the way the
    chat pipeline persists them (thinking + tool_use, in order).

    Returns ``(blocks, tool_use_id, thinking_signature)``.
    """
    assistant_blocks: list[dict] = []
    tool_use_id: str | None = None
    thinking_sig: str | None = None
    for event in events:
        if event.type == "content_block_start":
            block = event.content_block
            if block.type == "tool_use":
                tool_use_id = block.id
                assistant_blocks.append(
                    {
                        "type": "tool_use",
                        "id": block.id,
                        "name": block.name,
                        "input": "",
                    }
                )
            elif block.type == "thinking":
                assistant_blocks.append(
                    {"type": "thinking", "thinking": block.thinking, "signature": ""}
                )
        elif event.type == "content_block_delta":
            if event.delta.type == "input_json_delta":
                tool_block = next(
                    b for b in assistant_blocks if b["type"] == "tool_use"
                )
                tool_block["input"] += event.delta.partial_json
            elif event.delta.type == "thinking_delta":
                thinking_block = next(
                    b for b in assistant_blocks if b["type"] == "thinking"
                )
                thinking_block["thinking"] += event.delta.thinking
            elif event.delta.type == "signature_delta":
                thinking_sig = event.delta.signature
                thinking_block = next(
                    b for b in assistant_blocks if b["type"] == "thinking"
                )
                thinking_block["signature"] = event.delta.signature
    return assistant_blocks, tool_use_id, thinking_sig


class TestAnthropic:
    @pytest.mark.parametrize(
        "thinking", [None, _THINKING], ids=["no-thinking", "thinking"]
    )
    async def test_stream(self, thinking: dict | None) -> None:
        """Stream a forced tool call and continue with the tool result.

        With ``thinking`` enabled, the model streams an extended-thinking
        block before the tool_use, shifting the tool_use (and its input
        deltas) to a higher content-block index.  The assistant message must
        be rebuilt from the streamed blocks exactly, and the follow-up
        request must be accepted by the API — this is the request shape that
        failed with a 400 on ``tool_use.id`` when an empty id leaked into
        the persisted history.
        """
        provider = AnthropicProvider(api_key=API_KEY, model=MODEL)
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
                tools=[_WEATHER_TOOL],
                max_tokens=_MAX_TOKENS,
                thinking=thinking,
            )
        ]
        assert events[-1].type == "message_stop"

        assistant_blocks, tool_use_id, thinking_sig = _rebuild_assistant_blocks(events)

        if thinking is not None:
            assert thinking_sig, (
                "Expected a thinking block with a signature "
                "(thinking was enabled but no thinking block was streamed)"
            )
        assert tool_use_id is not None, "Model did not call get_weather"
        assert re.fullmatch(
            r"[a-zA-Z0-9_-]+", tool_use_id
        ), f"Invalid tool_use id: {tool_use_id!r}"

        # The API streams tool input as partial JSON; the pipeline persists
        # it as a parsed object, which is what the follow-up request sends.
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
                tools=[_WEATHER_TOOL],
                max_tokens=_MAX_TOKENS,
                thinking=thinking,
            )
        ]
        assert events2[-1].type == "message_stop"

        u = stream_usage(events) or stream_usage(events2)
        if u:
            report_usage("Anthropic.stream", u[0], u[1])

"""Integration test for the Anthropic provider — live tool-call round trip."""

import os
import re

import pytest
from anthropic.types import MessageParam, ToolUseBlockParam

from tests.integration.test_providers_api.conftest import (
    require_env,
    stream_usage,
    report_usage,
)
from providers.anthropic import AnthropicProvider
from streaming.persist import parse_tool_call_inputs

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

# A no-arg tool: Anthropic streams an empty ``input_json_delta`` for these,
# and the pipeline must accept the empty input as ``{}``.
_NO_ARG_TOOL = {
    "name": "get_my_balance",
    "description": "Get the calling user's current balance. Takes no arguments.",
    "input_schema": {
        "type": "object",
        "properties": {},
        "additionalProperties": False,
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
        ("tool", "content", "thinking"),
        [
            pytest.param(
                _WEATHER_TOOL,
                "What is the weather in Bangalore? Use the get_weather tool.",
                None,
                id="arg-tool-no-thinking",
            ),
            pytest.param(
                _WEATHER_TOOL,
                "What is the weather in Bangalore? Use the get_weather tool.",
                _THINKING,
                id="arg-tool-thinking",
            ),
            pytest.param(
                _NO_ARG_TOOL,
                "Call get_my_balance.",
                None,
                id="no-arg-tool",
            ),
        ],
    )
    async def test_stream(
        self, tool: dict, content: str, thinking: dict | None
    ) -> None:
        """Stream a forced tool call, parse it through the real pipeline, and
        continue with the tool result.

        Covers the request shapes that broke in production:

        * With ``thinking`` enabled, the model streams an extended-thinking
          block before the tool_use, shifting the tool_use (and its input
          deltas) to a higher content-block index.  The assistant message
          must be rebuilt from the streamed blocks exactly, or tool input
          deltas land on a synthetic ``tool_use`` with an empty id that the
          API rejects (400 ``tool_use.id``).
        * No-arg tools stream an empty ``input_json_delta``; the pipeline's
          ``parse_tool_call_inputs`` must accept that as ``{}`` rather than
          failing with "Invalid JSON in tool input".

        The follow-up request must be accepted by the API in all cases.
        """
        provider = AnthropicProvider(api_key=API_KEY, model=MODEL)
        messages: list[MessageParam] = [{"role": "user", "content": content}]

        events = [
            e
            async for e in provider.stream_response(
                prompt="",
                messages=messages,
                tools=[tool],
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
        assert tool_use_id is not None, f"Model did not call {tool['name']}"
        assert re.fullmatch(
            r"[a-zA-Z0-9_-]+", tool_use_id
        ), f"Invalid tool_use id: {tool_use_id!r}"

        # Run the rebuilt tool_use blocks through the real pipeline parser —
        # this is what broke for no-arg tools (empty input) and, with
        # thinking, what used to receive a synthetic empty-id block.  It
        # mutates the blocks in place, so the continuation below sends the
        # parsed input object.
        from typing import cast

        tool_use_blocks = cast(
            list[ToolUseBlockParam],
            [b for b in assistant_blocks if b["type"] == "tool_use"],
        )
        parse_errors = parse_tool_call_inputs(tool_use_blocks)
        assert (
            parse_errors == []
        ), f"parse_tool_call_inputs rejected valid tool call: {parse_errors}"

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
                tools=[tool],
                max_tokens=_MAX_TOKENS,
                thinking=thinking,
            )
        ]
        assert events2[-1].type == "message_stop"

        u = stream_usage(events) or stream_usage(events2)
        if u:
            report_usage("Anthropic.stream", u[0], u[1])

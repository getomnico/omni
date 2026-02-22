"""
OpenAI Provider — streams responses and normalizes them to Anthropic MessageStreamEvent format.

Uses the OpenAI Responses API (client.responses.create).
"""

import json
import logging
import time
from collections.abc import AsyncIterator
from typing import Any

from openai import AsyncOpenAI
from anthropic.types import (
    Message,
    Usage,
    RawMessageStartEvent,
    RawContentBlockStartEvent,
    RawContentBlockDeltaEvent,
    RawMessageStopEvent,
    ToolUseBlock,
    TextBlock,
    TextDelta,
    InputJSONDelta,
)
from anthropic.types.message_stream_event import MessageStreamEvent

from . import LLMProvider

logger = logging.getLogger(__name__)


def _convert_tools_to_openai(tools: list[dict[str, Any]]) -> list[dict[str, Any]]:
    """Convert Anthropic tool schema to OpenAI Responses API function-calling format (flat)."""
    return [
        {
            "type": "function",
            "name": tool["name"],
            "description": tool.get("description", ""),
            "parameters": tool["input_schema"],
        }
        for tool in tools
    ]


class OpenAIProvider(LLMProvider):
    """Provider for OpenAI API (GPT-4, etc.) using the Responses API."""

    def __init__(self, api_key: str, model: str):
        self.client = AsyncOpenAI(api_key=api_key)
        self.model = model

    async def stream_response(
        self,
        prompt: str,
        max_tokens: int | None = None,
        temperature: float | None = None,
        top_p: float | None = None,
        tools: list[dict[str, Any]] | None = None,
        messages: list[dict[str, Any]] | None = None,
        system_prompt: str | None = None,
    ) -> AsyncIterator[MessageStreamEvent]:
        """Stream response from OpenAI Responses API, yielding Anthropic-compatible MessageStreamEvents."""
        try:
            input_items = self._convert_messages(
                messages or [{"role": "user", "content": prompt}]
            )

            request_params: dict[str, Any] = {
                "model": self.model,
                "input": input_items,
                "max_output_tokens": max_tokens or 4096,
                "stream": True,
            }

            if system_prompt:
                request_params["instructions"] = system_prompt

            if top_p is not None:
                request_params["top_p"] = top_p

            if tools:
                request_params["tools"] = _convert_tools_to_openai(tools)
                logger.info(
                    f"[OPENAI] Sending request with {len(tools)} tools: {[t['name'] for t in tools]}"
                )

            logger.info(
                f"[OPENAI] Model: {self.model}, Input items: {len(input_items)}, Max tokens: {request_params['max_output_tokens']}"
            )

            stream = await self.client.responses.create(**request_params)

            # Emit message_start
            yield RawMessageStartEvent(
                type="message_start",
                message=Message(
                    id=str(time.time_ns()),
                    type="message",
                    role="assistant",
                    content=[],
                    model=self.model,
                    usage=Usage(input_tokens=0, output_tokens=0),
                ),
            )

            text_started = False
            current_text_index = 0
            tool_call_indices: dict[str, int] = {}  # call_id -> content block index
            next_block_index = 0

            async for event in stream:
                event_type = event.type

                # Handle text deltas
                if event_type == "response.output_text.delta":
                    if not text_started:
                        current_text_index = next_block_index
                        next_block_index += 1
                        text_started = True
                        yield RawContentBlockStartEvent(
                            type="content_block_start",
                            index=current_text_index,
                            content_block=TextBlock(type="text", text=""),
                        )

                    yield RawContentBlockDeltaEvent(
                        type="content_block_delta",
                        index=current_text_index,
                        delta=TextDelta(type="text_delta", text=event.delta),
                    )

                # Handle tool call start
                elif event_type == "response.output_item.added":
                    item = event.item
                    if item.type == "function_call":
                        block_index = next_block_index
                        next_block_index += 1
                        tool_call_indices[item.call_id] = block_index
                        yield RawContentBlockStartEvent(
                            type="content_block_start",
                            index=block_index,
                            content_block=ToolUseBlock(
                                type="tool_use",
                                id=item.call_id,
                                name=item.name,
                                input={},
                            ),
                        )

                # Handle tool call argument deltas
                elif event_type == "response.function_call_arguments.delta":
                    call_id = event.item_id
                    if call_id in tool_call_indices:
                        yield RawContentBlockDeltaEvent(
                            type="content_block_delta",
                            index=tool_call_indices[call_id],
                            delta=InputJSONDelta(
                                type="input_json_delta",
                                partial_json=event.delta,
                            ),
                        )

                # Handle completion
                elif event_type == "response.completed":
                    break

            yield RawMessageStopEvent(type="message_stop")

        except Exception as e:
            logger.error(
                f"[OPENAI] Failed to stream from OpenAI: {str(e)}", exc_info=True
            )

    def _convert_messages(self, messages: list[dict[str, Any]]) -> list[dict[str, Any]]:
        """Convert Anthropic-style messages to OpenAI Responses API input items."""
        input_items: list[dict[str, Any]] = []
        for msg in messages:
            role = msg["role"]
            content = msg.get("content", "")

            if isinstance(content, str):
                input_items.append({"role": role, "content": content})
                continue

            if not isinstance(content, list):
                input_items.append({"role": role, "content": str(content)})
                continue

            # Handle block-based content
            text_parts = []
            tool_calls = []
            tool_results = []

            for block in content:
                if not isinstance(block, dict):
                    continue
                block_type = block.get("type")

                if block_type == "text":
                    text_parts.append(block.get("text", ""))
                elif block_type == "tool_use":
                    tool_calls.append(
                        {
                            "type": "function_call",
                            "call_id": block["id"],
                            "name": block["name"],
                            "arguments": (
                                json.dumps(block["input"])
                                if isinstance(block["input"], dict)
                                else str(block["input"])
                            ),
                        }
                    )
                elif block_type == "tool_result":
                    result_content = block.get("content", "")
                    if isinstance(result_content, list):
                        parts = []
                        for rb in result_content:
                            if isinstance(rb, dict):
                                if rb.get("type") == "text":
                                    parts.append(rb.get("text", ""))
                                elif rb.get("type") == "search_result":
                                    title = rb.get("title", "")
                                    source = rb.get("source", "")
                                    inner = rb.get("content", [])
                                    inner_text = "\n".join(
                                        ib.get("text", "")
                                        for ib in inner
                                        if isinstance(ib, dict)
                                        and ib.get("type") == "text"
                                    )
                                    parts.append(f"[{title}]({source})\n{inner_text}")
                        result_content = "\n\n".join(parts)
                    tool_results.append(
                        {
                            "type": "function_call_output",
                            "call_id": block.get("tool_use_id", ""),
                            "output": str(result_content),
                        }
                    )

            if role == "assistant":
                if text_parts:
                    input_items.append(
                        {"role": "assistant", "content": "\n".join(text_parts)}
                    )
                for tc in tool_calls:
                    input_items.append(tc)
            elif role == "user" and tool_results:
                for tr in tool_results:
                    input_items.append(tr)
            else:
                if text_parts:
                    input_items.append({"role": role, "content": "\n".join(text_parts)})

        return input_items

    async def generate_response(
        self,
        prompt: str,
        max_tokens: int | None = None,
        temperature: float | None = None,
        top_p: float | None = None,
    ) -> str:
        """Generate non-streaming response from OpenAI Responses API."""
        try:
            params: dict[str, Any] = {
                "model": self.model,
                "input": prompt,
                "max_output_tokens": max_tokens or 4096,
                "stream": False,
            }
            response = await self.client.responses.create(**params)

            content = response.output_text
            if not content:
                raise Exception("Empty response from OpenAI")

            return content

        except Exception as e:
            logger.error(f"[OPENAI] Failed to generate response: {str(e)}")
            raise Exception(f"Failed to generate response: {str(e)}")

    async def health_check(self) -> bool:
        """Check if OpenAI API is accessible."""
        try:
            await self.client.responses.create(
                model=self.model,
                input="Hello",
                max_output_tokens=1,
                stream=False,
            )
            return True
        except Exception:
            return False

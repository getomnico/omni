"""
OpenAI Provider — streams responses and normalizes them to Anthropic MessageStreamEvent format.
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
    """Convert Anthropic tool schema to OpenAI function-calling format."""
    openai_tools = []
    for tool in tools:
        openai_tools.append(
            {
                "type": "function",
                "function": {
                    "name": tool["name"],
                    "description": tool.get("description", ""),
                    "parameters": tool["input_schema"],
                },
            }
        )
    return openai_tools


class OpenAIProvider(LLMProvider):
    """Provider for OpenAI API (GPT-4, etc.)."""

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
    ) -> AsyncIterator[MessageStreamEvent]:
        """Stream response from OpenAI, yielding Anthropic-compatible MessageStreamEvents."""
        try:
            msg_list = self._convert_messages(
                messages or [{"role": "user", "content": prompt}]
            )

            request_params: dict[str, Any] = {
                "model": self.model,
                "messages": msg_list,
                "max_tokens": max_tokens or 4096,
                "temperature": temperature or 0.7,
                "stream": True,
            }

            if top_p is not None:
                request_params["top_p"] = top_p

            if tools:
                request_params["tools"] = _convert_tools_to_openai(tools)
                logger.info(
                    f"[OPENAI] Sending request with {len(tools)} tools: {[t['name'] for t in tools]}"
                )

            logger.info(
                f"[OPENAI] Model: {self.model}, Messages: {len(msg_list)}, Max tokens: {request_params['max_tokens']}"
            )

            stream = await self.client.chat.completions.create(**request_params)

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

            # Track content blocks: OpenAI streams text in choice deltas and tool calls separately
            current_text_index = 0
            text_started = False
            tool_call_indices: dict[int, int] = (
                {}
            )  # openai tool index -> our content block index
            next_block_index = 0

            async for chunk in stream:
                if not chunk.choices:
                    continue

                delta = chunk.choices[0].delta

                # Handle text content
                if delta.content is not None:
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
                        delta=TextDelta(type="text_delta", text=delta.content),
                    )

                # Handle tool calls
                if delta.tool_calls:
                    for tc in delta.tool_calls:
                        tc_idx = tc.index
                        if tc_idx not in tool_call_indices:
                            block_index = next_block_index
                            next_block_index += 1
                            tool_call_indices[tc_idx] = block_index
                            yield RawContentBlockStartEvent(
                                type="content_block_start",
                                index=block_index,
                                content_block=ToolUseBlock(
                                    type="tool_use",
                                    id=tc.id or "",
                                    name=(
                                        tc.function.name
                                        if tc.function and tc.function.name
                                        else ""
                                    ),
                                    input={},
                                ),
                            )

                        if tc.function and tc.function.arguments:
                            yield RawContentBlockDeltaEvent(
                                type="content_block_delta",
                                index=tool_call_indices[tc_idx],
                                delta=InputJSONDelta(
                                    type="input_json_delta",
                                    partial_json=tc.function.arguments,
                                ),
                            )

                # Handle finish
                if chunk.choices[0].finish_reason is not None:
                    break

            yield RawMessageStopEvent(type="message_stop")

        except Exception as e:
            logger.error(
                f"[OPENAI] Failed to stream from OpenAI: {str(e)}", exc_info=True
            )

    def _convert_messages(self, messages: list[dict[str, Any]]) -> list[dict[str, Any]]:
        """Convert Anthropic-style messages to OpenAI format."""
        openai_messages = []
        for msg in messages:
            role = msg["role"]
            content = msg.get("content", "")

            if isinstance(content, str):
                openai_messages.append({"role": role, "content": content})
                continue

            if not isinstance(content, list):
                openai_messages.append({"role": role, "content": str(content)})
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
                            "id": block["id"],
                            "type": "function",
                            "function": {
                                "name": block["name"],
                                "arguments": (
                                    json.dumps(block["input"])
                                    if isinstance(block["input"], dict)
                                    else str(block["input"])
                                ),
                            },
                        }
                    )
                elif block_type == "tool_result":
                    result_content = block.get("content", "")
                    if isinstance(result_content, list):
                        # Extract text from search_result and text blocks
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
                            "role": "tool",
                            "tool_call_id": block.get("tool_use_id", ""),
                            "content": str(result_content),
                        }
                    )

            if role == "assistant":
                msg_dict: dict[str, Any] = {"role": "assistant"}
                if text_parts:
                    msg_dict["content"] = "\n".join(text_parts)
                if tool_calls:
                    msg_dict["tool_calls"] = tool_calls
                    if "content" not in msg_dict:
                        msg_dict["content"] = None
                openai_messages.append(msg_dict)
            elif role == "user" and tool_results:
                for tr in tool_results:
                    openai_messages.append(tr)
            else:
                if text_parts:
                    openai_messages.append(
                        {"role": role, "content": "\n".join(text_parts)}
                    )

        return openai_messages

    async def generate_response(
        self,
        prompt: str,
        max_tokens: int | None = None,
        temperature: float | None = None,
        top_p: float | None = None,
    ) -> str:
        """Generate non-streaming response from OpenAI."""
        try:
            response = await self.client.chat.completions.create(
                model=self.model,
                messages=[{"role": "user", "content": prompt}],
                max_tokens=max_tokens or 4096,
                temperature=temperature or 0.7,
                stream=False,
            )

            content = response.choices[0].message.content
            if not content:
                raise Exception("Empty response from OpenAI")

            return content

        except Exception as e:
            logger.error(f"[OPENAI] Failed to generate response: {str(e)}")
            raise Exception(f"Failed to generate response: {str(e)}")

    async def health_check(self) -> bool:
        """Check if OpenAI API is accessible."""
        try:
            await self.client.chat.completions.create(
                model=self.model,
                messages=[{"role": "user", "content": "Hello"}],
                max_tokens=1,
                stream=False,
            )
            return True
        except Exception:
            return False

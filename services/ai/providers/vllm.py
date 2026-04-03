"""
vLLM Provider — uses the OpenAI Chat Completions API that vLLM exposes.

vLLM serves an OpenAI-compatible API, so we use the OpenAI SDK with a custom
base_url. This gives us full tool/function-calling support for free.
"""

import json
import logging
import time
from collections.abc import AsyncIterator
from typing import Any, cast

import httpx
from openai import AsyncOpenAI
from openai.types.chat import (
    ChatCompletionAssistantMessageParam,
    ChatCompletionChunk,
    ChatCompletionMessageParam,
    ChatCompletionMessageToolCallParam,
    ChatCompletionSystemMessageParam,
    ChatCompletionToolMessageParam,
    ChatCompletionToolParam,
    ChatCompletionUserMessageParam,
)
from openai.types.chat.chat_completion_message_tool_call_param import Function
from anthropic.types import (
    Message,
    MessageParam,
    TextBlockParam,
    ToolParam,
    ToolResultBlockParam,
    ToolUseBlockParam,
    Usage,
    RawMessageStartEvent,
    RawContentBlockStartEvent,
    RawContentBlockDeltaEvent,
    RawContentBlockStopEvent,
    RawMessageStopEvent,
    ToolUseBlock,
    TextBlock,
    TextDelta,
    InputJSONDelta,
)
from anthropic.types.message_stream_event import MessageStreamEvent

from . import LLMProvider

logger = logging.getLogger(__name__)


def _convert_tools_to_openai(tools: list[ToolParam]) -> list[ChatCompletionToolParam]:
    """Convert Anthropic tool schema to OpenAI Chat Completions function-calling format."""
    result: list[ChatCompletionToolParam] = []
    for tool in tools:
        result.append(
            ChatCompletionToolParam(
                type="function",
                function={
                    "name": tool["name"],
                    "description": tool.get("description", ""),
                    "parameters": cast(dict[str, object], tool["input_schema"]),
                },
            )
        )
    return result


def _convert_messages_to_openai(
    messages: list[MessageParam],
) -> list[ChatCompletionMessageParam]:
    """Convert Anthropic-style messages to OpenAI Chat Completions format."""
    result: list[ChatCompletionMessageParam] = []

    for msg in messages:
        role = msg["role"]
        content = msg.get("content", "")

        if isinstance(content, str):
            if role == "assistant":
                result.append(
                    ChatCompletionAssistantMessageParam(
                        role="assistant", content=content
                    )
                )
            else:
                result.append(
                    ChatCompletionUserMessageParam(role="user", content=content)
                )
            continue

        if not isinstance(content, list):
            if role == "assistant":
                result.append(
                    ChatCompletionAssistantMessageParam(
                        role="assistant", content=str(content)
                    )
                )
            else:
                result.append(
                    ChatCompletionUserMessageParam(role="user", content=str(content))
                )
            continue

        # Handle block-based content (Anthropic format)
        text_parts: list[str] = []
        tool_calls: list[ChatCompletionMessageToolCallParam] = []
        tool_results: list[ChatCompletionToolMessageParam] = []

        for block in content:
            if not isinstance(block, dict):
                continue

            block = cast(
                TextBlockParam | ToolUseBlockParam | ToolResultBlockParam, block
            )

            if block["type"] == "text":
                block = cast(TextBlockParam, block)
                text_parts.append(block["text"])
            elif block["type"] == "tool_use":
                block = cast(ToolUseBlockParam, block)
                raw_input = block["input"]
                tool_calls.append(
                    ChatCompletionMessageToolCallParam(
                        id=block["id"],
                        type="function",
                        function=Function(
                            name=block["name"],
                            arguments=(
                                json.dumps(raw_input)
                                if isinstance(raw_input, dict)
                                else str(raw_input)
                            ),
                        ),
                    )
                )
            elif block["type"] == "tool_result":
                block = cast(ToolResultBlockParam, block)
                result_content = block.get("content", "")
                if isinstance(result_content, list):
                    parts: list[str] = []
                    for rb in result_content:
                        if not isinstance(rb, dict):
                            continue
                        if rb.get("type") == "text":
                            rb = cast(TextBlockParam, rb)
                            parts.append(rb["text"])
                        elif rb.get("type") == "search_result":
                            title = rb.get("title", "")
                            source = rb.get("source", "")
                            inner = rb.get("content", [])
                            inner_text = "\n".join(
                                ib["text"]
                                for ib in inner
                                if isinstance(ib, dict) and ib.get("type") == "text"
                            )
                            parts.append(f"[{title}]({source})\n{inner_text}")
                    result_content = "\n\n".join(parts)
                tool_results.append(
                    ChatCompletionToolMessageParam(
                        role="tool",
                        tool_call_id=block["tool_use_id"],
                        content=str(result_content),
                    )
                )

        if role == "assistant":
            assistant_msg = ChatCompletionAssistantMessageParam(role="assistant")
            if text_parts:
                assistant_msg["content"] = "\n".join(text_parts)
            if tool_calls:
                assistant_msg["tool_calls"] = tool_calls
            result.append(assistant_msg)
        elif role == "user" and tool_results:
            result.extend(tool_results)
        else:
            if text_parts:
                result.append(
                    ChatCompletionUserMessageParam(
                        role="user", content="\n".join(text_parts)
                    )
                )

    return result


class VLLMProvider(LLMProvider):
    """Provider for vLLM's OpenAI-compatible API.

    Uses the OpenAI SDK pointed at the vLLM server, giving us Chat Completions
    with full tool/function-calling support.
    """

    def __init__(self, vllm_url: str, model: str = "default"):
        self.vllm_url = vllm_url.rstrip("/")
        self.model = model
        self.client = AsyncOpenAI(
            api_key="unused",
            base_url=f"{self.vllm_url}/v1",
        )
        # Keep a raw httpx client for the health check endpoint
        self._http_client = httpx.AsyncClient(timeout=10.0)

    async def stream_response(
        self,
        prompt: str,
        max_tokens: int | None = None,
        temperature: float | None = None,
        top_p: float | None = None,
        tools: list[ToolParam] | None = None,
        messages: list[MessageParam] | None = None,
        system_prompt: str | None = None,
    ) -> AsyncIterator[MessageStreamEvent]:
        """Stream response from vLLM, yielding Anthropic-compatible MessageStreamEvents."""
        try:
            openai_messages = _convert_messages_to_openai(
                messages or [{"role": "user", "content": prompt}]
            )

            if system_prompt:
                system_msg = ChatCompletionSystemMessageParam(
                    role="system", content=system_prompt
                )
                openai_messages = [system_msg] + openai_messages

            params: dict[str, Any] = {
                "model": self.model,
                "messages": openai_messages,
                "max_tokens": max_tokens or 4096,
                "stream": True,
            }

            if temperature is not None:
                params["temperature"] = temperature
            if top_p is not None:
                params["top_p"] = top_p

            if tools:
                params["tools"] = _convert_tools_to_openai(tools)
                logger.info(
                    f"Sending request with {len(tools)} tools: {[t['name'] for t in tools]}"
                )

            stream = await self.client.chat.completions.create(**params)

            # Emit message_start
            yield RawMessageStartEvent(
                type="message_start",
                message=Message(
                    id=f"vllm-{time.time_ns()}",
                    type="message",
                    role="assistant",
                    content=[],
                    model=self.model,
                    usage=Usage(input_tokens=0, output_tokens=0),
                ),
            )

            text_started = False
            current_text_index = 0
            # tool_call index (from OpenAI) -> our content block index
            tool_block_indices: dict[int, int] = {}
            tool_call_ids: dict[int, str] = {}
            tool_call_names: dict[int, str] = {}
            next_block_index = 0

            chunk: ChatCompletionChunk
            async for chunk in stream:
                if not chunk.choices:
                    continue

                delta = chunk.choices[0].delta

                # Handle text content
                if delta.content:
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
                    for tc_delta in delta.tool_calls:
                        tc_index = tc_delta.index

                        # New tool call — emit content_block_start
                        if tc_index not in tool_block_indices:
                            # Close text block if open
                            if text_started:
                                yield RawContentBlockStopEvent(
                                    type="content_block_stop",
                                    index=current_text_index,
                                )
                                text_started = False

                            block_index = next_block_index
                            next_block_index += 1
                            tool_block_indices[tc_index] = block_index

                            call_id = tc_delta.id or f"call_{tc_index}"
                            tool_call_ids[tc_index] = call_id
                            name = (
                                tc_delta.function.name
                                if tc_delta.function and tc_delta.function.name
                                else ""
                            )
                            tool_call_names[tc_index] = name

                            yield RawContentBlockStartEvent(
                                type="content_block_start",
                                index=block_index,
                                content_block=ToolUseBlock(
                                    type="tool_use",
                                    id=call_id,
                                    name=name,
                                    input={},
                                ),
                            )

                        # Argument deltas
                        if tc_delta.function and tc_delta.function.arguments:
                            yield RawContentBlockDeltaEvent(
                                type="content_block_delta",
                                index=tool_block_indices[tc_index],
                                delta=InputJSONDelta(
                                    type="input_json_delta",
                                    partial_json=tc_delta.function.arguments,
                                ),
                            )

                # Handle finish_reason
                if chunk.choices[0].finish_reason is not None:
                    break

            # Close any open blocks
            if text_started:
                yield RawContentBlockStopEvent(
                    type="content_block_stop",
                    index=current_text_index,
                )
            for tc_index, block_index in tool_block_indices.items():
                yield RawContentBlockStopEvent(
                    type="content_block_stop",
                    index=block_index,
                )

            yield RawMessageStopEvent(type="message_stop")

        except Exception as e:
            logger.error(f"Failed to stream from vLLM: {e}", exc_info=True)

    async def generate_response(
        self,
        prompt: str,
        max_tokens: int | None = None,
        temperature: float | None = None,
        top_p: float | None = None,
    ) -> str:
        """Generate non-streaming response from vLLM."""
        try:
            params: dict[str, Any] = {
                "model": self.model,
                "messages": [{"role": "user", "content": prompt}],
                "max_tokens": max_tokens or 4096,
                "stream": False,
            }
            if temperature is not None:
                params["temperature"] = temperature
            if top_p is not None:
                params["top_p"] = top_p

            response = await self.client.chat.completions.create(**params)

            content = response.choices[0].message.content
            if not content:
                raise Exception("Empty response from vLLM service")

            return content

        except Exception as e:
            raise Exception(f"Failed to generate response from vLLM: {e}")

    async def health_check(self) -> bool:
        """Check if vLLM service is healthy."""
        try:
            response = await self._http_client.get(f"{self.vllm_url}/health")
            return response.status_code == 200
        except Exception:
            return False

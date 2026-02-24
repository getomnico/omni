"""
Gemini Provider — streams responses and normalizes them to Anthropic MessageStreamEvent format.
"""

import json
import logging
import time
from collections.abc import AsyncIterator
from typing import Any

from google import genai
from google.genai import types
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


def _convert_tools_to_gemini(tools: list[dict[str, Any]]) -> list[types.Tool]:
    """Convert Anthropic tool schema to Gemini function declarations."""
    declarations = []
    for tool in tools:
        declarations.append(
            types.FunctionDeclaration(
                name=tool["name"],
                description=tool.get("description", ""),
                parameters=tool.get("input_schema"),
            )
        )
    return [types.Tool(function_declarations=declarations)]


def _convert_messages_to_gemini(
    messages: list[dict[str, Any]],
) -> list[types.Content]:
    """Convert Anthropic-style messages to Gemini Content format."""
    gemini_contents = []

    for msg in messages:
        role = msg["role"]
        gemini_role = "model" if role == "assistant" else "user"
        content = msg.get("content", "")

        if isinstance(content, str):
            gemini_contents.append(
                types.Content(role=gemini_role, parts=[types.Part(text=content)])
            )
            continue

        if not isinstance(content, list):
            gemini_contents.append(
                types.Content(role=gemini_role, parts=[types.Part(text=str(content))])
            )
            continue

        parts: list[types.Part] = []
        for block in content:
            if not isinstance(block, dict):
                continue
            block_type = block.get("type")

            if block_type == "text":
                text = block.get("text", "")
                if text:
                    parts.append(types.Part(text=text))

            elif block_type == "tool_use":
                parts.append(
                    types.Part(
                        function_call=types.FunctionCall(
                            name=block["name"],
                            args=block.get("input", {}),
                        )
                    )
                )

            elif block_type == "tool_result":
                result_content = block.get("content", "")
                if isinstance(result_content, list):
                    text_parts = []
                    for rb in result_content:
                        if isinstance(rb, dict):
                            if rb.get("type") == "text":
                                text_parts.append(rb.get("text", ""))
                            elif rb.get("type") == "search_result":
                                title = rb.get("title", "")
                                source = rb.get("source", "")
                                inner = rb.get("content", [])
                                inner_text = "\n".join(
                                    ib.get("text", "")
                                    for ib in inner
                                    if isinstance(ib, dict) and ib.get("type") == "text"
                                )
                                text_parts.append(f"[{title}]({source})\n{inner_text}")
                    result_content = "\n\n".join(text_parts)

                tool_name = block.get("tool_use_id", "unknown")
                # Try to find the tool name from a preceding assistant message
                for prev_msg in reversed(gemini_contents):
                    if prev_msg.role == "model" and prev_msg.parts:
                        for p in prev_msg.parts:
                            if p.function_call and p.function_call.id == block.get(
                                "tool_use_id"
                            ):
                                tool_name = p.function_call.name
                                break
                            if p.function_call:
                                tool_name = p.function_call.name
                        break

                parts.append(
                    types.Part(
                        function_response=types.FunctionResponse(
                            name=tool_name,
                            response={"result": str(result_content)},
                        )
                    )
                )

        if parts:
            gemini_contents.append(types.Content(role=gemini_role, parts=parts))

    return gemini_contents


class GeminiProvider(LLMProvider):
    """Provider for Google Gemini API."""

    def __init__(self, api_key: str, model: str):
        self.client = genai.Client(api_key=api_key)
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
        """Stream response from Gemini, yielding Anthropic-compatible MessageStreamEvents."""
        try:
            contents = _convert_messages_to_gemini(
                messages or [{"role": "user", "content": prompt}]
            )

            config = types.GenerateContentConfig(
                max_output_tokens=max_tokens or 4096,
                temperature=temperature or 0.7,
            )

            if top_p is not None:
                config.top_p = top_p

            if system_prompt:
                config.system_instruction = system_prompt

            if tools:
                config.tools = _convert_tools_to_gemini(tools)
                logger.info(
                    f"Sending request with {len(tools)} tools: {[t['name'] for t in tools]}"
                )

            logger.info(
                f"Model: {self.model}, Messages: {len(contents)}, Max tokens: {config.max_output_tokens}"
            )

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

            next_block_index = 0
            text_started = False
            current_text_index = 0

            async for chunk in await self.client.aio.models.generate_content_stream(
                model=self.model,
                contents=contents,
                config=config,
            ):
                if not chunk.candidates:
                    continue

                candidate = chunk.candidates[0]
                if not candidate.content or not candidate.content.parts:
                    continue

                for part in candidate.content.parts:
                    # Handle text parts
                    if part.text is not None:
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
                            delta=TextDelta(type="text_delta", text=part.text),
                        )

                    # Handle function call parts
                    elif part.function_call is not None:
                        block_index = next_block_index
                        next_block_index += 1
                        tool_call_id = f"toolu_{time.time_ns()}"

                        yield RawContentBlockStartEvent(
                            type="content_block_start",
                            index=block_index,
                            content_block=ToolUseBlock(
                                type="tool_use",
                                id=tool_call_id,
                                name=part.function_call.name or "",
                                input={},
                            ),
                        )

                        args = part.function_call.args or {}
                        if args:
                            yield RawContentBlockDeltaEvent(
                                type="content_block_delta",
                                index=block_index,
                                delta=InputJSONDelta(
                                    type="input_json_delta",
                                    partial_json=json.dumps(dict(args)),
                                ),
                            )

            yield RawMessageStopEvent(type="message_stop")

        except Exception as e:
            logger.error(f"Failed to stream from Gemini: {str(e)}", exc_info=True)

    async def generate_response(
        self,
        prompt: str,
        max_tokens: int | None = None,
        temperature: float | None = None,
        top_p: float | None = None,
    ) -> str:
        """Generate non-streaming response from Gemini."""
        try:
            config = types.GenerateContentConfig(
                max_output_tokens=max_tokens or 4096,
                temperature=temperature or 0.7,
            )
            if top_p is not None:
                config.top_p = top_p

            response = await self.client.aio.models.generate_content(
                model=self.model,
                contents=prompt,
                config=config,
            )

            if not response.text:
                raise Exception("Empty response from Gemini")

            return response.text

        except Exception as e:
            logger.error(f"Failed to generate response: {str(e)}")
            raise Exception(f"Failed to generate response: {str(e)}")

    async def health_check(self) -> bool:
        """Check if Gemini API is accessible."""
        try:
            config = types.GenerateContentConfig(max_output_tokens=1)
            await self.client.aio.models.generate_content(
                model=self.model,
                contents="Hello",
                config=config,
            )
            return True
        except Exception:
            return False

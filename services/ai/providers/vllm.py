"""
VLLM Provider for OpenAI-compatible API.
"""

import json
import logging
import time
from collections.abc import AsyncIterator
from typing import Any

import httpx
from anthropic.types import (
    Message,
    Usage,
    RawMessageStartEvent,
    RawContentBlockStartEvent,
    RawContentBlockDeltaEvent,
    RawContentBlockStopEvent,
    RawMessageStopEvent,
    TextBlock,
    TextDelta,
)
from anthropic.types.message_stream_event import MessageStreamEvent

from . import LLMProvider

logger = logging.getLogger(__name__)


class VLLMProvider(LLMProvider):
    """Provider for vLLM OpenAI-compatible API."""

    def __init__(self, vllm_url: str):
        self.vllm_url = vllm_url
        self.client = httpx.AsyncClient(timeout=60.0)

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
        """Stream response from vLLM service."""
        # vLLM doesn't support tools yet, so we ignore the tools parameter
        if tools:
            logger.warning(
                "vLLM provider does not support tools, ignoring tools parameter"
            )

        # Use provided messages or create from prompt
        msg_list = messages or [{"role": "user", "content": prompt}]

        if system_prompt:
            msg_list = [{"role": "system", "content": system_prompt}] + msg_list

        payload = {
            "model": "placeholder",  # vLLM ignores this but requires it
            "messages": msg_list,
            "max_tokens": max_tokens or 512,
            "temperature": temperature or 0.7,
            "top_p": top_p or 0.9,
            "stream": True,
        }

        try:
            yield RawMessageStartEvent(
                type="message_start",
                message=Message(
                    id=str(time.time_ns()),
                    type="message",
                    role="assistant",
                    content=[],
                    model="vllm",
                    usage=Usage(input_tokens=0, output_tokens=0),
                ),
            )

            yield RawContentBlockStartEvent(
                type="content_block_start",
                index=0,
                content_block=TextBlock(type="text", text=""),
            )

            async with self.client.stream(
                "POST",
                f"{self.vllm_url}/v1/chat/completions",
                json=payload,
                headers={"Accept": "text/event-stream"},
            ) as response:
                response.raise_for_status()

                async for chunk in response.aiter_lines():
                    if chunk:
                        if chunk.startswith("data: "):
                            chunk_data = chunk[6:]  # Remove "data: " prefix

                            if chunk_data == "[DONE]":
                                break

                            try:
                                chunk_json = json.loads(chunk_data)
                                choices = chunk_json.get("choices", [])
                                if choices and len(choices) > 0:
                                    delta = choices[0].get("delta", {})
                                    content = delta.get("content", "")
                                    if content:
                                        yield RawContentBlockDeltaEvent(
                                            type="content_block_delta",
                                            index=0,
                                            delta=TextDelta(
                                                type="text_delta", text=content
                                            ),
                                        )
                            except json.JSONDecodeError:
                                continue

            yield RawContentBlockStopEvent(type="content_block_stop", index=0)
            yield RawMessageStopEvent(type="message_stop")

        except httpx.TimeoutException:
            logger.error("Timeout while calling vLLM service")
        except httpx.HTTPStatusError as e:
            logger.error(f"HTTP error from vLLM service: {e.response.status_code}")
        except Exception as e:
            logger.error(f"Failed to stream from vLLM: {str(e)}")

    async def generate_response(
        self,
        prompt: str,
        max_tokens: int | None = None,
        temperature: float | None = None,
        top_p: float | None = None,
    ) -> str:
        """Generate non-streaming response from vLLM service."""
        payload = {
            "model": "placeholder",  # vLLM ignores this but requires it
            "messages": [{"role": "user", "content": prompt}],
            "max_tokens": max_tokens or 512,
            "temperature": temperature or 0.7,
            "top_p": top_p or 0.9,
            "stream": False,
        }

        try:
            response = await self.client.post(
                f"{self.vllm_url}/v1/chat/completions", json=payload
            )
            response.raise_for_status()

            vllm_response = response.json()
            choices = vllm_response.get("choices", [])
            if not choices:
                raise Exception("No choices in vLLM response")

            message = choices[0].get("message", {})
            generated_text = message.get("content", "")

            if not generated_text:
                raise Exception("Empty response from vLLM service")

            return generated_text

        except httpx.TimeoutException:
            raise Exception("Request to vLLM service timed out")
        except httpx.HTTPStatusError as e:
            raise Exception(f"vLLM service error: {e.response.status_code}")
        except Exception as e:
            raise Exception(f"Failed to generate response: {str(e)}")

    async def health_check(self) -> bool:
        """Check if vLLM service is healthy."""
        try:
            response = await self.client.get(f"{self.vllm_url}/health")
            return response.status_code == 200
        except Exception:
            return False
